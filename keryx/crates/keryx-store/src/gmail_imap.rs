//! Gmail's Inbox label over IMAP, and nothing else (spec `keryx-gmail-imap.md`, 28 Oct).
//!
//! Asked through Mail, an archive on Personal became `APPEND` of a fresh copy plus `\Deleted` and
//! `EXPUNGE` of the original, with the Inbox label still on the copy; on OQTS it was recorded in
//! Mail's index and never sent (Connection Doctor, 28 Oct). This client says to Gmail exactly
//! what an archive means: `UID STORE <uid> -X-GM-LABELS (\Inbox)`, and `+` to put it back.
//!
//! **It cannot delete.** [`Command`] is the whole vocabulary and [`Session::exec`] takes nothing
//! else, so there is no way to send `APPEND`, `COPY`, `MOVE`, `EXPUNGE`, a flag, or a label
//! other than `\Inbox`; a test renders every command and checks it against that list.
//!
//! The message is found by its Message-ID in All Mail (`X-GM-RAW "rfc822msgid:…"`), never by a
//! remembered UID: Mail's moves renumber messages, and a Message-ID can stand for several copies
//! (Mail's uploads left two), so every copy is changed. All Mail is found by its special-use
//! flag `\All`, since its name is localised ("[Google Mail]/All Mail" in some accounts).
//!
//! Sign-in is an app password from the Keychain (`keryx-imap-<account>`), never logged.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

pub const HOST: &str = "imap.gmail.com";
pub const PORT: u16 = 993;
const TIMEOUT: Duration = Duration::from_secs(20);

/// The Keychain item holding an account's app password.
pub fn keychain_service(account_id: &str) -> String {
    format!("keryx-imap-{account_id}")
}

#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    #[error("Gmail IMAP: can't connect: {0}")]
    Connect(String),
    #[error("Gmail IMAP: sign-in refused ({0}): check the app password (docs/setup.md)")]
    Login(String),
    #[error("Gmail IMAP: no message with Message-ID {0} in All Mail")]
    NotFound(String),
    #[error("Gmail IMAP: {0}")]
    Protocol(String),
}

type Result<T> = std::result::Result<T, ImapError>;

/// Every command this client can send.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Login {
        user: String,
        pass: String,
    },
    /// The folder list, to find All Mail by its `\All` flag.
    List,
    Select(String),
    /// The UIDs of every copy with this Message-ID.
    Search(String),
    /// Add (`on`) or take away the Inbox label.
    SetInbox {
        uid: u32,
        on: bool,
    },
    Logout,
}

impl Command {
    /// The command as sent, without its tag.
    pub fn render(&self) -> Result<String> {
        Ok(match self {
            Command::Login { user, pass } => format!("LOGIN {} {}", quote(user)?, quote(pass)?),
            Command::List => r#"LIST "" "*""#.to_string(),
            Command::Select(mailbox) => format!("SELECT {}", quote(mailbox)?),
            Command::Search(id) => format!(
                "UID SEARCH X-GM-RAW {}",
                quote(&format!("rfc822msgid:{}", id.trim_matches(['<', '>'])))?
            ),
            Command::SetInbox { uid, on } => {
                format!("UID STORE {uid} {}X-GM-LABELS (\\Inbox)", if *on { '+' } else { '-' })
            }
            Command::Logout => "LOGOUT".to_string(),
        })
    }

    /// What the log may show: the password never.
    fn shown(&self) -> String {
        match self {
            Command::Login { user, .. } => format!("LOGIN {user} ***"),
            c => c.render().unwrap_or_default(),
        }
    }
}

