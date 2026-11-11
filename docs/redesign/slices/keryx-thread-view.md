# keryx: a thread you can read at a glance, and replies that look normal

**State:** built 2026-11-11 (T1–T4 c203953, R1 460ecfb); §6 not built · **Written:** 2026-11-11 · **Part of:**
[keryx-joins-locus.md](keryx-joins-locus.md)
**From:** your words, 11 Nov: "I currently find reading threads in both apple mail and our system
tricky to figure out what is new, what is old, what was said when, what the back and forth has
been … I worry things are lost in conversation. The best end state is things look completely
normal to whoever I am corresponding with, and for me the thread is very easy to read, the
timeline is clearly visible … especially if it has been a while since I last read a certain
thread." And: "most recent should always be at the top".

**Done when:** opening any thread shows, without scrolling, who said what and when, the newest
first, what arrived since you last looked, and what they asked that you have not answered; and a
reply sent from keryx is indistinguishable from one written in Mail.

---

## Why

**Reading.** The thread view today runs oldest first, so the message you opened it for is at the
bottom. Every message prints its text plus whatever it quoted (folded to one line, but still a
line per message), older messages are cut to three lines, and nothing marks when you last read
it, how long the gaps were, or who joined when. In an 11-message thread like Milly's (Tower, 29
Oct to 2 Nov) you scroll to the end and work backwards.

**Replying.** Measured 11 Nov with four replies saved as drafts on Oxford, then deleted:

| Test | What Mail saved |
| --- | --- |
| A: Mail's own reply, untouched | normal: "On 8 Nov 2026, at 22:30, Matilda Borthwick <…> wrote:" and her mail in a quote with a bar |
| B: script adds text at the top | her quoted mail gone |
| C: script replaces the body (keryx today) | her quoted mail gone; keryx's plain `> ` lines in its place |
| G: script sets the HTML (`html content`) | kept as written, quote block included, inside a borderless outer wrapper |

Mail adds the quote only when the message is saved, so a script that touches the body sees it
empty. What Matilda received on 10 Nov was keryx's plain text, `> ` on every line, her own
headers spelled out as text, in keryx's wording of the "On … wrote" line.

## What changes

### 1. Newest at the top, one block per message, only its own words

```
 Re: Tower QT/QR advice for timing and options                                personal
 Milly Parrott, Elspeth Proudlock, you · 10 messages · 29 Oct – 2 Nov · last read 3 Nov
 29 oct ●●●●●●●●●───────────────────────────────────────────● 2 nov          ● them  ● you
 your turn · she wrote last, 9 days ago

 ── draft, not sent ─────────────────────────────────────────────────────── 11 nov 14:53
       you        Hi Milly, thank you, that is super helpful, and good to know I can go
                  for either route (or both) next cycle…                         g d opens

 ── 2 nov · 3 days after your last ─────────────────────────────────────────────────────
 Milly Parrott    Hi Alec, I completely understand. While I'm disappointed we haven't had
 mon 13:35        the chance to interview you this time, …  (full text, it is the newest)

 ── 30 oct ─────────────────────────────────────────────────────────────────────────────
       you        Hi Milly, thank you so much for going out of your way to make this
       11:30      possible, I really appreciate it. I've spent…                  ⏎ more
       you        Thank you so much Elspeth and Milly! I am having a look now and I'll
       09:06      let you know by midday…
 Elspeth Proudlock  Hey Alec, no worries at all, I have now solved the issue and you
 08:59  joins     should now be able…
 Milly Parrott    Morning Alec, thanks for the CV! That's annoying, sorry about that.
 06:10            I'm cc'ing my colleague Elspeth who can…

 ── 29 oct ─────────────────────────────────────────────────────────────────────────────
       you        Thank you so much, that's very reassuring to hear…
 …
```

- **Newest first, always.** The newest message is shown whole; older ones show their first two
  lines and open with ⏎ (or all with `z`, as now).
- **Only what each message adds.** Each block is the sender's own text, with quoted history and
  signature removed (`text::compact`, which the summaries already use). Nothing is printed twice:
  what a message quoted is the message below it. `Z` still shows a message exactly as received.
- **Two lanes.** Their messages sit at the left edge, yours indented, so the back and forth reads
  down the page without reading names. The sender's colour is the existing voice colour.
- **Day bands with the gap written out:** "2 nov · 3 days after your last". A gap of a week or
  more is the line you notice.
- **Who joined when.** A new participant is marked `joins` on their first message; someone added
  in cc by another person is named there ("Milly added Elspeth").
- **A draft for this thread sits at the top**, marked not sent, with the key that opens it.

