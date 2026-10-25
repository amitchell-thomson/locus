# kairos: the calendar of locus

kairos is locus's calendar: your events, your tasks and the time planned for them, on top of
iCloud. It is one of locus's apps (ADR-008), speaking its taxonomy and registry, and shown in the
locus window's Calendar view (`Alt-3`). This file says what kairos does and the rules it keeps.
How it is built and the invariants that must not break are in [`CLAUDE.md`](CLAUDE.md); the keys
and what is drawn are in [`docs/tui.md`](docs/tui.md). The original build prompt (phases 0 to 9,
October 2026) is in git history.

Rewritten 25 Oct 2026, after spec K ("kairos joins locus") and spec E ("adding to the calendar")
and the removals of that day: project sessions, proposed project time, and the phone pushes and
pages.

---

## 1. What it is for

- **iCloud stays the source of truth for events**, so the iPhone's Calendar, its notifications and
  Siri keep working, and anything you add on the phone is in kairos within a minute or two.
- **A task planner**: a task with an estimate and a due date gets its work planned into your free
  time as calendar blocks, moved out of the way when your week changes, and checked in when done.
- **One week in two places, never two versions of it**: locus's Home and Project views show the
  same events, tasks and due dates, from kairos.
- **Claude can change your calendar safely**: every change is computed for risk by kairos, never
  labelled by the caller; anything risky waits for you in locus's Approve.

Not for: replacing the phone's Calendar app, writing to Google or Exchange calendars, invitations,
anyone but you.

---

## 2. What it knows

**Calendars are the locus areas** (ADR-008). Studies, Projects and Careers are each one iCloud
calendar; Personal is filtered by its streams, each its own calendar: Sport, Social, Errands, and
Family (the shared iCloud calendar, read-only except "share to Family"). Old names still resolve
(Engineering is Studies; Personal and Inbox are Errands). Events added without a calendar go to
Errands, the inbox.

**Event**: anything in those calendars, read losslessly (unknown iCalendar properties survive
every rewrite). kairos keeps its own facts about an event in a sidecar, never in iCloud: who made
it, whether it is a work block, what it is for (`locus`, a registry id), whether you are going,
whether a Family event blocks your time. Kinds: an ordinary event, a work block (a task's planned
time), and worked time (measured, below), which is never a calendar event.

**Task**: what it is for (a registry item such as `b15`, `millennium` or `quast`, or a Personal
stream), how long it takes in total, time left, an optional due date (date, or date and time),
optional "before" (an event it prepares for: its due time follows that event), status open, done
or dropped. **A due date is the task's attribute, never an event**: it is drawn as ⚑ at its time,
or at the top of the day when it has no time.

**Project**: a locus registry repository, kept as a task of kind project with a stage (active,
next, paused) and a note. kairos's project list is the registry's; a repo added there appears by
itself.

**Changeset**: every write, with full before and after images, who asked, the computed risk, and
its status (pending, applied, rejected, undone, failed).

---

## 3. Adding, moving and linking

Adding anything asks **what it is for**, by type-ahead over the registry and Personal's streams;
the calendar follows from the item's area (ADR-009). A task needs only how long it takes; how it
is planned follows from that: an hour or less in one go, a short Errands task as a chore in a gap,
longer in 2 to 3 hour blocks, a course's sheet 5 hours by default. `L` says what something is for
afterwards. A dentist appointment is Errands; a birthday Social, or shared to Family.

Shift-↑/↓ moves the selected item 30 minutes, Shift-←/→ a day; each nudge is one change with an
undo, and a work block you move is pinned. Moving an interview moves its prep's due date with it,
and the prep is planned again.

Dates found in your mail (bookings and rebookings for registry applications) reach the calendar
only through locus's Approve, linked to their application.

Enter on a linked item opens it in locus; locus can ask kairos to open a day.

---

## 4. Risk

Decided by kairos on every changeset, never by the caller. MCP has no approve tool.

**Refused:** writes to read-only imports; writes to Family except "share to Family".

**Low, applied at once and undoable:** the scheduler's own work blocks (any number, at any time,
so work moves out of the way of a new event at once); new tasks and changes to them; new one-off
events that do not clash with a fixed event; re-keying events kairos made; recording a check-in.

