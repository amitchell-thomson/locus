#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Reading and sending mail (cleanup C6, decision D4): the bar moves email by email, and what
//! is sent is what the confirm screen shows. Each edge case of the slice has a test here. The
//! daemon is the fake in `common`: nothing is ever sent.

mod common;
use common::*;
use keryx_core::changeset::{Author, Status};
use keryx_core::model::{Address, Attachment, Location, Message, thread_ref};
use keryx_store::db::Raw;
use keryx_tui::app::{App, Key, Mode};
use keryx_tui::render_to_string;
use keryx_tui::send::{Purpose, SendState};

fn addr(name: &str, email: &str) -> Address {
    Address::new(Some(name), email)
}

/// A message for the test mirror.
#[allow(clippy::too_many_arguments)]
fn msg(
    account: &str,
    thread: &str,
    id: &str,
    from: Address,
    to: Vec<Address>,
    date: i64,
    body: &str,
    read: bool,
) -> Message {
    Message {
        account: account.into(),
        source_id: format!("{account}-{id}"),
        message_id: id.into(),
        thread_id: thread.into(),
        from: Some(from),
        to,
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Summer project".into(),
        date,
        location: Location::Mailboxes {
            physical: "INBOX".into(),
            labels: vec![],
        },
        in_inbox: true,
        read,
        flagged: false,
        size: 100,
        attachments: vec![],
        body_text: body.into(),
        hidden_text_removed: false,
        snippet: String::new(),
        in_reply_to: vec![],
        references: vec![],
        list_headers: vec![],
    }
}

fn put(a: &App, m: &Message) {
    a.db.upsert(m, Raw::None, 1).unwrap();
}

fn me() -> Address {
    addr("Alec Mitchell-Thomson", "newc7370@ox.ac.uk")
}

fn milly() -> Address {
    addr("Milly Borthwick", "milly@example.org")
}

const PARA: &str = "Thanks for sending the plan over. I have read it twice now and I think the second \
half works well, but the first part needs a clearer statement of what we measure and when.";

/// Eight messages over a week, Milly and you in turn; her last four are new since you last read
/// the thread (11 Oct 12:00).
fn long_thread(a: &mut App) -> String {
    let base = ts(2026, 10, 7, 9);
    for k in 0..8i64 {
        let theirs = k % 2 == 0 || k >= 5;
        let (from, to) = if theirs {
            (milly(), vec![me()])
        } else {
            (me(), vec![milly()])
        };
        let body = format!("Message {k}. {PARA}\n\n{PARA}\n\nDoes Thursday at {k} work for you?");
        let m = msg(
            "oxford",
            "long",
            &format!("long-{k}@example.org"),
            from,
            to,
            base + k * 86_400,
            &body,
            k < 5,
        );
        put(a, &m);
    }
    a.reload();
    thread_ref("oxford", "long")
}

/// The thread-read file in a temp dir, with this thread last read at `t`.
fn read_mark(a: &mut App, dir: &tempfile::TempDir, r: &str, t: i64) {
    let p = dir.path().join("thread-read");
    std::fs::write(&p, format!("{r} {t}\n")).unwrap();
    a.thread_read_file = Some(p);
}

fn open(a: &mut App, r: &str) {
    let (acc, t) = keryx_core::model::parse_thread_ref(r).unwrap();
    assert!(a.open_thread(acc, t));
}

fn target_id(a: &App) -> String {
    let o = a.open.as_ref().unwrap();
    o.msgs[o.target().unwrap()].message_id.clone()
}

/// The frame row the body starts on in `long_thread` at these heights: the border, then the
/// seven lines of its header (subject, people, timeline, whose turn, three questions).
const BODY_TOP: usize = 8;

/// The first row of the drawn frame that carries the bar: the selected email's first line.
fn bar_row(s: &str) -> Option<usize> {
    s.lines().position(|l| l.contains('▌'))
}

/// The view's top line, as the last frame drew it.
fn top(a: &App) -> i32 {
    let o = a.open.as_ref().unwrap();
    i32::from(o.top_in(&o.blocks.borrow(), o.max_top.get()))
}

/// The selected email's block as last drawn.
fn sel_block(a: &App) -> keryx_tui::app::Block {
    let o = a.open.as_ref().unwrap();
    let t = o.target().unwrap();
    *o.blocks.borrow().iter().find(|b| b.msg == t).unwrap()
}

