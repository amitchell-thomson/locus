# ADR-002 — How the build is run

**Status:** accepted
**Date:** 2026-10-19
**Your words:** "the key part of my work is the planning and guidance for the system, I want to
make sure we use the best method for executing the plan (ie slash goal or just chat messages to
'start slice 0' etc...)" (19 Oct 2026); "yes go ahead" to the recommendation below.

## Context
Options available in Claude Code (checked against its docs, 19 Oct 2026):
- `/goal <condition>` keeps a session working, turn after turn, until a small model judges the
  condition met. The judge runs no commands and reads no files: it sees only what the
  transcript shows, so a condition must name a check whose output is printed, plus a turn bound.
  Session-scoped; unattended only in auto mode.
- Plain chat keeps Alec in every step but drifts and loses context over a long slice.
- Plan mode blocks edits until a written plan is approved.
- Workflows fan out many agents; suited to wide jobs, not ordered slices.

## Decision
1. Each slice starts with a spec in `docs/redesign/slices/`: steps with checks, Alec's decisions
   listed up front, departures from the plan flagged. He approves before anything is built.
2. One named session per slice. Mechanical steps under `/goal` with a printed check and a turn
   bound; judgement stops and asks. Design work (Slice 1) is plain chat with him at the screen.
3. Each slice ends with a report in its spec file: done, evidence per "done when", deviations,
   decisions. Big ones go to the tablet.

## Consequences
- Alec spends his time on specs and reports, not on supervising mechanical work.
- A spec costs time before each slice; small slices may get a short one.

## What would make this wrong
If `/goal` runs drift from the spec or stop early often, or if Alec finds specs a chore he skims;
then steps become smaller and chat-driven, or specs shorter.
