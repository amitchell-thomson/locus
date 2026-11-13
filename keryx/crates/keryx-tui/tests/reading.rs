#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Reading and sending mail (cleanup C6, decision D4): a thread reads like a document, and what
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

#[test]
fn a_long_thread_opens_on_its_newest_new_message_and_reads_as_one_scroll() {
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
    // A page: the header shrinks to two lines (no timeline) and the bar follows the top line.
    let _ = render_to_string(&mut a, 90, 24);
    keys(&mut a, &[Key::Char(' ')]);
    let s = render_to_string(&mut a, 90, 24);
    assert!(!s.contains('●'), "the timeline goes while you scroll: {s}");
    assert!(s.contains('▌'), "the bar marks the message at the top: {s}");
    // Space runs on into the next message: one scroll, no jumping back.
    let before = a.open.as_ref().unwrap().scroll.get();
    let first = target_id(&a);
    for _ in 0..3 {
        keys(&mut a, &[Key::Char(' ')]);
        let _ = render_to_string(&mut a, 90, 24);
    }
    assert!(a.open.as_ref().unwrap().scroll.get() > before);
    assert_ne!(target_id(&a), first, "the bar moved on with the text");
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
    assert_eq!(i32::from(a.open.as_ref().unwrap().scroll.get()), page);
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

fn scroll(a: &App) -> u16 {
    a.open.as_ref().unwrap().scroll.get()
}

/// The message under the top line and how far into it (the view's anchor).
fn place(a: &App) -> (String, usize) {
    let o = a.open.as_ref().unwrap();
    let (i, off) = o.anchor.get().unwrap();
    (o.msgs[i].message_id.clone(), off)
}

/// Alec, 13 Nov: "the thread scrolling does not work properly, it is stuck at the top and
/// pressing down arrow goes down one line and then jumps you back up". How far a thread scrolls
/// was reckoned under the full header and the frame drawn under the two-line one, so a thread a
/// few lines too long for the pane was clamped back to the top by the next frame. Key presses
/// alone never showed it: the run loop draws between keys (every half second, and whenever
/// keryxd writes the mirror), so this drives frames and reloads between them, at every height.
#[test]
fn down_holds_through_frames_and_refreshes_at_every_height() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    let mut tried = 0;
    for h in 8..90u16 {
        open(&mut a, &r);
        let _ = render_to_string(&mut a, 90, h);
        if a.open.as_ref().unwrap().max_scroll.get() == 0 {
            // It fits under its header: nothing to scroll, and ↓ moves nothing.
            keys(&mut a, &[Key::Down]);
            let _ = render_to_string(&mut a, 90, h);
            assert_eq!(scroll(&a), 0, "h {h}");
            continue;
        }
        tried += 1;
        let start = scroll(&a);
        for step in 1..=3u16 {
            keys(&mut a, &[Key::Down]);
            for _ in 0..3 {
                let _ = render_to_string(&mut a, 90, h);
                a.reload();
            }
            let max = a.open.as_ref().unwrap().max_scroll.get();
            assert_eq!(scroll(&a), (start + step).min(max), "h {h}, ↓ {step}: put back");
            assert!(scroll(&a) > 0, "h {h}");
        }
        keys(&mut a, &[Key::Char('j'), Key::Char(' ')]);
        let at = scroll(&a);
        let _ = render_to_string(&mut a, 90, h);
        a.reload();
        let _ = render_to_string(&mut a, 90, h);
        assert_eq!(scroll(&a), at, "h {h}: j and space hold too");
        keys(&mut a, &[Key::Esc]);
    }
    assert!(tried > 30, "{tried}");
}

#[test]
fn new_mail_in_the_thread_shows_above_and_the_view_stays_where_you_were_reading() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 90, 30);
    keys(&mut a, &[Key::Char(']')]);
    let _ = render_to_string(&mut a, 90, 30);
    keys(&mut a, &[Key::Down, Key::Down, Key::Down]);
    let _ = render_to_string(&mut a, 90, 30);
    let before = place(&a);
    assert_eq!(before.0, "long-6@example.org");
    let shown = render_to_string(&mut a, 90, 30);
    // Milly writes again while you read: keryxd puts it in the mirror and the loop reloads.
    let m = msg(
        "oxford",
        "long",
        "long-8@example.org",
        milly(),
        vec![me()],
        ts(2026, 10, 14, 14),
        "Message 8. One more thing about Thursday.",
        false,
    );
    put(&a, &m);
    a.reload();
    let s = render_to_string(&mut a, 90, 30);
    assert_eq!(a.open.as_ref().unwrap().msgs.len(), 9, "the thread has it");
    assert_eq!(place(&a), before, "the same line of the same message at the top");
    assert_eq!(target_id(&a), "long-6@example.org", "the bar did not move");
    let body = |s: &str| s.lines().skip(3).take(20).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(body(&s), body(&shown), "the screen did not move:\n{shown}\n{s}");
    assert!(s.contains("1 new message in this thread, above"), "{s}");
    // Frames and reloads with nothing new keep it there.
    for _ in 0..3 {
        a.reload();
        let _ = render_to_string(&mut a, 90, 30);
    }
    assert_eq!(place(&a), before);
    // ⇧↑ goes to it, open.
    keys(&mut a, &[Key::Top]);
    let s = render_to_string(&mut a, 90, 30);
    assert!(s.contains("Message 8. One more thing"), "{s}");
    assert_eq!(target_id(&a), "long-8@example.org");
}

#[test]
fn a_resize_keeps_the_message_you_were_reading_at_the_top() {
    let (mut a, _f) = app();
    let dir = tempfile::tempdir().unwrap();
    let r = long_thread(&mut a);
    read_mark(&mut a, &dir, &r, ts(2026, 10, 9, 12));
    open(&mut a, &r);
    let _ = render_to_string(&mut a, 90, 30);
    keys(&mut a, &[Key::Char(']')]);
    let _ = render_to_string(&mut a, 90, 30);
    keys(&mut a, &[Key::Down, Key::Down]);
    let _ = render_to_string(&mut a, 90, 30);
    let before = place(&a);
    // Wider, then narrower, then shorter and taller: the lines rewrap and the view stays.
    for (w, h) in [(140, 30), (70, 30), (90, 18), (90, 50), (90, 30)] {
        let _ = render_to_string(&mut a, w, h);
        let _ = render_to_string(&mut a, w, h);
        assert_eq!(place(&a), before, "{w}x{h}");
        assert_eq!(target_id(&a), "long-6@example.org", "{w}x{h}");
    }
    // And scrolling goes on from there.
    keys(&mut a, &[Key::Down]);
    let _ = render_to_string(&mut a, 90, 30);
    assert_eq!(place(&a), (before.0.clone(), before.1 + 1));
}