### 2. The header: the whole thread in four lines

- **Line 1:** subject and account.
- **Line 2:** the people, the number of messages, the span, and when you last read it.
- **Line 3: the timeline strip.** One mark per message on a date axis from first to last, in
  the voice colours, so a burst, a long silence and who went quiet are visible at once. A `◆`
  marks where you last read; marks after it are drawn bright.
- **Line 4: whose turn it is** and for how long ("your turn · she wrote last, 9 days ago", or
  "waiting on them · 4 days"). It uses the existing needs-reply mark when there is one.

### 3. What is new since you last read it

keryx records when you open a thread (a new `thread_seen` table: account, thread, time; local
only, regenerable from nothing). Messages after that time are drawn under a band
**"new since you last read · 3 Nov"**, and the timeline marks them. With no record yet, Mail's
unread flags stand in, as now.

### 4. What they asked that you have not answered

Below line 4, at most three lines: **sentences ending in `?` from their messages since your last
reply**, quoted exactly and with whose they are ("Milly: could you do a call on Thursday?").
Deterministic, no model: a quoted sentence is checkable, and a missing one is better than an
invented one. When you reply, they drop off.

### 5. Replies that look like Mail's

keryx writes the whole reply as HTML in Mail's own reply format and gives it to Mail with
`html content` (test G):

```html
<div>your text</div><div><br><blockquote type="cite">
  <div>On 2 Nov 2026, at 13:35, Milly Parrott &lt;…&gt; wrote:</div><br>
  <div>…her message's own HTML, which already holds the history below it…</div>
</blockquote></div>
```

- The "On … wrote:" line uses Mail's wording, with the address.
- Their message is quoted as they sent it (its HTML, or its text when there is none), so the
  history nests the way every client folds it.
- **The editor no longer shows a `>` quote** that would be sent. Their message is shown below a
  marker as reference while you write and is not part of what you type.
- Plain-text-only messages and forwards follow the same rule (Mail's "Begin forwarded message:"
  block for a forward).

**Open check:** Mail's borderless outer wrapper. A test reply went to your Personal address (cc
Oxford) on 11 Nov in the evening; how it looks in Gmail and Outlook decides between:
- (a) it is invisible: ship as above;
- (b) it shows: Personal and OQTS send directly over Gmail SMTP with the app passwords they
  already have, so keryx writes every byte; Oxford stays on Mail.

### 6. A one-line catch-up `[proposal]`

For a thread you have not opened in two weeks or more, `s` asks a model for one line on where it
stands, cached per thread until a new message arrives. Each call costs budget, so it runs only
when you press `s`. Left out of the steps below unless you want it.

## Steps

1. **T1, the order and the blocks:** newest first, own text only, two lanes, day bands with gaps,
   joins, the draft at the top. Snapshot tests on the Milly thread's shape (anonymised fixture).
2. **T2, the header:** the four lines and the timeline strip, fitted to the pane width.
3. **T3, last read:** `thread_seen`, the new band and the `◆`.
4. **T4, unanswered questions:** the sentences and their tests (a question you answered drops
   off; a quoted question inside their reply does not count).
5. **R1, the reply format:** after you look at the test mail. HTML in Mail's format, the editor
   change, tests that the built HTML matches Mail's own reply from test A. Then one real reply
   sent and checked by you before it is the default.

Each step is installed and shown to you with a `dump` of a real thread before the next starts.

## As built (11 Nov)

- **T1–T4** as above, with these differences: `⏎` shows every message whole (and folds them back),
  not one at a time, since the thread scrolls rather than selecting a message; "joins" is someone
  whose first message comes from outside every earlier From/To/Cc, and the message that brought
  someone in says "adds Elspeth"; lines a client wrapped at a fixed width are rejoined before the
  pane wraps them; a folded message's preview leaves its greeting off.
- **Last read** is a small file, `keryx-thread-read` in locus's state directory, not a table: the
  TUI opens the mirror read-only.
- **R1**: the engine builds the HTML at send time from the original in the mirror (its HTML part,
  else its text) and Mail sets it with `html content`. A plain `> ` quote at the end of the body
  (keryx's own, and the one Claude's waiting drafts carry) is cut off before sending. The editor
  still shows that quote under the marker as reference; it is not sent. Forwards and new messages
  go as before. The test reply of 11 Nov read as a normal Mail reply in Gmail and Outlook (yours).
- Checks: keryx-core, keryx-store and keryx-tui green; keryxd 72 of 74, the two failures
  (`automated_mail_is_filed_as_it_arrives_and_people_mail_never`,
  `restart_mid_session_needs_no_new_handshake`) fail identically at the commit before this work.