/// A frame, then a reload from the mirror and two more frames, as the run loop does between
/// keys; every frame must be the same. Returns it.
fn settled(a: &mut App, w: u16, h: u16) -> String {
    let s = render_to_string(a, w, h);
    a.reload();
    let s2 = render_to_string(a, w, h);
    let s3 = render_to_string(a, w, h);
    assert_eq!(s, s2, "a reload moved the view at {w}x{h}");
    assert_eq!(s2, s3, "a frame moved the view at {w}x{h}");
    s
}

#[test]
fn a_long_thread_opens_with_the_bar_on_its_newest_new_message() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    open(&mut a, &r);
    let o = a.open.as_ref().unwrap();
    // New: Milly's messages after 9 Oct 12:00 (5, 6, 7). Open: all three; the rest fold.
    let open_now: Vec<String> = o.first_open.iter().map(|&i| o.msgs[i].message_id.clone()).collect();
    assert_eq!(open_now.len(), 3, "{open_now:?}");
    let _ = render_to_string(&mut a, 90, 40);
    assert_eq!(
        target_id(&a),
        "long-7@example.org",
        "the newest new message, at the top"
    );
    // The whole thread on a tall screen: three new ones open, one divider, the rest a line each.
    let s = render_to_string(&mut a, 90, 84);
    insta::assert_snapshot!("reading_long_thread", s);
    assert_eq!(
        s.matches("read before").count(),
        1,
        "one divider between new and read: {s}"
    );
    assert!(s.contains("      you "), "folded lines name who, yours stepped in: {s}");
    // Older messages are one line each: who, when, first words.
    let folded = s
        .lines()
        .filter(|l| l.contains("Message 1.") || l.contains("Message 2."))
        .count();
    assert_eq!(folded, 2, "{s}");
    // ↓: the bar to the next email, the header (timeline and all) staying put.
    let _ = render_to_string(&mut a, 90, 24);
    keys(&mut a, &[Key::Down]);
    let s = render_to_string(&mut a, 90, 24);
    assert!(s.contains('●'), "the header stays full height: {s}");
    assert_eq!(target_id(&a), "long-6@example.org");
    assert_eq!(bar_row(&s), Some(BODY_TOP), "its first line at the top: {s}");
}

#[test]
fn enter_z_and_r_act_on_the_message_under_the_bar() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    a.drafts_dir = Some(dir.path().to_path_buf());
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 90, 30);
    // ] ] : two messages on, to Milly's 5.
    keys(&mut a, &[Key::Char(']'), Key::Char(']')]);
    let _ = render_to_string(&mut a, 90, 30);
    assert_eq!(target_id(&a), "long-5@example.org");
    // ⏎ folds the one under the bar, and only that one.
    let i = a.open.as_ref().unwrap().target().unwrap();
    keys(&mut a, &[Key::Enter]);
    let _ = render_to_string(&mut a, 90, 30);
    assert!(!a.open.as_ref().unwrap().is_open(i));
    assert_eq!(a.open.as_ref().unwrap().toggled.len(), 1);
    keys(&mut a, &[Key::Enter]);
    let _ = render_to_string(&mut a, 90, 30);
    // r answers it, says so, and keryxd is told which message (not the thread's latest).
    keys(&mut a, &[Key::Char('R')]);
    let said = a.message.clone().unwrap().0;
    assert!(said.starts_with("replying to Milly, "), "{said}");
    let req = a.editor_request.take().unwrap();
    match &req.purpose {
        Purpose::Reply { to: Some(t), r#ref, .. } => {
            assert_eq!(t.message_id, "long-5@example.org");
            assert_eq!(r#ref, &r);
        }
        p => panic!("{p:?}"),
    }
    assert!(
        req.text.contains("Message 5."),
        "the quote is of the message read: {}",
        req.text
    );
}

