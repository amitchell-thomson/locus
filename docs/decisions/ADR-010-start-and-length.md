# ADR-010 — Every calendar item has a start and a length: ⇧ moves it, ⌥ changes its length

**Status:** accepted
**Date:** 2026-10-27
**Your words:** "I want a clean structure of moving/ changing extending time for events/ work time for tasks that works cleanly across all types ... I dont want these systems becoming messy, they are meant to be easier, more usable and make my workflow quicker"; "can we have some way to edit checked in blocks, as I accidentally said that one was done at one point and was not able to undo that accidental press"; "just implement all of this now" (27 Oct 2026)
**Builds on:** ADR-008, ADR-009

## Context
On 27 Oct, B14 Image Processing needed more time than planned. kairos had no way to make a
block longer, only to move blocks and change the estimate. A pinned block nudged back a day
landed on a morning block already checked in. The scheduler, presuming the unanswered past
block done, made a new one for the time "missing" from Wednesday, and that was moved onto the
same morning too. Three B14 blocks overlapped at 11:00. The week view drew one, because
`dedup` read two blocks with the same title starting 10 minutes apart as copies of one event.
120 minutes of work that never happened were counted, the task looked used up, and check-ins
asked about blocks the calendar did not show. Separately, an accidental "done" could not be
undone: the check-in was recorded as the scheduler's change, which `u` passes over, and undo
restored the calendar entry but not the check-in record.

## Decision
**Two gestures on whatever is selected, the same for every kind of item.** ⇧↑↓ moves it 30
minutes, ⇧←→ a day (as before). ⌥↑ makes it 15 minutes shorter, ⌥↓ 15 minutes longer: the end
moves, the start stays. Together they reach any start and end (started later and kept going: ⇧↓
then ⌥↓). `m` and `e` stay as the typed, exact versions.

**What a length change means follows from the item.** An event: the event. A work block ahead
or under way: its task gets that much more (or less) time, so the rest of the plan stays where
it is. A work block in the past: what happened; fix it, then `y`. A checked-in (✓) block: still
editable, the record changes and what is left of the task does not. Moving never changes how
much time a task gets; moved into the past, a block says when the work was done.

**kairosd enforces two rules for every caller** (TUI, CLI, MCP). No two work blocks overlap:
a flexible block in the way is moved by the replan, while a pinned, started or checked-in one
stops the edit with a message naming it. And two different work blocks are never drawn as one.

**A check-in is one changeset, authored by whoever answered** (you, or Claude for you): the
record, the task's time, the ✓, and "task finished" when it was. `u` takes all of it back.
`y x d` answer a ✓ block again, so "it didn't happen after all" is `x`.

**Keys:** +⌥↑↓; `c` (partly) and `l` (took longer) are gone, replaced by "make the block what
happened, then `y`". In the check-in list, ⏎ opens the block on its day to do that.

## Alternatives
- **A resize key per edge** (start and end separately): more to remember for no reach; move plus
  length already gives every start and end.
- **Length on a future block moves time between blocks** (estimate unchanged): rejected; what is
  drawn should be the plan, and "give this more time here" was what you were trying to do.
- **Refuse moving a planned block into the past**: proposed, then dropped. Moving a block back to
  when you did the work is the gesture for "I worked earlier"; what broke was the overlap and
  the hidden blocks, not the move.
- **Keep `c` and `l` beside ⌥**: two ways to say the same thing, and the keys you were trying not
  to accumulate.

## Consequences
- The check-in record travels in changesets (`Op::Checkin`), so `Checkin` and `Outcome` moved
  from kairos-store to kairos-core.
- Partial and took-longer outcomes stay in the API (Claude records "only managed 20 min" with
  them); only the TUI keys went.
- The took-longer form's "plan the rest from" field went with it; `not_before` stays in the API.

## What would make this wrong
- If ⌥ does not reach the TUI on a terminal you use (Option sent as a character rather than
  Alt), the gesture needs a modifier-free fallback such as `<` `>`.
- If you keep wanting a length change on a future block to move time rather than add it, the
  default is the wrong way round.
