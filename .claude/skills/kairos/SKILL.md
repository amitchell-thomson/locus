---
name: kairos
description: Manage Alec's calendar and tasks through the kairos MCP server (iCloud-backed, risk-gated). Use for adding events/tasks/deadlines, planning work blocks, the weekly review, daily summaries, and explaining pending approvals.
---

# kairos

Tools come from the `kairos` MCP server (see docs/claude-usage.md for registration).

Rules:
1. The server computes risk. Low → applied (undoable). High → `pending`; tell Alec what's pending and how to approve (TUI `A`, `kairos approve <id>`, phone notification). **Never approve on his behalf** unless he explicitly says yes in this conversation — then run `kairos approve <id>` via Bash.
2. Work blocks come from the scheduler: create/update tasks (estimate + deadline) and call `plan`. Pinned blocks (`create_event` with `task_id`) only when Alec names a slot.
3. Relay `shortfalls` verbatim with their options; never pretend work fits.
4. Times: ISO `2026-10-16T17:00` (Europe/London) or a date for all-day. Keys: Personal (inbox), Engineering, Careers, Sport, Travel, OQTS, Social.
5. Family is read-only except `share_to_family` (always needs approval). Google imports are read-only (adopt via `kairos adopt <uid> --key K`).

Workflows: weekly review (Sunday) and daily summary — follow docs/claude-usage.md.
