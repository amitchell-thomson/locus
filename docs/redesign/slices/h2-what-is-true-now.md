# H2: what is true now

**State:** approved 11 Nov 2026 with your amendments (D1 slots widened, D6 deferred to the Canvas pull, D7 noted) · **Written:** 2026-11-11 · **Runs in:** `claude` (locus-dev)
**From:** heuris v2 (accepted 10 Nov), sections A, B ("Facts, and how they are kept honest"), D, F, I
row H2, K ("decided in H2"); ADR-011 decisions 3 and 9.
**Builds on:** H1 (closed 10 Nov: the index, `context`/`search`/`open`/`timeline`/`waiting`, 36 of 49
questions passing); Slice 0's API-credit route and call ledger; keryx's scheduled runs; the Home chat.

**Done when (from heuris):** "the offers, mail and Jump-feedback questions come back with the right
current state; a CV description of nabla is accurate." Scored on the H2 questions of the test set
(Y1, Y7, Y9, 14, 35, 42, 43; Q31 waits for the syllabi, D6) and on the 13 H1 partials, each in a fresh chat as in H1, with
effort (calls, seconds, first useful call) recorded. Target: every H2 question passes, the two
integrity questions (42, 43) pass exactly, and no H1 pass becomes worse.

---

## Why H2

H1 made everything findable. What H1's partials still lack is **what is true now**, held in one
place with its source and date, rather than re-derived by each chat from whatever it happens to find:

- Y1 framed the offers without your stated goals (systematic trading, then a graduate role) because
  those live in a chat and a mail, not as a fact.
- Y5 gave its own reasons why regime-ml stopped; the decision is nowhere it can be looked up.
- Q35 and P2 read a dropped task as live; the state of a thing was inferred from fragments.
- The BH contract was "signed" in one place and "pending the background check" in another, and only
  you could say which was current.
- Brian's advice reached answers only after it became a note in your applications file.

heuris v2's four missing things were: one index (H1, done), **a ledger of current facts where a
newer one replaces an older one**, **a short current brief for each thing and for you**, and Claude
reasoning with tools (H1, done). H2 is the middle two, plus records (what happened, per session or
thread) that briefs draw their history from.

---

## Decisions for you (my recommendation first)

**D1. How a fact is named, so a newer one finds the one it replaces** (heuris K: "decided in H2").
A fact is `subject · slot = value`, as of a date, with its sources.
- *subject* is a registry id (`millennium`, `quast`, `b15`, `jcr-treasurer`), `me`, or a person
  (`person:brian`).
- *slot* comes from a fixed list per kind of subject (below), so "Millennium's stage" is always
  the same slot and a newer value replaces the older.
- **Single slots** hold one current value (`stage`, `start`). **Named slots** hold many, each
  under a short name: `experience:millennium-qd-2027`, `achievement:gr-quant-challenge-2026`,
  `note:brian-advice`. A named slot is replaced only by a new value under the same name.

Your call (11 Nov): **every fact worth remembering must have a slot.** So:
- the list covers your experience, projects, achievements, skills and education, not only status;
- every kind has named `note:<name>` slots as the catch-all, so nothing true is ever unrecordable;
- the list lives in one file (`taxonomy/fact-slots.toml`), and `remember` with a slot not on it is
  refused with the nearest slots, so new kinds of fact get a deliberate slot rather than drifting.

| Subject | Single slots | Named slots (many) |
|---|---|---|
| **me** | `status` (where you are now), `location`, `degree` | `goal:`, `preference:`, `constraint:`, `plan:`, `decision:`, `experience:` (roles, internships, research), `achievement:` (awards, competitions, results), `skill:`, `education:` (courses, grades, qualifications), `project:` (things you built outside the registry), `interest:`, `contact:` (people and how you know them), `note:` |
| **application** | `stage`, `turn`, `start`, `end`, `deadline`, `outcome` | `offer:` (terms), `interview:` (round, date, who), `contact:`, `feedback:`, `note:` |
| **course** | `tutor`, `exam`, `mark` | `tutorials:`, `sheet_due:`, `topic:` (covered, with confidence later in L), `note:` |
| **repo / project** | `state`, `goal`, `stack` | `result:` (numbers, with where they come from), `decision:`, `achievement:`, `idea:`, `note:` |
| **role** | `state`, `term` | `duty:`, `deadline:`, `contact:`, `note:` |
| **person** | `role`, `organisation`, `relation` | `contact:`, `said:` (advice, commitments, with date), `note:` |

The alternative, free-text facts matched by similarity, is how the Jump mistake happens: two
phrasings of one fact both look current.