**High, waiting for you:** changing, moving or deleting an event you made yourself; anything
recurring; anything starting within 2 hours (except the scheduler's blocks); an event clashing
with a fixed one; sharing to Family; dropping a task or a due date; more than 8 operations at once
(the scheduler's own block changes do not count).

**Your own edits** typed in the TUI or the CLI on a terminal skip approval (forbidden rules
still apply). **The one approve place is locus's Approve**; kairos's `A` opens it. Undo restores
the before images and refuses, with a diff, if the item was changed elsewhere since.

---

## 5. The scheduler

Deterministic code; Claude never hand-places work.

- **What blocks time:** your events (not Family's unless you opt one in, not ones you are not
  going to), with buffers around them; recovery time after Sport.
- **Constraints** (`constraints.toml`): term and vacation profiles with a day window, meals,
  deep-work windows, a later start at weekends, at most 3 hours back to back without a break and
  10 hours of load a day (Social and Sport not counted), finishing 2 hours before a due time when
  there is room.
- **Placing work:** earliest due first with slack; the best tier that fits (ideal blocks, then
  shorter, then squeezed down to 40 minutes, up to the due time, never past it); spread across the
  days before the due date in proportion to their free time; never within the next 15 minutes.
- **Stable:** blocks that still fit stay; only displaced work moves, or work that can come out of
  a squeeze, or (when time is freed) unpinned work from 2 hours ahead, which re-flows.
- **Honest:** a shortfall is reported per task ("2.5 h short before Fri 17:00") with options; a
  squeeze is reported per block with its reasons. Never silently overbooked.
- **Learned estimates:** after three finished tasks of a shape, time left is planned at your
  median actual-to-estimate ratio (clamped ×0.7 to ×2), and the plan says so.
- **Chores** take the smallest gap they fit, away from deep-work hours.

---

## 6. Check-ins and time spent

- A work block that has ended waits for you to say how it went: done, partly (how long), took
  longer (how long, how much more, from when), missed, or finished (the task is done). The
  calendar then shows it: ✓ in the title, trimmed to what was done, or removed if missed. Nothing
  is decided for you; an unanswered block counts as done after a week.
- **You check in from the calendar** (on the block, or `C` for every block waiting), with
  `kairos checkin`, or by telling Claude. The TUI opens the list once each evening if anything
  waits. Nothing is pushed to the phone.
- **Tasks finish only when you say so.** At 0 time left a task asks: finished, or how much more?
- **Time on a project** is measured from your Claude sessions in its repository (a span from a
  prompt you typed until ten quiet minutes) and drawn as a faint bar on the left of the day, with
  nothing to check in; plus the checked-in blocks of tasks for it. The two are merged as spans of
  time, so an hour worked through Claude inside a block counts once. Time is set aside for a
  project by planning a task for it.

---

## 7. Where you meet it

- **The locus window, Calendar view** (`Alt-3`): week, day, month and agenda; arrows and Tab;
  every hour the same height. Its Tasks page, Home's projects section and Approve are locus's
  (`T`, `P` and `A` open them).
- **Home**: today's events, due dates, the chore that fits now, tasks beside Approve.
- **The phone**: the iCloud calendars (add and edit events there as usual), and the "Add task"
  Shortcut (`docs/shortcut.md`).
- **The CLI** `kairos`: `today`, `week`, `ls`, `next --tmux`, `add`, `task`, `deadline`, `plan`,
  `checkin`, `changes`, `approve`, `undo`, `doctor`, `sync`; `--json` on every read
  (`docs/json.md`).
- **Claude**: the MCP server at `/mcp` on kairosd (Streamable HTTP, stateless, bearer token, over
  Tailscale); `docs/claude-usage.md` and the `kairos` skill.

---

## 8. How it runs

iCloud ⇄ vdirsyncer on the server ⇄ kairosd, the only writer. The Mac keeps a read-only copy
(rsync every minute) for the CLI and for the TUI when the server is unreachable. kairosd and the
CLI are built from this repository and deployed by `deploy/deploy.sh` (unit
`locus-kairosd.service`). Semantic rules (DST, all-day ends, recurrence, dedup, the risk table,
the scheduler) each have tests; `scripts/ci.sh` (at the root) is the gate.

### Rules for times and repeats

- All-day ends are exclusive. An ambiguous local time is its first occurrence; a non-existent one
  takes the offset before the gap. Recurrence runs in the series' own wall-clock time, so 19:00
  stays 19:00 across the clock change.
- Duplicates are hidden in the view, never deleted: same UID, or matching titles starting within
  15 minutes on the same day (both timed or both all-day). Family's copy wins, then yours, then an
  import.
- A term-time repeat is one series per term, from the term profile's dates.