/// An IMAP quoted string. CR, LF and non-ASCII cannot be quoted, and none occur in an address,
/// an app password, a Message-ID or Gmail's folder names, so they are refused, not encoded.
fn quote(s: &str) -> Result<String> {
    if s.chars().any(|c| c == '\r' || c == '\n' || !c.is_ascii()) {
        return Err(ImapError::Protocol(format!("can't send {s:?} in a command")));
    }
    Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// One signed-in conversation with the server.
pub struct Session<S: Read + Write> {
    io: BufReader<S>,
    n: u32,
}

impl<S: Read + Write> Session<S> {
    /// Read the greeting and sign in.
    pub fn start(stream: S, user: &str, pass: &str) -> Result<Self> {
        let mut s = Session {
            io: BufReader::new(stream),
            n: 0,
        };
        let hello = s.line()?;
        if !hello.starts_with("* OK") {
            return Err(ImapError::Protocol(format!("unexpected greeting: {}", hello.trim())));
        }
        s.exec(&Command::Login {
            user: user.to_string(),
            pass: pass.to_string(),
        })
        .map_err(|e| match e {
            ImapError::Protocol(m) => ImapError::Login(m),
            e => e,
        })?;
        Ok(s)
    }

    /// Send one command and return its untagged replies; anything but OK is an error.
    pub fn exec(&mut self, cmd: &Command) -> Result<Vec<String>> {
        self.n += 1;
        let tag = format!("k{}", self.n);
        let line = format!("{tag} {}\r\n", cmd.render()?);
        tracing::debug!(command = cmd.shown(), "gmail imap");
        let w = self.io.get_mut();
        w.write_all(line.as_bytes())
            .and_then(|()| w.flush())
            .map_err(|e| ImapError::Connect(e.to_string()))?;
        let mut untagged = Vec::new();
        loop {
            let l = self.line()?;
            if let Some(rest) = l.strip_prefix(&format!("{tag} ")) {
                let rest = rest.trim_end();
                return if rest.starts_with("OK") {
                    Ok(untagged)
                } else {
                    Err(ImapError::Protocol(format!("{} → {rest}", cmd.shown())))
                };
            }
            if l.starts_with('+') {
                return Err(ImapError::Protocol(format!("{} asked for more", cmd.shown())));
            }
            untagged.push(l.trim_end().to_string());
        }
    }

    /// One response line, with any `{n}` literals it carries read in.
    fn line(&mut self) -> Result<String> {
        let mut out = String::new();
        loop {
            let mut buf = Vec::new();
            let n = self
                .io
                .read_until(b'\n', &mut buf)
                .map_err(|e| ImapError::Connect(e.to_string()))?;
            if n == 0 {
                return Err(ImapError::Connect("the server closed the connection".into()));
            }
            let part = String::from_utf8_lossy(&buf).into_owned();
            let trimmed = part.trim_end();
            out.push_str(&part);
            let Some(len) = literal_len(trimmed) else {
                return Ok(out);
            };
            let mut lit = vec![0u8; len];
            self.io
                .read_exact(&mut lit)
                .map_err(|e| ImapError::Connect(e.to_string()))?;
            out.push_str(&String::from_utf8_lossy(&lit));
        }
    }

    /// All Mail's name, from its `\All` flag.
    pub fn all_mail(&mut self) -> Result<String> {
        self.exec(&Command::List)?
            .iter()
            .filter(|l| l.starts_with("* LIST") && l.contains("\\All"))
            .find_map(|l| last_name(l))
            .ok_or_else(|| ImapError::Protocol("no folder is marked \\All (All Mail)".into()))
    }

    /// Put every copy of `message_id` in the inbox (`on`) or take it out; the UIDs changed.
    pub fn set_inbox(&mut self, message_id: &str, on: bool) -> Result<Vec<u32>> {
        let all = self.all_mail()?;
        self.exec(&Command::Select(all))?;
        let uids: Vec<u32> = self
            .exec(&Command::Search(message_id.to_string()))?
            .iter()
            .filter_map(|l| l.strip_prefix("* SEARCH"))
            .flat_map(|l| {
                l.split_whitespace()
                    .filter_map(|n| n.parse().ok())
                    .collect::<Vec<u32>>()
            })
            .collect();
        if uids.is_empty() {
            return Err(ImapError::NotFound(message_id.to_string()));
        }
        for &uid in &uids {
            let reply = self.exec(&Command::SetInbox { uid, on })?;
            // Gmail answers with the labels it now holds: they are checked, not assumed.
            if let Some(labels) = reply.iter().find_map(|l| fetched_labels(l))
                && has_inbox(&labels) != on
            {
                return Err(ImapError::Protocol(format!(
                    "Gmail still shows UID {uid} {} the inbox",
                    if on { "out of" } else { "in" }
                )));
            }
        }
        Ok(uids)
    }

    pub fn logout(mut self) {
        let _ = self.exec(&Command::Logout);
    }
}

/// `{123}` at the end of a line: a literal of that many bytes follows.
fn literal_len(line: &str) -> Option<usize> {
    let open = line.strip_suffix('}')?.rfind('{')?;
    line[open + 1..line.len() - 1].parse().ok()
}

/// The folder name a `* LIST (…) "/" name` line ends with.
fn last_name(line: &str) -> Option<String> {
    let line = line.trim_end();
    if let Some(body) = line.strip_suffix('"') {
        let start = body.rfind(" \"")?;
        return Some(body[start + 2..].replace("\\\"", "\"").replace("\\\\", "\\"));
    }
    line.rsplit(' ').next().map(str::to_string)
}

/// The labels in `* n FETCH (X-GM-LABELS (…) …)`.
fn fetched_labels(line: &str) -> Option<String> {
    let start = line.find("X-GM-LABELS (")? + "X-GM-LABELS (".len();
    let end = line[start..].find(')')?;
    Some(line[start..start + end].to_string())
}

fn has_inbox(labels: &str) -> bool {
    labels
        .split_whitespace()
        .map(|t| t.trim_matches('"').replace("\\\\", "\\"))
        .any(|t| t.eq_ignore_ascii_case("\\Inbox"))
}

/// A TLS connection to Gmail, checked against the Mac's trusted certificates.
pub fn connect(host: &str, port: u16) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>> {
    use rustls_platform_verifier::ConfigVerifierExt;
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| ImapError::Connect(e.to_string()))?
        .next()
        .ok_or_else(|| ImapError::Connect(format!("{host} has no address")))?;
    let tcp = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| ImapError::Connect(e.to_string()))?;
    tcp.set_read_timeout(Some(TIMEOUT))
        .and_then(|()| tcp.set_write_timeout(Some(TIMEOUT)))
        .map_err(|e| ImapError::Connect(e.to_string()))?;
    let config = rustls::ClientConfig::with_platform_verifier().map_err(|e| ImapError::Connect(e.to_string()))?;
    let name =
        rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| ImapError::Connect(e.to_string()))?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| ImapError::Connect(e.to_string()))?;
    Ok(rustls::StreamOwned::new(conn, tcp))
}

