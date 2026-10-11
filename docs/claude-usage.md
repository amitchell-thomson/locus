# Using kairos from Claude

kairosd exposes an MCP server at `http://compute-node:7420/mcp` (Streamable HTTP, **stateless**, bearer token). A dropped network or daemon restart costs one retry, not a dead session: there's no session to lose.

## Register with Claude Code

```sh
claude mcp add --transport http --scope user kairos http://compute-node:7420/mcp \
  --header "Authorization: Bearer $(security find-generic-password -w -s kairos-api-token)"
claude mcp list   # kairos: … ✓ Connected
```
(On compute-node itself use `$(cat ~/.config/kairos/secrets/api-token)`.) Check the current syntax with `claude mcp add --help` if this ever changes.

## The rules Claude works under

- **The server decides risk.** Low-risk changes are applied at once (and undoable): task blocks, tasks, deadlines, new one-off events that don't clash and aren't within 2 h, re-keying events kairos made. High-risk changes come back `status: "pending"`: editing/moving/deleting Alec's own events, anything recurring, anything within 2 h, clashes with fixed events, sharing to Family, deleting deadlines, dropping tasks, >8 ops.
- **There is no approve tool.** Claude says what's pending and how to approve it: TUI `A`, `kairos approve <id>`, or the ntfy notification on the phone. In Claude Code, Claude may run `kairos approve <id>` via Bash **only after Alec says yes** — Claude Code's own permission prompt is the second gate.
- **Don't hand-place work.** Create/update tasks with estimates and deadlines and let `plan` place blocks. Only use `create_event` with `task_id` (a pinned block) when Alec asks for a specific slot.
- **Report shortfalls honestly.** If `plan` returns `shortfalls`, say e.g. "B4 sheet: 2.5 h short before Fri 17:00" and list the options it gives.
- While `live = false`, everything lands in `ZZ-kairos-test` (intended key kept for colour) and anything touching other calendars is refused.

## Workflows

### "Add a B4 problem sheet due Friday 17:00, about 4 hours"
1. `create_task {title: "B4 problem sheet", key: "Engineering", estimate_min: 240, due: "2026-10-16T17:00"}`
2. The response includes the ⚑ deadline changeset and the plan: blocks placed (and any shortfall).
3. Reply with the blocks ("Mon 14:00–16:00, Tue 09:00–11:00") or the shortfall and options.

### Moving one of Alec's events
`update_event {uid, start: "…"}` → `pending` with reasons. Tell Alec it's queued and where to approve.

### Weekly review (Sunday)
1. `list_events {from: today, to: +7d}` — skim the week.
2. **Triage the Inbox:** events in `Inbox` that belong elsewhere → suggest keys; on Alec's OK, `move_event` (his own events → pending; that's expected).
3. **Imported:** `list_events {overlays: ["imported"]}` → suggest anything worth adopting (bookings, travel) → `kairos adopt <uid> --key Travel` (via Bash, after OK).
4. **Deadlines:** `list_tasks` → check dates/estimates are still right; `update_task` as needed.
5. `plan` → summarise blocks, moved count, shortfalls.
6. `list_changesets {status: "pending"}` → remind Alec what's waiting.
7. One-paragraph summary of the week ahead.

### Daily
1. `list_events` for today + `list_tasks` → "Today: … ; work blocks: …; due soon: ⚑ …".
2. If Alec reports how blocks went, the check-in lives in the CLI/TUI/phone (`kairos checkin`), or adjust with `update_task {remaining_min}` and `plan`.

### Undo
`list_changesets {status: "applied"}` → `undo {id}`. If the event was edited elsewhere since, undo refuses with a diff — show it to Alec.
