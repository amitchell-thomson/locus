# keryx joins locus: one app, then a mail view you would rather use than Mail

**State:** approved 2026-10-26, in progress · **Written:** 2026-10-26 · **Runs in:** `locus slice 1`
**Decision:** [ADR-008](../../decisions/ADR-008-kairos-joins-locus.md) ("keryx will later too").
**From:** your words, 26 Oct: "lets begin refining it to something I actually find easier and
better to use than mail". **Builds on:** [kairos-joins-locus.md](kairos-joins-locus.md) and
[kairos-as-a-locus-app.md](kairos-as-a-locus-app.md), whose steps this follows for keryx.

**Done when:** keryx is built, tested and deployed from this repository and looks and behaves like
locus (one theme, one set of keys, one approve place, the taxonomy and registry); triage acts on
your real mail; the inbox holds only what is new, everything else filed into the taxonomy and
shown on that item's page; the Mail view is calm and readable (clean message text, a quiet list,
what came since you last looked); you write and send mail inside it, in a Neovim editor, with the
list still beside you; and Claude's work on your mail (filing, what needs a reply, drafts, dates
and tasks) waits in Approve.

Underneath that: nothing is ever deleted (keryx archives, it never trashes), Claude never sends,
and mail stays untrusted data. Those three rules from keryx's own spec become rules of locus.

---

## Where keryx is today (26 Oct)

- **It reads, and only reads.** All four accounts are mirrored from Mac Mail (Personal 3,382
  messages, Oxford 2,865, OQTS 61, OQTS shared 873; every mailbox count matches Mail), but the
  config has `live = false`. So `d` archive, `m` file and the rest only ever act on `[keryx-test]`
  mail. Real triage still happens in Mail.
- **Claude does nothing with it yet.** The scheduled runs that would mark what needs a reply,
  draft replies and propose cleanups are built but not installed.
- **The inbox is a firehose.** 1,065 threads sit in the inboxes (578 of them in OQTS shared), and
  about 210 messages arrive a week. Over half come from mailing lists (newsletters, Canvas,
  LinkedIn, DMARC reports, GitHub bots), interleaved by date with the few from people.
- **Reading is noisy.** A newsletter shows rows of invisible preheader characters and long
  tracking links in the body. The account column cuts "OQTS shared" to "OQTS sh…".
- **It looks like an older kairos**: its own copy of kairos's theme, a powerline-style bar,
  `A` for its own approvals, phone pages and ntfy pushes (off), `x` for "mark" where locus now
  uses `x` to remove.
- It lives in its own repository (`amitchell-thomson/keryx`, 24 commits, 27k lines of Rust), and
  keryxd runs on the Mac from launchd, because it reads Mac Mail's store and acts through Mail.

---

## Round 1 with you (26 Oct)

"I do not like the view you are proposing, I like having the new messages on the right, but it is
still a little bit noisy, and just too dense." What you asked for, and where it is in the steps:

| You said | In this spec |
|---|---|
| keep the list on the left and the message on the right; less noisy, less dense | the layout below; X5 |
| "edit and send messages in a neovim style view within keryx itself, where I can still see the left bar" | compose in the right pane, X6 |
| "hide and show the left bar" | `b`, X5 |
| Tab to shift between inboxes, every inbox's name in the header, the one shown highlighted, the others greyed | the header tabs, X5 |
| "how we get rid of all of the rubbish characters / broken formatting / links" | X3, answered below |
| "how would I easily add attachments / change subject / etc... all the usual functions" | X6, answered below |
| "a specific new since I last was here" | a rule in the list, X5 |
| "mail in my inbox to be only new things, and everything else gets filed into the taxonomy" | the inbox rule, X7 |

**The view** (the mockup is narrowed to fit here; at your window size the list is about a third):

```
╭ mail  all · personal · oxford · oqts · oqts shared ──────────────────────────── 3 new ╮
│ today                          │ an applicant                          00:27 · oqts   │
│ ● an applicant          00:27  │ Following up on an event                             │
│   Following up on an event     │                                                      │
│ ─ new since sat 22:10 ──────── │ Hello,                                               │
│ ● a recruiter           sun    │                                                      │
│   Re: summer internship        │ A short example of how a message reads, its          │
│   a society             sun    │ paragraphs kept and its one link turned into         │
│   A competition invitation     │ a number. [1]                                        │
│                                │                                                      │
│ friday                         │ … 2 earlier messages · z show                        │
│   a committee           fri    │                                                      │
│   This week's agenda           │ [1] example.org                                      │
╰ ⏎ open · r reply · d done · m file · b list · ? keys ────────────────────────────────╯
```

