# keryx joins locus: one app, then a mail view you would rather use than Mail

**State:** proposed, waiting for your approval · **Written:** 2026-10-26 · **Runs in:** `locus slice 1`
**Decision:** [ADR-008](../../decisions/ADR-008-kairos-joins-locus.md) ("keryx will later too").
**From:** your words, 26 Oct: "lets begin refining it to something I actually find easier and
better to use than mail". **Builds on:** [kairos-joins-locus.md](kairos-joins-locus.md) and
[kairos-as-a-locus-app.md](kairos-as-a-locus-app.md), whose steps this follows for keryx.

**Done when:** keryx is built, tested and deployed from this repository and looks and behaves like
locus (one theme, one set of keys, one approve place, the taxonomy and registry); triage acts on
your real mail; the Mail view (`Alt-4`) leads with what needs you rather than with everything that
arrived; mail is filed by what it is for and shows on that item's page; and Claude's work on your
mail (what needs a reply, drafts, dates and tasks) waits in Approve.

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

## Decisions only you can make

| # | Question | Options | Recommendation |
|---|---|---|---|
| X-a | Acting on real mail | a. `live = true`, after a round trip on `[keryx-test]` mail from each account (archive, file, reply, undo, seen in Mail and on the phone within seconds) · b. stay read-only | **a.** Without it nothing you do in the Mail view sticks, which is the main reason it can't beat Mail. |
| X-b | What the Mail view leads with | a. **needs you** first: mail from people and threads waiting on your reply, then one line per kind of automated mail with its count (`newsletters 23 · notifications 14 · receipts 6`), each opening to clear in one go · b. one list by date, as now | **a.** The people are what matters and they are the minority; the rest is cleared in batches, not read one by one. Mockup below. |
| X-c | Claude's part | a. a locus background job, inside the budget (plan E): it marks what needs a reply and why, drafts the clear-cut replies, proposes archiving or filing automated mail, and finds dates and tasks; all of it waits in locus's Approve · b. keryx's own scheduled runs (08:00, 18:00) and its own review screen | **a.** One approve place and one budget, as for everything else; keryx's own review screen would be a second Approve. |
| X-d | Filing | a. by **what it is for** (a registry item: B14, Millennium, QUAST; or an area or stream: Admin is Errands), each mapped to the folder it goes in on that account; a thread filed for an application or a course shows on its page · b. keep keryx's six keys (Admin, Engineering, Careers, Travel, OQTS, JCR Treasurer) | **a.** The same "For" as kairos (ADR-009). The server folders stay as they are; only what you choose changes. |
| X-e | Marking several threads | a. `v` marks (as in visual mode), since `x` now removes everywhere · b. `space` | **a.** `space` asks on the Project view, and `x` would mean two things. |
| X-f | The old keryx repository | a. archived once this is done (as kairos's was) · b. kept | **a.** |

**Mockup for X-b** (the Mail view, narrowed to fit here):

```
╭ mail · needs you 3 ────────────────────────────────────────────────────────────────── 4 accounts ╮
│─ needs you ──────────────────────────────────────────────────────────────────────────────── [n] ─│
│ ▸ an applicant    Following up on an event         asks if there's news   00:27  OQTS            │
│   a recruiter     Re: summer internship            waiting on your reply  sun    Oxford          │
│   a society       A competition invitation         new                    sun    OQTS            │
│─ for you to see ─────────────────────────────────────────────────────────────────────────── [s] ─│
│   a committee     This week's agenda               from a person, no ask  fri    Oxford          │
│─ everything else · 41 ───────────────────────────────────────────────────────────────────── [e] ─│
│   newsletters 17 · notifications 12 · receipts 4 · lists 8   ⏎ review · d clear all              │
╰ ⏎ open · d archive · m file for · r reply · t task · ? keys ─────────────────────────────────────╯
```

---

## Rules
- **Nothing that works stops.** Each step leaves keryx building, its tests green and keryxd
  running; your mail is touched only after X-a, and only by your own keys or what you approve.
- **Never delete. Claude never sends. Mail is untrusted.** keryx's rules, unchanged: no trash or
  expunge path; a draft Claude writes waits for you and sends only after your yes, with the 10 s
  hold; mail text reaches Claude only inside keryx's marked envelopes, and never reaches a
  terminal raw.
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
- locus-theme for the look (grey frame, its title on the left, locus's footer, hints, lowercase
  days); the shared keys (`d` done is archive, `x` removes nothing here so it goes, `v` marks per
  X-e, `+` composes); "approve" not "approvals"; `kairos_key` and keys become For (per X-d).
- One approve place: drafts waiting to send and cleanup proposals appear in locus's Approve (as
  calendar changes did), and keryx's `A` opens it. The phone pages and ntfy pushes go (plan B.4).
- One contract: what locus shows of mail (Home's mail line, an application's timeline, a course's
  page) comes from keryx as locus-contract types; the date finder reads keryx through it rather
  than running the `keryx` command.
- **Check:** keryx's and locus's key lists pass the shared-key test; dumps at 95 × 38 and
  155 × 43 beside Home; locus's Approve shows a `[keryx-test]` draft and sends it on `y`.

### X3 Reading
- Invisible preheader characters stripped; tracking links shown as their site (`[1] aaa.com`)
  with the full address one key away; the account as a short coloured tag that never cuts.
- **Check:** your real newsletters, before and after, as dumps.

### X4 Live (per X-a)
- The round trips on `[keryx-test]` mail from each account, then `live = true`.
- **Check:** each round trip seen in Mail and on the phone, and undone with `u`.

### X5 The view (per X-b), first cut
- Needs you, for you to see, everything else in kinds; built from keryx's own signals first
  (sent to you by a person, a reply you owe, automated by headers), Claude's marks (X6) refining
  them.
- **Check:** your inbox as it is that day, in the three parts, for you to judge.

### X6 Claude on your mail (per X-c)
- A locus job, inside the budget: needs-reply marks with a reason, drafts for the clear-cut ones,
  cleanup proposals for automated mail, dates and tasks found in mail (beside the date finder
  already in Approve), each into Approve.
- **Check:** one real run on your mail, its items in Approve, what it cost.

### X7 Filing for (per X-d)
- `m` asks what it is for (the same type-ahead as kairos); the thread goes to that account's
  folder for it and shows on the item's page.
- **Check:** a Millennium thread filed, seen in Mail's folder and on Millennium's timeline.

### X8 Rounds with you
After X5, the Mail view is reviewed in use like every other screen: what you reach for, what you
skip, what Mail still does better. Each round's changes are recorded in slice-1.md.

## Order
X1, X2 and X3 (mechanical, no change to your mail), then X4 with you, then X5 to X7, then the
rounds. X6 can wait for Slice 0's budget logging if that is not in yet.

## Report
*(when the steps are done)*