#[test]
fn the_question_in_an_older_message_is_answered_there_and_the_confirm_shows_the_real_quote() {
    // A thread that forks: answering an older message threads the reply to it.
    let (mut a, f) = app();
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    let r = select(&mut a, "Re: Group project plan");
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 80, 24);
    keys(&mut a, &[Key::Char(']')]);
    let _ = render_to_string(&mut a, 80, 24);
    assert_eq!(target_id(&a), "thread-a@example.com", "Jane's, the older one");
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    let typed = req.text.replacen("\n\n\n", "\n\nPart two is mine.\n", 1);
    a.editor_done(req.purpose, Ok(typed));
    let Mode::ConfirmSend(d) = &a.mode else {
        panic!("{:?}", a.mode)
    };
    assert_eq!(d.req.in_reply_to.as_deref(), Some("thread-a@example.com"));
    let s = render_to_string(&mut a, 80, 24);
    insta::assert_snapshot!("reading_confirm_reply_to_older", s);
    assert!(s.contains("replying to Jane, 12 Oct 09:00"), "{s}");
    assert!(
        s.contains("wrote:") && s.contains("│ Let's split the report"),
        "the quote that goes: {s}"
    );
    assert!(
        !s.contains("> Let's split"),
        "not the plain quote, which is not sent: {s}"
    );
    keys(&mut a, &[Key::Char('y')]);
    let call = f.take().remove(0);
    assert!(call.contains(r#"irt=Some("thread-a@example.com")"#), "{call}");
    assert_eq!(a.mode, Mode::Thread, "you stay in the thread");
}

#[test]
fn the_confirm_screen_wraps_its_warnings_and_scrolls() {
    let (mut a, _f) = app();
    a.cfg.live = false;
    let r = select(&mut a, "Lab partner for Thursday?");
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 60, 12);
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    let long = (0..30).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    a.editor_done(
        req.purpose,
        Ok(req.text.replacen("\n\n\n", &format!("\n\n{long}\n"), 1)),
    );
    let s = render_to_string(&mut a, 60, 12);
    assert!(
        s.contains("so this will") && s.contains("not be sent"),
        "wrapped, not cut: {s}"
    );
    assert!(!s.contains("line 29"), "{s}");
    keys(&mut a, &[Key::Bottom]);
    let s = render_to_string(&mut a, 60, 12);
    assert!(s.contains("line 29") || s.contains("Jane"), "scrolled to the end: {s}");
}

/// Milly writes to the society's shared address, which is in OQTS shared, and her first mail
/// is also in Oxford (you were cc'd at your Oxford address): one conversation in two accounts.
fn merged(a: &mut App) -> (String, String) {
    let shared = addr("OQTS", "oqts@oqts.org");
    let mut m1 = msg(
        "oxford",
        "mx",
        "m1@example.org",
        milly(),
        vec![shared.clone()],
        ts(2026, 10, 12, 10),
        "Can the society host the talk?",
        true,
    );
    m1.cc = vec![me()];
    put(a, &m1);
    let mut c1 = m1.clone();
    c1.account = "oqts-shared".into();
    c1.source_id = "shared-m1".into();
    c1.thread_id = "ms".into();
    put(a, &c1);
    let mut m2 = msg(
        "oqts-shared",
        "ms",
        "m2@example.org",
        milly(),
        vec![shared],
        ts(2026, 10, 13, 10),
        "Also: is the 20th free?",
        false,
    );
    m2.in_reply_to = vec!["m1@example.org".into()];
    m2.references = vec!["m1@example.org".into()];
    put(a, &m2);
    a.reload();
    (thread_ref("oxford", "mx"), thread_ref("oqts-shared", "ms"))
}