- Two quiet lines a thread: who, then the subject; no snippet in the list (the message is on the
  right). Unread is a dot, not bold everywhere. Days as headings, white space between them.
- **New since you were last here**: a rule in the list at the moment you last left the Mail view,
  and the count in the header.
- **The header names every inbox**; `Tab` (and `⇧Tab`) moves the highlight, and the list follows.
  The highlighted one is the inbox shown; the others are there, greyed.
- `b` hides the list (the message takes the width) and shows it again.

**Writing** (`r`, `R`, `+` new, `F` forward), the list still beside it:

```
╭ mail  all · personal · oxford · oqts · oqts shared ───────────────────── writing a reply ╮
│ ● an applicant          00:27  │ From:    oqts                                          │
│   Following up on an event     │ To:      an applicant                                  │
│ ● a recruiter           sun    │ Cc:                                                    │
│   Re: summer internship        │ Subject: Re: Following up on an event                  │
│                                │ Attach:  ~/Documents/example.pdf  (212 kB)             │
│                                │ ────────────────────────────────────────────────────── │
│                                │ Hi,                                                    │
│                                │                                                        │
│                                │ Thanks for getting in touch. █                         │
│                                │                                                        │
│                                │ -- Alec                                                │
│                                │ ┄ the quote below is kept ┄                            │
╰ -- INSERT --        :w send · :q cancel · ^a attach · ^t to · ? keys ──────────────────╯
```

It is your Neovim, with your config, running inside the right-hand pane (X-g). The header block
is part of the text: change the subject by editing the `Subject:` line, add people on `To:` and
`Cc:` (with address completion from your mail, already written for Neovim), switch the account on
`From:`. An attachment is an `Attach:` line, with file-path completion as you type; `^a` opens a
file picker that adds the line for you. `:w` asks "send to an applicant from oqts? y", then the
10-second hold you can undo with `u`; `:q` cancels and keeps a draft.

**Cleaning the text of a mail** (X3), on your own newsletters before and after:
- **Invisible characters** go: the preheader padding newsletters use (U+034F, zero-width spaces
  and joiners, U+FEFF, soft hyphens) and anything a terminal would misdraw.
- **The best part of the mail is used**: its plain-text version when it is a real one, else the
  HTML turned into text that keeps paragraphs, headings and lists, never one long run.
- **Links become numbers**: `[1]` in the text and the site name in a list at the end
  (`[1] example.org`); tracking redirects are unwrapped to where they really go; image-only links,
  "view this email in your browser" and the unsubscribe and postal-address footer fold away under
  one line. `⏎` on a number opens it; `y` copies it.
- Quoted earlier messages stay folded (`z` shows them), as now.

**The inbox holds only what is new** (X7). Once you have read a thread and dealt with it, `d`
(done) files it for what it is for, with keryx's guess already chosen (`B14`, `Millennium`,
`Errands`); `⏎` takes the guess, typing picks another. Automated mail never needs to reach you:
receipts, notifications and newsletters are filed for their item as they arrive (a GitHub mail for
`quast` to QUAST; a Canvas mail for B14 to B14; a receipt to Errands), and a person's mail is
never moved without you. Today's 1,065 inbox threads are cleared once: Claude proposes where each
goes, grouped, in Approve, and you accept a group at a time.

---

## Decisions only you can make