**D2. Which facts are written from mail and the calendar without asking** (heuris B: "automatically,
with its source, in the auto log with undo").
Recommended: automatic for what a sender of authority states about a thing: an application's
`stage`, `offer` terms, `start`, `deadline` from the firm or recruiter; a course's `tutorials` and
`sheet_due` from the tutor or department; a booked interview from the calendar. Each is listed in
the automatic-actions log with undo. **Anything else from mail** (a friend's message, your own
sent mail stating a plan) and **any conflict** with a current fact goes to Approve as a proposed
fact. Nothing from chats is automatic (unchanged from heuris).

**D3. Your applications file** (`~/files/Careers/applications.toml`, "kept by you and the Home chat
until heuris H2 writes it from mail").
Recommended: its entries are imported once as facts (stage, turn, notes), and from then on the
**facts are the record**: Home's careers section, `context` and briefs read facts. The file is
retired, replaced by "say it in any chat" (`remember`) or `y` in the Home chat. Alternative: keep
the file as a hand-edited override that wins over mail. That keeps two sources of truth, which is
the failure H2 exists to remove.

**D4. What a brief looks like** (heuris K: "decided in H2").
One per registry item that is live or active, and one about you. About 250 words, the same four
headings each time:
- **Now:** current facts, each with its date;
- **Open:** what is waiting on you or on someone else;
- **Recent:** the last two to four weeks, from records, newest first, with pointers;
- **Next:** dates coming up.
Rebuilt when a fact it rests on changes, and at most nightly otherwise. `context(about=…)` returns
the brief, plus any record written since it was built, so it is never stale where it is read
(heuris B). **Recommended**; a longer free-form brief reads better but is harder to keep exact.

**D5. Records: one dated note per unit of work** (heuris B).
- **Chats:** one per Claude session with at least three of your turns, written when the session
  has been quiet for an hour: what was asked, decided, made, and left open.
- **Commits:** one per repo per day with commits.
- **Mail:** one per changed thread from a person or firm (not lists, not OQTS-shared).
Records are indexed (kind `record`), so `search` and `timeline` find them, and briefs take "Recent"
from them. A hypothetical discussed in a chat can appear in that chat's record as *discussed*,
never in a brief as *true*: briefs take current state only from facts (heuris B). **Recommended.**

**D6. Years 1 and 2: syllabi as "studied" facts** (heuris C, Q31).
**Deferred, your call (11 Nov): "we can do those once the canvas pull is set up."** The syllabi
come with the Canvas pull, as `education:` and course `topic:` facts; Q31 is scored then.

**D7. Chats on claude.ai** (heuris K: "whether chats on claude.ai are brought in").
They live only in Anthropic's export. **Not in H2** (agreed 11 Nov). Your note: this may be a
shortfall later, since you do use claude.ai sometimes, and you would move more to Claude Code
chats only once the terminal renders LaTeX and equations well ("there is no real difference
between the two" otherwise). Recorded as a known gap; equations in the terminal are their own
piece of work, not H2's.

**D8. Models and cost** (heuris F).
Facts, records and briefs run on the API credit through the Batch API, never on your subscription:
- **Sonnet:** facts from mail, chat records, briefs;
- **Haiku:** commit and mail-thread records.

Estimated $20–30 a month in use, plus about $5 once for the backfill (D9), against the $100. The
credit ledger already paces and logs every call (`locus budget`). **Recommended.**

**D9. Backfill.** Recommended: facts and records from mail and chats **since 1 June 2026** (this
recruiting season, the BH internship, quast, locus), briefs for every live item. Older history stays
searchable through H1's index without records.

**D10. How `remember` is offered.**
- `remember` and `forget` stay off the allow list, so Claude Code and claude.ai show the exact fact
  and you answer with one key.
- Your global CLAUDE.md gets one line: offer `remember` straight after a message of yours that
  states a fact worth keeping, and never for a hypothetical ("suppose…", "what if…").
- The Home chat shows its own `remember: "…"? y/n` line under the reply (heuris B).

**Recommended.** If you approve nearly every one, heuris says to revisit (ADR-011).

---

## How it works

```
mail, calendar ──► fact extractor (Sonnet, batch, on the credit) ──► facts ledger ──► briefs (Sonnet)
   (keryx, kairos)        │ authority + no conflict: written, logged, undo             │        │
                          └ else: proposed in Approve                                  ▼        ▼
any chat ── remember / forget (you approve: prompt, or y/n in the Home chat) ─► facts   context, Home
sessions, commits, threads ──► records (Sonnet / Haiku, batch) ──► index (kind record) ─► briefs' "Recent"
```

**Where it lives.** On the server, in `heuris.db` beside H1's items: three new tables:
- `facts`: subject, slot, value, as_of, sources, route, status, replaces, created;
- `records`: unit, period, text, sources;
- `briefs`: subject, text, built_at, built_from.

Facts and records are also indexed as items, so every H1 tool sees them.

**Who writes.** heuris writes facts, records and briefs only. Mail stays in keryx, the calendar in
kairos (heuris v2). The extractor reads mail through H1's index, the same cleaned, scrubbed,
OQTS-shared-free text H1 indexes.

**Correcting.** "That's wrong" in any chat calls `forget` (you approve), which retires the fact and
restores the one it replaced, if any. An automatic fact is undone from the automatic-actions log
(Approve's `l`). Both are logged.

**What the tools gain.**
- `context` returns the facts and the brief, plus newer records (a project, firm or you).
- `search` and `timeline` find facts and records like any item.
- New tools:
  - `remember(subject, slot, value, source?)`;
  - `forget(fact)`;
  - `facts(subject?, slot?, history?)` for the exact current value and what it replaced.

**Integrity.**
- A fact is stored only with a source: a pointer, or "you said so in chat <session>".
- An extracted fact must quote the sentence it rests on. The quote is checked against the source
  text after the call (CLAUDE.md §2.1: a model told to cite will cite, so it is verified, not
  trusted).
- A fact whose quote is not in the source is dropped and counted in `locus gates`.

---

## Steps

Each step ends with its check; the slice ends with the scoring (H2.10).

**H2.1 The ledger.**
- Build the `facts` table, the `facts` tool, and `context` showing current facts with dates and
  sources.
- Import your applications file as facts (D3).
- *Check:* `context` shows Millennium as offer accepted and BH as signed with the renege planned,
  each with its source; `facts(millennium, stage, history=true)` shows the chain.

**H2.2 `remember` and `forget`.**
- Add the two tools (off the allow list), the CLAUDE.md line (D10) and the same line in the server's
  instructions.
- *Check:* in a fresh Claude Code chat, saying "Jump said no offer, reapply next year" offers
  `remember` with that exact fact. "Suppose I took the Jump offer" offers nothing. `forget`
  restores the replaced value.

**H2.3 The Home chat's `y/n`.**
- When a headless turn's `remember` is refused, Home shows the fact under the reply; `y` records
  it, `n` drops it.
- *Check:* in the locus window, with a test fact.

**H2.4 Facts from mail and the calendar.**
- A batch job, every hour, on threads changed since the last run: Sonnet reads each thread from a
  person or firm and proposes facts in the D1 shape, each with the quoted sentence it rests on.
- The quote is verified, then the fact is written (D2: authority, no conflict) or proposed in
  Approve.
- Interview bookings come from the calendar directly, with no model.
- *Check:* on the last month of mail, every application's stage matches your applications file
  (now facts) or is proposed as a change; Isobel's B18 tutorials and the sheet date appear as
  course facts.

**H2.5 Home reads facts.**
- Home's careers section reads stage and turn from facts, not the file.
- Fact proposals and conflicts appear in Approve; automatic facts appear in the log with undo.
- *Check:* Home and `context` agree with each other and with the ledger.

**H2.6 Records.**
- Chats, commits and mail threads (D5), batched on the credit, indexed as kind `record`.
- *Check:* yesterday's sessions each have a record that names what was decided; a search for "why
  did regime-ml stop" finds a record if one was ever written, and says so plainly if not.

**H2.7 Briefs.**
- One per live item and one about you (D4), rebuilt on fact change.
- `context(about)` returns the brief plus newer records.
- *Check:* the quast brief's "Now" matches `state.md`; your own brief names your goals as you
  stated them, with sources.

**H2.8 Syllabi:** deferred to the Canvas pull (D6).

**H2.9 Backfill.**
- Facts, records and briefs since 1 June (D9), batched; spend logged.
- *Check:* `locus budget` shows the backfill within the estimate.

**H2.10 Scoring.**
- The H2 questions and the 13 H1 partials, in fresh chats, graded as in H1, with effort.
- Report to you with the before and after.

---

## Not in H2

- **Learning** (goals, tracks, refreshers, marking): slice L.
- **Facts from chats without approval:** only if `remember` turns out to be friction (ADR-011's
  revisit).
- **claude.ai chats** (D7), and equations rendered in the terminal (what would let you do
  everything in Claude Code).
- **Year 1 and 2 syllabi** (D6): with the Canvas pull.
- **The old engine's retirement** (propositions and the rest, heuris G): measured separately, not
  needed for H2.

## Risks

- **Wrong automatic facts.** Mitigated by authority-only automatic writes, quote verification,
  conflicts to Approve, and undo. The first week's automatic facts get a review list.
- **Cost drift.** Mitigated by batch, cached prompts, and the ledger's daily pace; briefs rebuild
  at most nightly unless a fact changes.
- **Records that restate hypotheticals as plans.** Records say "discussed"; briefs never take
  state from records.
