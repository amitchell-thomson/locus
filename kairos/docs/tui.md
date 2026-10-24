# The kairos TUI

`kairos` (or `kairos tui`). `?` shows the keys in the app; this page also explains what's drawn.

## Views and moving around

Arrows and Tab, as in the locus window (spec K6, 2026-10-24; `hjkl` are gone).

| Key | |
|---|---|
| `Tab` / `Shift-Tab` | week → day → month → agenda (and back) |
| `←` `→` / `Shift-←` `Shift-→` | day / week back, forward |
| `↑` `↓` | select the previous / next item; past the first or last, scroll the day an hour |
| `t` | today |
| `Enter` | details; expands duplicates ⧉ |
| `w` | week: Mon–Sun ↔ Mon–Fri (wider columns) |
| `1` `2` `3` | show/hide Studies · Projects · Careers |
| `4` · `5`–`8` | show/hide all of Personal · its streams Family, Sport, Social, Errands |
| `p` `f` | show/hide everything of yours · Family |
| `/` `r` `q` | search · sync now · quit |

**Every hour is the same height** (two rows). A taller window shows more hours, never taller ones;
a day that doesn't fit shows `▲` / `▼` beside the hours and scrolls with `↑` / `↓` past the first or
last item. It opens on the selected item, else now, else the first thing that day.

**The calendars are the locus areas** (ADR-008): Studies, Projects, Careers, and Personal as its
streams. Old names still work wherever you type a calendar (`Engineering` is Studies, `Personal`
or `Inbox` is Errands), and an area name means all of it (`kairos week --key Personal`).

## Changing things

Your own edits here apply straight away (no approval); `u` undoes the last one, including the replan after it.

| Key | |
|---|---|
| `a` | add: `e` event · `t` task (a form; `Tab` between fields) |
| `e` · `K` · `m` | edit · change calendar · move. On a ⚑ these act on the task's due date |
| `d` | remove: delete an event · skip a work block (kairos plans that time elsewhere) · on a task's ⚑, remove the task with its due date and all its planned time (past, checked-in blocks stay) |
| `s` | share an event to Family (asks first) |
| `B` | going or not. Your own events: not going greys it and frees the time for the scheduler. Family events: whether it blocks your time |
| `y` | approve a pending change |
| `n` | start the selected task now: a block from now, the rest reflows |
| `S` | run the scheduler |
| `T` | tasks: `D` done · `W` log work · `+` more time · `o` reopen · `e` edit · `n` start · `S` plan |
| `A` | changes waiting for approval (from Claude or scripts) |
| `P` | projects (below) |

**A due date is the task's own** (spec K5): the ⚑ on its day is drawn from the task, never written
to a calendar. `kairos deadline "Apps close" fri 17:00 --key Careers` makes a task with only the
date. Task types: sheet, coursework, admin, chore (a short errand for a gap).

## Check-ins

A work block or project session that has ended waits for you to say how it went: no answer is decided for you. The status bar shows **N to check in · C** while anything waits. Blocks nobody answers count as done after a week (`notify.autodone_after_days`).

| Key (on a past block) | |
|---|---|
| `y` | done: the block gets ✓ |
| `c` | partly: asks how many minutes; the block is trimmed to that and the rest of the time is planned again |
| `x` or `d` | missed: the block is removed and its time planned again |
| `F` | done, and the whole task is finished |
| `+` | took longer: a form for the time you actually spent, how much more you think it needs (0 = finished), and optionally a date to plan it from. The ✓ block grows to the time spent (into free time), and the rest is planned again |
| `C` | every block waiting for a check-in, any day |
| `W` | log work that wasn't in a block (a ✓ block ending now) |

The same answers work from the phone (evening push → check-in page, including "took longer"), `kairos checkin` (`l` = took longer), or by telling Claude (MCP `checkin` with `outcome: "longer"` / `log_work`).

## Projects (`P`)

Your projects, grouped **Active** · **Next** · **Paused** · **Done**, each with its note and the time
spent: this week, last 30 days, when you last worked on it.

| Key | |
|---|---|
| `a` | add a project · `e` edit · `s` next status · `d` delete |

**Time on a project is measured, not planned** (spec K6). It comes from your Claude sessions: a
span opens at a prompt you typed in a session whose folder is that project's repository and runs
until it goes quiet for ten minutes. It is drawn after the fact as a faint bar down the right edge
of the day (`worked · locus` beside it in the day view), with nothing to check in. The Mac sends its
sessions on every `kairos sync`; kairosd measures the server's every five minutes. To set time aside
for a project, place an ordinary work block for one of its tasks; it gets the usual check-in. Proposed
time (◌) and the session diamonds are gone.

## Reading a work block

- **Left edge:** `┆` kairos may move it · `╏` you placed it (pinned) · `▌` ordinary events and anything in the past.
- **Box (tasks):** 󰄱 to do · 󰡖 over, waiting for a check-in · 󰄵 done. (Stored titles use ☐ / ✓ so the phone shows them too.)
- Titles are italic. A block starting off the grid (e.g. 10:15) shows a dim `:15`; a title cut short beside a tag ends in `…`.
- **Bottom edge**, on the selected block: for a future block `task: X/Y done by then · Z left after`; for a past or current block `task: X/Y done · Z left`. Key hints sit on the right.
- **Selection:** the selected item is filled in its own colour; other blocks of the same task get a lighter tint; pending approvals stay blue.
- Greyed: an event you aren't going to (`B`), a Family event you haven't opted into, or tentative.