#[test]
fn a_merged_thread_shows_both_copies_once_and_r_answers_from_the_account_that_has_it() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    let (ox, shared) = merged(&mut a);
    open(&mut a, &ox);
    assert_eq!(a.open.as_ref().unwrap().msgs.len(), 2, "the copy counted once");
    let s = render_to_string(&mut a, 90, 20);
    insta::assert_snapshot!("reading_merged_thread", s);
    assert!(s.contains("is the 20th free?"), "the message only in OQTS shared: {s}");
    // The newest (only in OQTS shared, sent to its address): answered from there.
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    match &req.purpose {
        Purpose::Reply {
            r#ref,
            account,
            to: Some(t),
        } => {
            assert_eq!((r#ref.as_str(), account.as_str()), (shared.as_str(), "oqts-shared"));
            assert_eq!(t.message_id, "m2@example.org");
        }
        p => panic!("{p:?}"),
    }
    // The older one is in both: answered from the copy in the account it came to.
    keys(&mut a, &[Key::Char(']')]);
    let _ = render_to_string(&mut a, 90, 20);
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    match &req.purpose {
        Purpose::Reply {
            r#ref,
            account,
            to: Some(t),
        } => {
            assert_eq!(t.message_id, "m1@example.org");
            let (acc, _) = keryx_core::model::parse_thread_ref(r#ref).unwrap();
            assert_eq!(acc, account, "the thread is in the account it goes from");
        }
        p => panic!("{p:?}"),
    }
}

#[test]
fn an_account_without_a_copy_does_not_answer_and_the_line_says_which_does() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    a.drafts_dir = Some(dir.path().to_path_buf());
    // Sent to your OQTS address, but only the shared inbox has it.
    let m = msg(
        "oqts-shared",
        "solo",
        "solo@example.org",
        milly(),
        vec![addr("Alec", "alec@oqts.org")],
        ts(2026, 10, 13, 9),
        "Quick question?",
        false,
    );
    put(&a, &m);
    a.reload();
    open(&mut a, &thread_ref("oqts-shared", "solo"));
    let _ = render_to_string(&mut a, 90, 20);
    keys(&mut a, &[Key::Char('R')]);
    let said = a.message.clone().unwrap().0;
    assert!(said.contains("has no copy") && said.contains("OQTS shared"), "{said}");
    let req = a.editor_request.take().unwrap();
    assert!(matches!(&req.purpose, Purpose::Reply { account, .. } if account == "oqts-shared"));
}

#[test]
fn claudes_draft_says_so_and_r_opens_it_rather_than_starting_a_second() {
    let (mut a, f) = app();
    let lab = select(&mut a, "Lab partner for Thursday?");
    let (acct, thread) = keryx_core::model::parse_thread_ref(&lab).unwrap();
    let mut c = cs(Status::Pending);
    c.id = "cs-claude".into();
    c.author = Author::Claude;
    c.interactive = false;
    c.ops = vec![keryx_core::changeset::Op::Send(keryx_core::changeset::Outgoing {
        account: acct.into(),
        thread_id: Some(thread.into()),
        from: addr("Alec", "newc7370@ox.ac.uk"),
        to: vec![addr("Jane Doe", "jane.doe@example.com")],
        cc: vec![],
        bcc: vec![],
        subject: "Re: Lab partner for Thursday?".into(),
        body: "Thursday works for me.".into(),
        in_reply_to: None,
        references: vec![],
        message_id: "<c@example.org>".into(),
        attachments: vec![],
    })];
    a.db.save_changeset(&c).unwrap();
    a.reload();
    open(&mut a, &lab);
    let s = render_to_string(&mut a, 90, 22);
    insta::assert_snapshot!("reading_thread_with_a_draft", s);
    assert!(s.contains("Claude's draft, not sent · R opens it"), "{s}");
    assert!(!s.contains("g d opens it"), "the old hint, where d meant file: {s}");
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    assert!(
        matches!(&req.purpose, Purpose::EditDraft { id, .. } if id == "cs-claude"),
        "{:?}",
        req.purpose
    );
    assert!(req.text.contains("Thursday works for me."));
    assert!(f.take().is_empty());
}

#[test]
fn an_image_only_email_reads_and_quotes_as_having_no_text() {
    let (mut a, _f) = app();
    let mut m = msg(
        "oxford",
        "img",
        "img@example.org",
        milly(),
        vec![me()],
        ts(2026, 10, 13, 9),
        "",
        false,
    );
    m.attachments = vec![Attachment {
        id: 2,
        name: "poster.png".into(),
        mime: "image/png".into(),
        size: 40_000,
    }];
    put(&a, &m);
    a.reload();
    open(&mut a, &thread_ref("oxford", "img"));
    let s = render_to_string(&mut a, 80, 16);
    assert!(s.contains("(no text)") && s.contains("poster.png"), "{s}");
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    a.editor_done(req.purpose, Ok(req.text.replacen("\n\n\n", "\n\nLovely poster.\n", 1)));
    let s = render_to_string(&mut a, 80, 20);
    assert!(s.contains("has no text: 1 attachment"), "{s}");
}

#[test]
fn a_long_newsletter_pages_with_space_and_is_read_only_for_what_was_seen() {
    let (mut a, f) = app();
    let dir = tempfile::tempdir().unwrap();
    let body = (0..200)
        .map(|n| format!("Item {n}: something on sale."))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut m = msg(
        "oxford",
        "news",
        "news@example.org",
        addr("Deals", "deals@shop.example.com"),
        vec![me()],
        ts(2026, 10, 13, 9),
        &body,
        false,
    );
    m.list_headers = vec![("list-id".into(), "<deals.shop.example.com>".into())];
    put(&a, &m);
    a.reload();
    let r = thread_ref("oxford", "news");
    read_mark(&mut a, &dir, &r, ts(2026, 10, 1, 9));
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 80, 20);
    let page = a.open.as_ref().unwrap().page();
    keys(&mut a, &[Key::Char(' ')]);
    let _ = render_to_string(&mut a, 80, 20);
    assert_eq!(top(&a), page);
    // Opened by mistake and left: it was on screen, so it is read (one message, seen).
    keys(&mut a, &[Key::Esc]);
    assert_eq!(
        f.take(),
        vec![format!(r#"triage {{"action":"read","on":true}} ["{r}"]"#)]
    );
}

#[test]
fn leaving_before_a_new_message_was_on_screen_keeps_it_new() {
    let (mut a, f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    let mark = ts(2026, 10, 9, 12);
    read_mark(&mut a, &dir, &r, mark);
    open(&mut a, &r);
    // Fold everything open, so nothing new below the first is on a 6-line screen... then
    // leave: only the top was seen.
    let _ = render_to_string(&mut a, 90, 8);
    keys(&mut a, &[Key::Esc]);
    assert!(f.take().is_empty(), "not marked read in the mailbox");
    let said = a.message.clone().unwrap().0;
    assert!(said.contains("stay unread"), "{said}");
    let stamped = a.thread_read_at(&r).unwrap();
    assert!(stamped > mark, "what was seen moved the mark: {stamped}");
    // The unseen new messages are still new next time.
    open(&mut a, &r);
    let o = a.open.as_ref().unwrap();
    assert!((0..o.msgs.len()).any(|i| a.is_new(o, i)));
}

#[test]
fn a_forward_carries_the_message_under_the_bar() {
    let (mut a, _f) = app();
    let r = select(&mut a, "Re: Group project plan");
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 80, 24);
    keys(&mut a, &[Key::Char(']')]);
    let _ = render_to_string(&mut a, 80, 24);
    keys(&mut a, &[Key::Char('F')]);
    let req = a.editor_request.take().unwrap();
    assert!(matches!(req.purpose, Purpose::Forward { .. }));
    assert!(
        req.text.contains("Let's split the report"),
        "Jane's, not your reply: {}",
        req.text
    );
    assert!(!req.text.contains("I'll take part two"), "{}", req.text);
}

#[test]
fn reply_all_on_a_mailing_list_warns_that_everyone_on_it_gets_it() {
    let (mut a, _f) = app();
    let list = addr("Society", "members@lists.example.org");
    let mut m = msg(
        "oxford",
        "list",
        "list@example.org",
        milly(),
        vec![list],
        ts(2026, 10, 13, 9),
        "Who is coming on Friday?",
        false,
    );
    m.cc = vec![addr("Sam", "sam@example.com")];
    m.list_headers = vec![
        ("list-id".into(), "<members.lists.example.org>".into()),
        ("list-post".into(), "<mailto:members@lists.example.org>".into()),
    ];
    put(&a, &m);
    a.reload();
    open(&mut a, &thread_ref("oxford", "list"));
    let _ = render_to_string(&mut a, 90, 20);
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    a.editor_done(req.purpose, Ok(req.text.replacen("\n\n\n", "\n\nMe!\n", 1)));
    // R answers Milly; a on the confirm screen makes it reply all, and the warning comes with it.
    let Mode::ConfirmSend(d) = &a.mode else {
        panic!("{:?}", a.mode)
    };
    assert!(
        d.warnings.iter().all(|w| !w.contains("mailing list")),
        "{:?}",
        d.warnings
    );
    keys(&mut a, &[Key::Char('a')]);
    let Mode::ConfirmSend(d) = &a.mode else {
        panic!("{:?}", a.mode)
    };
    assert!(
        d.warnings
            .iter()
            .any(|w| w.contains("members@lists.example.org") && w.contains("mailing list")),
        "{:?}",
        d.warnings
    );
    let s = render_to_string(&mut a, 90, 24);
    assert!(s.contains("mailing list"), "{s}");
}

/// Reply to the lab thread and press y: the fake holds it as `cs-1`.
fn send_reply(a: &mut App, dir: &tempfile::TempDir) -> String {
    a.drafts_dir = Some(dir.path().to_path_buf());
    let r = select(a, "Lab partner for Thursday?");
    open(a, &r);
    let _ = render_to_string(a, 90, 24);
    keys(a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    let text = req.text.replacen("\n\n\n", "\n\nYes, 2pm works.\n", 1);
    // As the editor leaves it: the text kept as a draft until it has gone.
    std::fs::write(a.draft_path(&req.purpose).unwrap(), &text).unwrap();
    a.editor_done(req.purpose, Ok(text));
    a.key(Key::Char('y'));
    a.run_deferred();
    r
}

#[test]
fn after_y_you_stay_in_the_thread_and_c_cancels_the_send_in_its_hold() {
    let (mut a, f) = app();
    let dir = tempfile::tempdir().unwrap();
    send_reply(&mut a, &dir);
    assert!(f.take()[0].starts_with("send oxford"));
    assert_eq!(a.mode, Mode::Thread, "still reading the thread");
    a.hold = Some(("cs-1".into(), a.now.timestamp() + 8));
    let s = render_to_string(&mut a, 90, 24);
    assert!(
        s.contains("sending · c cancels · 8 s") && s.contains("Yes, 2pm works."),
        "{s}"
    );
    keys(&mut a, &[Key::Char('c')]);
    assert_eq!(f.take(), vec!["cancel cs-1"]);
    let s = render_to_string(&mut a, 90, 24);
    assert!(s.contains("cancelled: nothing sent"), "{s}");
    assert!(a.outbox.iter().all(|o| o.state == SendState::Cancelled));
}

#[test]
fn a_send_that_fails_after_its_hold_stays_said_until_you_take_it_up() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    send_reply(&mut a, &dir);
    // The hold ends and Mail refuses it: keryxd records the failure in the mirror.
    let mut c = cs(Status::Failed);
    c.error = Some("Mail: the account is offline".into());
    a.db.save_changeset(&c).unwrap();
    a.hold = None;
    a.settle_send();
    let s = render_to_string(&mut a, 100, 24);
    assert!(
        s.contains("not sent: Mail: the account is offline"),
        "in the thread: {s}"
    );
    assert!(s.lines().last().unwrap().contains("not sent"), "and the footer: {s}");
    // Still said after other keys, and after leaving the thread.
    keys(&mut a, &[Key::Char('j'), Key::Esc]);
    let s = render_to_string(&mut a, 100, 24);
    assert!(s.contains("✗ not sent"), "{s}");
    // r in its thread opens your kept words again, and the failure is dealt with.
    keys(&mut a, &[Key::Enter]);
    let _ = render_to_string(&mut a, 100, 24);
    keys(&mut a, &[Key::Char('R')]);
    let req = a.editor_request.take().unwrap();
    assert!(req.text.contains("Yes, 2pm works."), "{}", req.text);
    assert!(a.failed_send().is_none());
}

#[test]
fn a_send_that_goes_says_sent_in_its_thread() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    send_reply(&mut a, &dir);
    a.db.save_changeset(&cs(Status::Sent)).unwrap();
    a.hold = None;
    a.settle_send();
    assert_eq!(a.message.as_ref().unwrap().0, "sent");
    let s = render_to_string(&mut a, 90, 24);
    assert!(s.contains("── sent"), "{s}");
}

/// Where the bar is and where the frame shows it, for one email: the bar on `id`, its block's
/// first line at the view's top (or as near as the end of the thread allows), and that line
/// drawn on the frame's row for it.
fn assert_bar_on(a: &App, s: &str, id: &str, h: u16) {
    assert_eq!(target_id(a), id, "h {h}");
    let b = sel_block(a);
    let o = a.open.as_ref().unwrap();
    let max = i32::from(o.max_top.get());
    let first = o.order().first().copied() == o.target();
    let home = if first { 0 } else { b.start as i32 };
    assert_eq!(top(a), home.min(max), "h {h}: the view follows the bar\n{s}");
    // On the frame: the email's first line is the first row with the bar.
    let want = BODY_TOP + b.start - top(a) as usize;
    assert_eq!(bar_row(s), Some(want), "h {h}: its first line\n{s}");
    if !first && i32::try_from(b.start).unwrap() <= max {
        assert_eq!(want, BODY_TOP, "h {h}: at the top of the view\n{s}");
    }
}

/// Alec, 13 Nov, after the thread as one scroll snapped back twice: "just make the purple bar
/// move between thread emails". ↓ ↓ ↑ move the bar one email each, the view follows, and the
/// run loop's frames and reloads in between move nothing, at every height tried.
#[test]
fn down_and_up_move_the_bar_one_email_and_the_view_follows_at_every_height() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    for h in [20u16, 24, 30, 45, 60] {
        open(&mut a, &r);
        let s = settled(&mut a, 90, h);
        assert_bar_on(&a, &s, "long-7@example.org", h);
        assert_eq!(top(&a), 0, "h {h}: opened at the top of the thread");
        for (k, id) in [
            (Key::Down, "long-6@example.org"),
            (Key::Down, "long-5@example.org"),
            (Key::Up, "long-6@example.org"),
            (Key::Char('j'), "long-5@example.org"),
            (Key::Char('j'), "long-4@example.org"),
            (Key::Char('k'), "long-5@example.org"),
            (Key::Char(']'), "long-4@example.org"),
            (Key::Char('['), "long-5@example.org"),
        ] {
            keys(&mut a, &[k]);
            let s = settled(&mut a, 90, h);
            assert_bar_on(&a, &s, id, h);
        }
        // ↓ to the oldest, then once more: it stays.
        for _ in 0..10 {
            keys(&mut a, &[Key::Down]);
        }
        let s = settled(&mut a, 90, h);
        assert_bar_on(&a, &s, "long-0@example.org", h);
        // ⇧↑ back to the newest.
        keys(&mut a, &[Key::Top]);
        let s = settled(&mut a, 90, h);
        assert_bar_on(&a, &s, "long-7@example.org", h);
        keys(&mut a, &[Key::Esc]);
    }
}