**Answered 26 Oct 2026:** X-a, X-c, X-e, X-f, X-g as recommended ("agree with my real neovim, its
colors match the theme"). X-b: folders for the areas and Personal's streams only, as the calendar
has them ("I only need the same high level ones as calendar, ie the areas, and then personal's
threads"): Studies, Projects, Careers, Errands, Sport, Social, Family; the item a thread is for
(B14, Millennium) is kept by keryx and shows on its page. X-d: "Automated mail filed away straight
away." Approve gets a redesign in the design sessions (added to slice-1.md). Reading a thread in
order: "unless you have a great idea now, in that case just implement it and I will review"; see
X5.

| # | Question | Options | Recommendation |
|---|---|---|---|
| X-a | Acting on real mail | a. `live = true`, after a round trip on `[keryx-test]` mail from each account (archive, file, reply, undo, seen in Mail and on the phone within seconds) · b. stay read-only | **a.** Without it nothing you do in the Mail view sticks. |
| X-b | Where filed mail goes on the servers | a. folders that mirror the taxonomy (`Studies/B14`, `Careers/Millennium`, `Personal/Errands`), made on each account when first needed · b. your existing folders only (Admin, Careers, Travel, …), each item mapped onto one | **a.** Mail on the phone and in Mail is then filed exactly as in locus, and an item's mail is one folder. keryx's spec ruled out creating folders; this lifts that, for these folders only. |
| X-c | Claude's part | a. a locus background job, inside the budget (plan E): the one-off clear-out of today's inbox, filing guesses for automated mail, what needs a reply and why, drafts for the clear-cut ones, dates and tasks; all of it in locus's Approve · b. keryx's own scheduled runs and review screen | **a.** One approve place and one budget. |
| X-d | Automated mail as it arrives | a. filed for its item at once, low risk and undoable, listed in the automatic log (`l`) · b. proposed in Approve like everything else | **a.** "Inbox only new things": automated mail is never new to you, and approving each one would make Approve the new inbox. |
| X-e | Marking several threads | a. `v` marks (as in visual mode), since `x` now removes everywhere · b. `space` | **a.** |
| X-f | The old keryx repository | a. archived once this is done · b. kept | **a.** |
| X-g | The editor | a. your real Neovim, run inside the right pane (a terminal inside keryx), so your config, motions and completion all work · b. a Vim-like editor written into keryx (the common motions only) | **a.** Anything less than your Neovim will feel wrong within a day; the address-completion source for it already exists. |

---

## Rules
- **Nothing that works stops.** Each step leaves keryx building, its tests green and keryxd
  running; your mail is touched only after X-a, and only by your own keys, by what you approve, or
  (per X-d) by filing automated mail, each undoable and logged.
- **Never delete. Claude never sends. Mail is untrusted.** keryx's rules, unchanged: no trash or
  expunge path; a draft Claude writes waits for you and sends only after your yes, with the 10 s
  hold; mail text reaches Claude only inside keryx's marked envelopes, and never reaches a
  terminal raw.
- **A person's mail is yours.** Nothing moves mail from a person without you, as now.
- **One writer each.** keryxd is the only writer to mail and to its mirror; kairosd stays the
  only writer to the calendar, and keryx reaches it through kairosd's API as locus does.
- Each step is a commit with tests and clippy green; anything about how it looks or reads is
  decided with you at the screen.

---

## Steps

### X1 Move in
- keryx into `keryx/` with its history (git subtree), its crates members of the one workspace;
  `eco-dates` from `crates/` (no more pinned rev); `scripts/ci.sh` gates it; keryxd is built from
  this repository and its launchd agent installed from `deploy/keryx/`.
- **Check:** `scripts/ci.sh` green from the root; `keryx doctor` all good from the new build.

### X2 One app (as kairos-as-a-locus-app did for kairos)
- locus-theme for the look; the shared keys (`d` done, `v` marks per X-e, `+` writes a new mail,
  `x` gone); "approve" not "approvals"; filing keys become For (the same type-ahead as kairos).
- One approve place: drafts waiting to send and filing proposals appear in locus's Approve, and
  keryx's `A` opens it. The phone pages and ntfy pushes go (plan B.4).
- One contract: what locus shows of mail (Home's mail line, an application's timeline, a course's
  page) comes from keryx as locus-contract types; the date finder reads keryx through it.
- **Check:** the shared-key test passes for keryx; locus's Approve shows a `[keryx-test]` draft
  and sends it on `y`.

### X3 Clean text
- As described above: invisible characters, the best part of the mail, links as numbers with
  tracking unwrapped, footers folded.
- **Check:** ten of your real newsletters and notices, before and after, as dumps (kept out of
  the repository).

### X4 Live (per X-a)
- The round trips on `[keryx-test]` mail from each account, then `live = true`.
- **Check:** each seen in Mail and on the phone, and undone with `u`.

### X5 The view
- The layout above: two quiet lines a thread, days as headings, the new-since rule, the header
  tabs with `Tab`/`⇧Tab`, `b` for the list.
- **Check:** dumps at 95 × 38 and 155 × 43 on your inbox, for you to judge at the screen.

### X6 Writing (per X-g)
- Neovim in the right pane for reply, reply all, forward and new mail; the header block as above;
  `^a` attachments; `:w` to send with the confirm and the hold; `:q` keeps a draft.
- **Check:** a reply with an attachment and a changed subject sent to yourself from each
  account, read on the phone with the attachment intact.

### X7 The inbox holds only what is new (per X-b, X-d)
- `d` files for what it is for, the guess chosen; automated mail filed as it arrives; the
  folders made on first use; filed mail on its item's page (an application's timeline, a course).
- The one-off clear-out (per X-c): Claude's proposals for today's inbox, grouped, in Approve.
- **Check:** the inbox after the clear-out, counted; a week later, what reached it and what was
  filed by itself.

### X8 Claude on your mail (per X-c)
- The locus job: what needs a reply and why, drafts for the clear-cut ones, dates and tasks found
  in mail, into Approve.
- **What mail is for, inferred** (your note of 26 Oct: "in the future I will want a locus job to
  infer what emails are for itself, and leave any unsure ones for me to do"): the job marks
  threads for their item (`m`'s record, nothing moves), only where it is sure; the unsure ones
  stay unmarked for you. Its marks are listed like the automatic filings, and undoable.
- **Check:** one real run, its items in Approve, what it cost.

### X9 Rounds with you
Each change after X5 is reviewed in use; every round recorded in slice-1.md.

## Order
X1, X2 and X3 (no change to your mail), then X4 with you, then X5 and X6, then X7 and X8, then the
rounds. X8 and the clear-out wait for Slice 0's budget logging if that is not in yet.

## Rounds after X7 (26 Oct, with you at the screen)

- **Round 2:** attachments open (a wrong path, not Mail); `^o` checks a file in the attach
  picker; reading a thread is magenta throughout; days ruled; a real address dropdown in
  Neovim; the cursor follows Neovim's mode; your own addresses shown.
- **Round 3:** arrows over a thread's attachments; Oxford's `From:` shows your college address
  while Mail sends as before.
- **Round 4:** the inbox holds what is new, needs action or is needed soon, and **what a thread is
  for is separate from being done with it**: `m` marks what it is for and it stays in the inbox
  (its tag on the row, on its item's page at once, keryx's own record); `d` files it for its mark,
  or asks. **Needs reply is the one attention marker** (`f`, a peach bar, first in its day);
  keryx no longer flags. Go-to lost Flagged. An item's page lists its inbox threads first.

## Report
*(when the steps are done)*

**Interim, 26 Oct 2026.** Done: X1, X2 (its check, a `[keryx-test]` draft sent from Approve,
waits for X4), X3, X5, X6 and X7 apart from the clear-out. Waiting: X4 with you (the round
trips, then `live = true`; X6's and X7's live checks ride on it); X8 and the clear-out, for
Slice 0's budget logging (0.7, not built yet). Notes:

- **X5, reading in order.** No redesign of its own: the conversation reads top to bottom
  with a colour bar per writer (you in blue) and earlier messages cut to three lines until
  `z`. For you to judge at the screen, as the answer asked.
- **X6.** Your Neovim runs on a pty in the pane (portable-pty, vt100, tui-term) and draws
  your LazyVim in about 200 ms. `:w` sends, `:q` keeps a draft, `:cq` throws it away; each
  was driven through real Neovim before committing. Address completion comes through
  blink.cmp, registered for that buffer only, since your config has no keryx snippet and is
  never edited.
- **X7.** The folder is the item's area, or for Personal its stream. keryx makes only those
  folders, on first use (SPEC §6.2 now says so). The guess was read over your real inbox
  (1,077 threads): it answered 316, and where it answered it was right except that bounce
  notices counted as errands (30 of 85), now excluded. Filing on arrival started at 13:52
  (its mark) and touches only `[keryx-test]` mail until live. Filed mail shows on its item's
  page: New for an unread thread, and every thread on an application's timeline.