/// One account's sign-in.
#[derive(Clone)]
pub struct GmailImap {
    pub user: String,
    pub pass: String,
}

impl GmailImap {
    /// The account's address and its app password from the Keychain.
    pub fn from_keychain(account_id: &str, address: &str) -> Result<Self> {
        let pass = crate::keychain::get(&keychain_service(account_id)).map_err(|e| ImapError::Login(e.to_string()))?;
        Ok(GmailImap {
            user: address.to_string(),
            // Google shows app passwords in groups of four; the spaces are not part of it.
            pass: pass.split_whitespace().collect(),
        })
    }

    pub fn set_inbox(&self, message_id: &str, on: bool) -> Result<Vec<u32>> {
        let mut s = Session::start(connect(HOST, PORT)?, &self.user, &self.pass)?;
        let out = s.set_inbox(message_id, on);
        s.logout();
        out
    }

    /// Sign in and find All Mail (`keryx doctor`).
    pub fn check(&self) -> Result<String> {
        let mut s = Session::start(connect(HOST, PORT)?, &self.user, &self.pass)?;
        let out = s.all_mail();
        s.logout();
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};

    /// A Gmail that answers this client's commands, and remembers every line it was sent.
    struct Fake {
        out: VecDeque<u8>,
        pending: Vec<u8>,
        sent: Vec<String>,
        /// uid → labels.
        labels: BTreeMap<u32, Vec<String>>,
        /// Message-ID → uids.
        ids: BTreeMap<String, Vec<u32>>,
        all_mail: String,
        /// Gmail keeps the Inbox label whatever it is told.
        stubborn: bool,
    }