/// A long newest email over a short older one.
fn long_email(a: &mut App) -> String {
    let body = (0..60)
        .map(|n| format!("Line {n} of the long one."))
        .collect::<Vec<_>>()
        .join("\n\n");
    put(
        a,
        &msg(
            "oxford",
            "pg",
            "pg-old@example.org",
            milly(),
            vec![me()],
            ts(2026, 10, 12, 9),
            "The short older one.",
            true,
        ),
    );
    put(
        a,
        &msg(
            "oxford",
            "pg",
            "pg-new@example.org",
            milly(),
            vec![me()],
            ts(2026, 10, 13, 9),
            &body,
            false,
        ),
    );
    a.reload();
    thread_ref("oxford", "pg")
}

/// space pages inside the selected email and never back; once its end is on screen, the next
/// space moves the bar on. PgUp pages back up it.
#[test]
fn space_pages_a_long_email_then_moves_the_bar_on_and_never_back() {
    let (mut a, _f) = app();
    let r = long_email(&mut a);
    for h in [20u16, 30, 45] {
        open(&mut a, &r);
        let _ = settled(&mut a, 90, h);
        assert_eq!(target_id(&a), "pg-new@example.org");
        let page = a.open.as_ref().unwrap().page();
        let mut last_top = top(&a);
        let mut moved_on = false;
        for n in 0..40 {
            keys(&mut a, &[Key::Char(' ')]);
            let s = settled(&mut a, 90, h);
            let t = top(&a);
            assert!(t >= last_top, "h {h}, space {n}: the view went back up\n{s}");
            assert!(t - last_top <= page, "h {h}, space {n}: more than a page");
            if target_id(&a) == "pg-old@example.org" {
                moved_on = true;
                // The end of the long one was on screen before the bar moved.
                break;
            }
            let b = sel_block(&a);
            assert!(t <= b.end as i32, "h {h}: paged past the end of the email");
            last_top = t;
        }
        assert!(moved_on, "h {h}: space reached the next email");
        // And on: at the last email, space does nothing more.
        keys(&mut a, &[Key::Char(' '), Key::Char(' ')]);
        let _ = settled(&mut a, 90, h);
        assert_eq!(target_id(&a), "pg-old@example.org");
        // PgUp: back to the long one, at its last page, then up it a page at a time to its top.
        keys(&mut a, &[Key::PageUp]);
        let _ = settled(&mut a, 90, h);
        assert_eq!(target_id(&a), "pg-new@example.org");
        let mut prev = top(&a);
        assert!(prev > 0, "h {h}: its last page");
        for _ in 0..40 {
            keys(&mut a, &[Key::PageUp]);
            let _ = settled(&mut a, 90, h);
            assert!(top(&a) <= prev);
            prev = top(&a);
            if prev == 0 {
                break;
            }
        }
        assert_eq!((prev, target_id(&a).as_str()), (0, "pg-new@example.org"));
        keys(&mut a, &[Key::Esc]);
    }
}

