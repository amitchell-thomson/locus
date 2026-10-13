# The kairos TUI

`kairos` (or `kairos tui`). `?` shows the keys in the app; this page also explains what's drawn.

## Views and moving around

| Key | |
|---|---|
| `Tab` | week → day → month |
| `g` | agenda |
| `h` `l` / `H` `L` | day / week back, forward |
| `t` | today |
| `j` `k` | select the next / previous item |
| `Enter` | details; expands duplicates ⧉ |
| `w` | week: Mon–Sun ↔ Mon–Fri (wider columns) |
| `p` `f` | show/hide Private (all your calendars) · Family |
| `1`–`6` | show/hide one calendar (Personal, Engineering, Careers, Projects, Sport, Social) |
| `/` `r` `q` | search · sync now · quit |

## Changing things

Your own edits here apply straight away (no approval); `u` undoes the last one, including the replan after it.

| Key | |
|---|---|
| `a` | add: `e` event · `t` task (a form; `Tab` between fields) |
| `e` · `K` · `m` | edit · change calendar · move. On a ⚑ deadline these act on the task's deadline |
| `d` | remove: delete an event · skip a work block (kairos plans that time elsewhere) · on a task's ⚑, remove the task with its deadline and all its planned time (past, checked-in blocks stay) |
| `s` | share an event to Family (asks first) |
| `B` | Family event: whether you're attending (blocks the scheduler or not) |
| `y` | accept proposed project time ◌ (asks what you'll work on) · approve a pending change |
| `n` | start the selected task now: a block from now, the rest reflows |
| `P` | run the scheduler |
| `T` | tasks: `D` done · `W` log work · `+` more time · `o` reopen · `e` edit · `n` start |
| `A` | changes waiting for approval (from Claude or scripts) |

## Check-ins

A work block that has ended waits for you to say how it went: no answer is decided for you. The status bar shows **N to check in · C** while anything waits. Blocks nobody answers count as done after a week (`notify.autodone_after_days`).

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

## Reading a work block

- **Left edge:** `┆` kairos may move it · `╏` you placed it (pinned) · `▌` ordinary events and anything in the past.
- **Box:** 󰄱 to do · 󰡖 over, waiting for a check-in · 󰄵 done. (Stored titles use ☐ / ✓ so the phone shows them too.)
- Titles are italic. A block starting off the grid (e.g. 10:15) shows a dim `:15`; a title cut short beside a tag ends in `…`.
- **Bottom edge**, on the selected block: for a future block `task: X/Y done by then · Z left after`; for a past or current block `task: X/Y done · Z left`. Key hints sit on the right.
- **Selection:** the selected item is filled in its own colour; other blocks of the same task get a lighter tint; pending approvals stay blue.
- ◌ marks proposed project time (only in kairos until you accept it with `y`, or drop it with `d`).
