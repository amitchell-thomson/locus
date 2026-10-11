---
name: kairos
description: Manage Alec's calendar and tasks through the kairos MCP server (iCloud-backed, risk-gated). Use for adding/editing/deleting events (including bulk edits like renaming or cleaning up many lectures), tasks/deadlines, planning work blocks, the weekly review, daily summaries, and explaining pending approvals.
---

# kairos

Tools come from the `kairos` MCP server (see docs/claude-usage.md for registration).

Rules:
1. The server computes risk. (Alec's own edits typed in the TUI/terminal apply directly; anything you do — MCP or `kairos` via Bash — goes through the policy.) Low → applied (undoable). High → `pending`; tell Alec what's pending and how to approve (TUI `A`, `kairos approve <id>`, phone notification). **Never approve on his behalf** unless he explicitly says yes in this conversation — then run `kairos approve <id>` via Bash.
2. Work blocks come from the scheduler: create/update tasks (estimate + deadline) and call `plan`. Pinned blocks (`create_event` with `task_id`) only when Alec names a slot.
3. Relay `shortfalls` verbatim with their options; never pretend work fits.
4. Times: ISO `2026-10-16T17:00` (Europe/London) or a date for all-day. Keys: Inbox (default), Engineering, Careers, Sport, Travel, OQTS, Social.
5. Family is read-only except `share_to_family` (always needs approval).
6. **More than one event → one batch.** Find with `search_events`, then ONE `update_events` / `delete_events` / `move_events` call with a plain-English `summary`. Never loop single-event tools: each would be its own approval.

## Bulk edits

1. `search_events` (query words, keys, location, window; default today → +180 d). Page with `offset` if `total` > rows.
2. Work out the change per event yourself (e.g. parse "B10 Basic Soil Mechanics #1 (Knappett) LR2" → title "B10 Basic Soil Mechanics #1 (Knappett)", location "LR2"). Skip events that don't match the pattern and say which.
3. Before submitting anything ambiguous or destructive (deletions, > ~20 events), show Alec the plan in a few lines: counts per group plus 2–3 before/after examples. Proceed when he agrees, or straight away if he already said exactly what to do.
4. Submit one batch. It will usually come back `pending` (his own events / > 8 ops): tell him "N events queued as <id> — approve with `A` in kairos, the phone notification, or tell me yes". One `undo` reverts the whole batch.
5. Documents/screenshots (e.g. his module list): read them, map module codes to titles via `search_events`, list what will be removed per module, then `delete_events`.

Workflows: weekly review (Sunday) and daily summary — follow docs/claude-usage.md.