#[test]
fn new_mail_in_the_thread_shows_above_without_moving_the_bar_or_the_view() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    // The bar on the newest (the top of the thread) and on an older one.
    for downs in [0usize, 1] {
        open(&mut a, &r);
        let _ = settled(&mut a, 90, 30);
        for _ in 0..downs {
            keys(&mut a, &[Key::Down]);
        }
        let shown = settled(&mut a, 90, 30);
        let at = target_id(&a);
        let n = format!("long-8-{downs}@example.org");
        let m = msg(
            "oxford",
            "long",
            &n,
            milly(),
            vec![me()],
            ts(2026, 10, 14, 14 + downs as u32),
            "Message 8. One more thing about Thursday.",
            false,
        );
        put(&a, &m);
        a.reload();
        let s = settled(&mut a, 90, 30);
        assert_eq!(target_id(&a), at, "the bar did not move");
        // The email under the bar is on the same row, and everything below it the same. (The
        // day band above the newest goes when the new one came the same day.)
        let row = bar_row(&shown).unwrap();
        assert_eq!(bar_row(&s), Some(row), "the view did not move:\n{shown}\n{s}");
        let body = |s: &str| {
            s.lines()
                .skip(row)
                .take(26 - row)
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(body(&s), body(&shown), "the view did not move:\n{shown}\n{s}");
        assert!(s.contains("1 new message in this thread, above"), "{s}");
        // ⇧↑ goes to it, open.
        keys(&mut a, &[Key::Top]);
        let s = settled(&mut a, 90, 30);
        assert!(s.contains("Message 8. One more thing"), "{s}");
        assert_eq!(target_id(&a), n);
        keys(&mut a, &[Key::Esc]);
    }
}

