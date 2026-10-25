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
- **There is no approve tool.** Claude says what's pending and how to approve it: locus's Approve page (kairos `A` opens it), or the ntfy notification on the phone. In Claude Code, Claude may run `kairos approve <id>` via Bash **only after Alec says yes** — Claude Code's own permission prompt is the second gate.
- **Don't hand-place work.** Create/update tasks with estimates and deadlines and let `plan` place blocks. Only use `create_event` with `task_id` (a pinned block) when Alec asks for a specific slot.
- **Report shortfalls honestly.** If `plan` returns `shortfalls`, say e.g. "B4 sheet: 2.5 h short before Fri 17:00" and list the options it gives.
- **Only record what Alec says.** Check-ins (`checkin`), logged work (`log_work`) and finishing (`complete_task`) are his answers relayed — never guess them.
- kairos is live (since 2026-10-11): writes go to the real iCloud calendars. (With `live = false`, everything would land in `ZZ-kairos-test` and anything touching other calendars is refused.)

## Tools

| | |
|---|---|
| Read | `list_events`, `search_events`, `find_free`, `list_tasks`, `list_checkins`, `list_changesets`, `get_changeset` |
| Events | `create_event`, `update_event`, `delete_event`, `move_event`, `share_to_family`; batches: `update_events`, `delete_events`, `move_events` |
| Tasks | `create_task`, `update_task` (incl. `reopen`), `complete_task`, `create_deadline`, `start_task`, `log_work`, `plan` |
| Check-ins | `list_checkins`, `checkin` (done / partial + `actual_min` / longer + `actual_min`, `remaining_min`, `not_before` / missed / finished) |
| Projects | `list_projects` (time spent: measured Claude sessions in its repository plus checked-in blocks of its tasks, overlaps counted once), `create_project` (registry names only), `update_project` (status active / next / paused / done). Time is set aside for a project by a task for it (`create_task` with `locus` or `project`) |
| Undo | `undo` |

## Workflows

### "Add a B4 problem sheet due Friday 17:00, about 4 hours"
1. `create_task {title: "B4 problem sheet", locus: "b4", estimate_min: 240, due: "2026-10-16T17:00"}`
2. The response includes the ⚑ deadline changeset and the plan: blocks placed (and any shortfall).
3. Reply with the blocks ("Mon 14:00–16:00, Tue 09:00–11:00") or the shortfall and options.

### Bulk edits ("move the room codes in the lecture titles into the location field")
1. `search_events {keys: ["Studies"], query: "LR"}` (and `"Software Lab"` etc.) → rows with uid/title/location.
2. Build each new title/location; `update_events {summary: "Move room codes from lecture titles into location", updates: [{uid, title, location}, …]}`.
3. Response `pending` (his events, > 8 ops) → "185 lectures queued as cs-…; approve in locus's Approve, or on your phone". One `undo` reverts all.

### "Delete the lectures for modules I don't take" (+ a document/screenshot)
1. Read the document; list the modules he takes.
2. `search_events {keys: ["Studies"]}` → group rows by module code (B1, B10, …).
3. Show: "Keeping B1, B2, B10…; removing B12 (8), B18 (8)… = 40 events." Wait for OK unless he was explicit.
4. `delete_events {summary: "Remove lectures for modules not taken: B12, B18, …", events: [{uid}, …]}` → one approval, one undo.

### Moving one of Alec's events
`update_event {uid, start: "…"}` → `pending` with reasons. Tell Alec it's queued and where to approve.

### Weekly review (Sunday)
1. `list_events {from: today, to: +7d}` — skim the week.
2. **Triage Personal:** `search_events {keys: ["Personal"], to: +14d}` → suggest keys; on Alec's OK, one `move_events` batch.
3. **Deadlines:** `list_tasks` → check dates/estimates are still right; `update_task` as needed.
4. `plan` → summarise blocks, moved count, shortfalls.
5. `list_changesets {status: "pending"}` → remind Alec what's waiting.
6. One-paragraph summary of the week ahead.

### Daily
1. `list_events` for today + `list_tasks` → "Today: … ; work blocks: …; due soon: ⚑ …".
2. `list_checkins` → mention blocks still waiting ("Mon 19:30 Purchase Furniture hasn't been checked in").

### "I only managed 20 minutes of the furniture last night"
1. `list_checkins` → the block (Mon 19:30–20:30 Purchase Furniture).
2. `checkin {block_uid, outcome: "partial", actual_min: 20}` → the block is trimmed to 20 min with ✓, and the 40 min left are planned again; reply with where they went.
3. "That took 3 h, not 2, and I need about another hour — but not before Saturday" → `checkin {block_uid, outcome: "longer", actual_min: 180, remaining_min: 60, not_before: "sat"}`.
4. Missed entirely → `outcome: "missed"`. Did it and the task is done → `outcome: "finished"`. Worked on something with no block → `log_work {id, minutes}`.

### "I'm starting the B4 sheet now"
`start_task {id: "B4"}` → a block from now; the task's other blocks reflow.

### Project time
"Set aside time for locus this week" → `create_task {title, locus: "locus", estimate_min}` then `plan`. "What should I work on?" → `list_projects`: active ones with least time this week, and next ones waiting.

### Undo
`list_changesets {status: "applied"}` → `undo {id}`. If the event was edited elsewhere since, undo refuses with a diff — show it to Alec.