    impl Fake {
        fn new() -> Fake {
            let mut f = Fake {
                out: VecDeque::new(),
                pending: Vec::new(),
                sent: Vec::new(),
                labels: BTreeMap::new(),
                ids: BTreeMap::new(),
                all_mail: "[Gmail]/All Mail".into(),
                stubborn: false,
            };
            f.say("* OK Gimap ready");
            f
        }
        fn say(&mut self, l: &str) {
            self.out.extend(l.as_bytes());
            self.out.extend(b"\r\n");
        }
        fn answer(&mut self, line: &str) {
            self.sent.push(line.to_string());
            let (tag, cmd) = line.split_once(' ').unwrap();
            if let Some(rest) = cmd.strip_prefix("LOGIN ") {
                if rest.ends_with("\"good\"") {
                    self.say(&format!("{tag} OK alec authenticated (Success)"));
                } else {
                    self.say(&format!(
                        "{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)"
                    ));
                }
            } else if cmd.starts_with("LIST") {
                self.say(r#"* LIST (\HasNoChildren) "/" "INBOX""#);
                let all = self.all_mail.clone();
                self.say(&format!(r#"* LIST (\All \HasNoChildren) "/" "{all}""#));
                self.say(r#"* LIST (\HasNoChildren \Trash) "/" "[Gmail]/Bin""#);
                self.say(&format!("{tag} OK Success"));
            } else if cmd.starts_with("SELECT") {
                self.say("* 3 EXISTS");
                self.say(&format!("{tag} OK [READ-WRITE] selected. (Success)"));
            } else if let Some(q) = cmd.strip_prefix("UID SEARCH X-GM-RAW \"rfc822msgid:") {
                let id = q.trim_end_matches('"');
                let uids = self.ids.get(id).cloned().unwrap_or_default();
                let list: Vec<String> = uids.iter().map(u32::to_string).collect();
                self.say(format!("* SEARCH {}", list.join(" ")).trim_end());
                self.say(&format!("{tag} OK SEARCH completed (Success)"));
            } else if let Some(q) = cmd.strip_prefix("UID STORE ") {
                let (uid, op) = q.split_once(' ').unwrap();
                let uid: u32 = uid.parse().unwrap();
                let labels = self.labels.entry(uid).or_default();
                if op.starts_with('+') {
                    labels.push("\\\\Inbox".into());
                } else if !self.stubborn {
                    labels.retain(|l| l != "\\\\Inbox");
                }
                let shown: Vec<String> = labels.iter().map(|l| format!("\"{l}\"")).collect();
                let reply = format!("* 1 FETCH (X-GM-LABELS ({}) UID {uid})", shown.join(" "));
                self.say(&reply);
                self.say(&format!("{tag} OK Success"));
            } else if cmd == "LOGOUT" {
                self.say("* BYE LOGOUT Requested");
                self.say(&format!("{tag} OK 73 good day (Success)"));
            } else {
                self.say(&format!("{tag} BAD Unknown command"));
            }
        }
        fn inbox(&self, uid: u32) -> bool {
            self.labels
                .get(&uid)
                .is_some_and(|l| l.iter().any(|x| x == "\\\\Inbox"))
        }
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.out.len());
            for (i, b) in self.out.drain(..n).enumerate() {
                buf[i] = b;
            }
            Ok(n)
        }
    }

    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.pending.extend_from_slice(buf);
            while let Some(i) = self.pending.windows(2).position(|w| w == b"\r\n") {
                let line: Vec<u8> = self.pending.drain(..i + 2).collect();
                self.answer(String::from_utf8_lossy(&line).trim_end());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn gmail() -> Fake {
        let mut f = Fake::new();
        // The newsletter Mail had uploaded twice: both copies carry the Inbox label.
        f.ids.insert("abc@mail.example".into(), vec![10717, 10720]);
        f.labels.insert(10717, vec!["\\\\Inbox".into(), "\\\\Important".into()]);
        f.labels.insert(10720, vec!["\\\\Inbox".into()]);
        f
    }

    /// Run one archive or un-archive and hand back the fake to look at.
    fn run(mut f: Fake, id: &str, on: bool) -> (Result<Vec<u32>>, Fake) {
        // A new connection: a new greeting.
        f.out.clear();
        f.say("* OK Gimap ready");
        let mut s = Session::start(&mut f, "alec@example.com", "good").unwrap();
        let out = s.set_inbox(id, on);
        s.logout();
        (out, f)
    }

    #[test]
    fn archiving_takes_the_inbox_label_off_every_copy_and_back_puts_it_on() {
        let (out, f) = run(gmail(), "<abc@mail.example>", false);
        assert_eq!(out.unwrap(), vec![10717, 10720]);
        assert!(!f.inbox(10717) && !f.inbox(10720));
        assert!(
            f.labels[&10717].contains(&"\\\\Important".to_string()),
            "other labels kept"
        );
        let (out, f) = run(f, "abc@mail.example", true);
        assert_eq!(out.unwrap().len(), 2);
        assert!(f.inbox(10717) && f.inbox(10720));
    }

    #[test]
    fn it_never_sends_anything_but_label_changes_on_the_inbox() {
        let (_, f) = run(gmail(), "abc@mail.example", false);
        let (_, f) = run(f, "abc@mail.example", true);
        for line in &f.sent {
            let cmd = line.split_once(' ').unwrap().1;
            for bad in [
                "APPEND",
                "EXPUNGE",
                "\\Deleted",
                "MOVE",
                "COPY",
                "FLAGS",
                "CLOSE",
                "DELETE",
            ] {
                assert!(!cmd.to_uppercase().contains(&bad.to_uppercase()), "{line}");
            }
            if cmd.starts_with("UID STORE") {
                assert!(cmd.ends_with("X-GM-LABELS (\\Inbox)"), "{line}");
            }
        }
        // And there is no other command it could send.
        let every = [
            Command::Login {
                user: "a@b".into(),
                pass: "p".into(),
            },
            Command::List,
            Command::Select("[Gmail]/All Mail".into()),
            Command::Search("x@y".into()),
            Command::SetInbox { uid: 1, on: true },
            Command::SetInbox { uid: 1, on: false },
            Command::Logout,
        ];
        let allowed = [
            "LOGIN ",
            "LIST ",
            "SELECT ",
            "UID SEARCH X-GM-RAW ",
            "UID STORE ",
            "LOGOUT",
        ];
        for c in every {
            let r = c.render().unwrap();
            assert!(allowed.iter().any(|a| r.starts_with(a)), "{r}");
        }
    }

    #[test]
    fn all_mail_is_found_by_its_flag_not_its_name() {
        let mut f = gmail();
        f.all_mail = "[Google Mail]/All Mail".into();
        let (out, f) = run(f, "abc@mail.example", false);
        assert!(out.is_ok());
        assert!(
            f.sent.iter().any(|l| l.ends_with("SELECT \"[Google Mail]/All Mail\"")),
            "{:?}",
            f.sent
        );
    }

    #[test]
    fn a_wrong_password_a_missing_message_and_a_label_that_stays_are_errors() {
        let mut f = gmail();
        let err = Session::start(&mut f, "alec@example.com", "bad").err().unwrap();
        assert!(matches!(err, ImapError::Login(_)), "{err}");
        assert!(!f.sent.iter().any(|l| l.contains("\"bad\"") && !l.contains("LOGIN")));

        let (out, _) = run(gmail(), "nope@x", false);
        assert!(matches!(out, Err(ImapError::NotFound(_))), "{out:?}");

        let mut f = gmail();
        f.stubborn = true;
        let (out, _) = run(f, "abc@mail.example", false);
        assert!(
            out.unwrap_err()
                .to_string()
                .contains("still shows UID 10717 in the inbox")
        );
    }

    #[test]
    fn quoting_and_literals() {
        assert_eq!(quote(r#"a"b\c"#).unwrap(), r#""a\"b\\c""#);
        assert!(quote("a\r\nLOGOUT").is_err(), "no command smuggled in a value");
        assert_eq!(literal_len("* 1 FETCH (X-GM-LABELS ({5}"), Some(5));
        assert_eq!(literal_len("* OK done"), None);
        assert_eq!(
            last_name(r#"* LIST (\All \HasNoChildren) "/" "[Gmail]/All Mail""#).as_deref(),
            Some("[Gmail]/All Mail")
        );
        assert!(has_inbox(r#""\\Important" "\\Inbox""#));
        assert!(!has_inbox(r#""\\Important" Errands"#));
        // The password never reaches the log.
        let shown = Command::Login {
            user: "a@b".into(),
            pass: "secret".into(),
        }
        .shown();
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn a_reply_carrying_a_literal_is_read_whole() {
        let mut f = Fake::new();
        f.out.clear();
        f.out
            .extend(b"* 1 FETCH (X-GM-LABELS ({6}\r\n\xc3\x89tude \"\\\\Inbox\") UID 7)\r\nk1 OK Success\r\n");
        let mut s = Session {
            io: BufReader::new(&mut f),
            n: 0,
        };
        let got = s.exec(&Command::Logout).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(has_inbox(&fetched_labels(&got[0]).unwrap()));
    }
}