#[test]
fn a_resize_keeps_the_bar_and_its_email_at_the_top() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 90, 30);
    keys(&mut a, &[Key::Down]);
    for (w, h) in [(140, 30), (70, 30), (90, 20), (90, 45), (90, 30)] {
        let s = settled(&mut a, w, h);
        assert_eq!(target_id(&a), "long-6@example.org", "{w}x{h}");
        assert_eq!(bar_row(&s), Some(BODY_TOP), "{w}x{h}: its first line at the top\n{s}");
    }
    keys(&mut a, &[Key::Down]);
    let s = settled(&mut a, 90, 30);
    assert_bar_on(&a, &s, "long-5@example.org", 30);
}

/// Seen, for read on leaving: the emails the bar was on, and those wholly on screen; not one
/// that was only partly in view.
#[test]
fn read_is_stamped_for_what_the_bar_was_on() {
    let (mut a, f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    let mark = ts(2026, 10, 9, 12);
    read_mark(&mut a, &dir, &r, mark);
    // New: 7, 6, 5 (open) and 4 (folded). The bar on 7 and 6; 5 only partly on screen.
    open(&mut a, &r);
    let _ = settled(&mut a, 90, 20);
    keys(&mut a, &[Key::Down]);
    let _ = settled(&mut a, 90, 20);
    keys(&mut a, &[Key::Esc]);
    assert!(f.take().is_empty(), "two new ones not seen: not read in the mailbox");
    let unseen = a.message.clone().unwrap().0;
    assert!(unseen.starts_with("2 new messages"), "{unseen}");
    // The bar on every new one: read, in the mailbox too.
    open(&mut a, &r);
    let _ = settled(&mut a, 90, 20);
    for _ in 0..3 {
        keys(&mut a, &[Key::Down]);
        let _ = settled(&mut a, 90, 20);
    }
    keys(&mut a, &[Key::Esc]);
    assert_eq!(
        f.take(),
        vec![format!(r#"triage {{"action":"read","on":true}} ["{r}"]"#)]
    );
}
