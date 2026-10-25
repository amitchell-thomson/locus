# kairos — notes for Claude

Terminal calendar + task scheduler on top of iCloud. Spec: `SPEC.md`. Alec checks work through tests and acceptance criteria, so every semantic rule has a test.

## Layout

| crate | role |
|---|---|
| `kairos-core` | Pure logic, no network/SQLite. `config` (schema + validation), `constraints`, `ics` (lossless RFC 5545 parser/writer), `time` (DST rules), `normalize` (VEVENT → series), `expand` (recurrence), `dedup`, `view` (overlays/filters), `dates` (natural input), `risk` (§6 policy), `sched` (§7 scheduler), `build` (writing events), `changeset`, `task`, `api` (shared request/response types). |
| `kairos-store` | Disk + network for reads: `vdir` (vdirsyncer layout, atomic writes), `load`, `cache` (instance index behind `next`/`today`/TUI), `db` (SQLite: sidecar, tasks, changesets, check-ins), `feeds` (Google ICS conditional GET), `client` (blocking kairosd client). |
| `kairosd` | Single writer. `engine` (intents → changesets → risk → apply/approve/reject/undo; tasks; plan; check-ins), `http` (axum API), `mcp` (rmcp, stateless Streamable HTTP), `jobs` (sync, feeds, measured project time, registry, auto-done). No phone pushes or pages (removed 2026-10-25). |
| `kairos-cli` | `kairos` binary (all commands, `--json`, `next --tmux`, `doctor`, `sync`, `tui`). |
| `kairos-tui` | ratatui front end (`app` = state/keys, testable; `ui` = drawing). |
| `xtask` | `cargo xtask anonymise`, `cargo xtask check-fixtures`. |

Data flow: iCloud ⇄ vdirsyncer (rw, compute-node only) ⇄ server vdir. The TUI draws kairosd's own index (`GET /api/index`, ETag; polled every 3 s and right after a write), falling back to the local copy offline; the Mac's local copy (`sync.full_cmd` = rsync of the server's vdir, launchd every 60 s) → `cache::Index` → CLI/tmux/offline TUI. Writes: CLI/TUI/MCP/Shortcut → kairosd HTTP → engine → vdir file → `vdirsyncer sync <pair/collection>` → iCloud. Google feeds → `~/.local/share/kairos/feeds/<id>.ics`.

## Invariants (don't break these)

- **Single writer**: only kairosd writes to iCloud. The Mac only reads (kairosd's index, or an rsync of its vdir).
- **`live = false` by default**: new events are redirected into `test_calendar` (`ZZ-kairos-test`, intended key kept as `X-KAIROS-ORIGIN-KEY`); ops touching any other calendar are refused (`EngineError::LiveDisabled`). Checked at propose *and* apply.
- **Risk is computed, never supplied** (`risk::assess` over facts the engine gathers). Order: forbidden → high triggers → low. No approve tool in MCP.
- **The scheduler's own work blocks are low-risk at any time and in any number** (Alec, 2026-10-12; `risk::is_scheduler_block_op`: scheduler-created, flex, unpinned, no fixed overlap). That's what lets work move out of the way of new events at once and start 15 min from now. Pinned blocks (hand-moved ones) and everything else keep the normal rules.
- **Alec's own interactive edits skip approval** (Alec's request, 2026-10-11): the TUI and the CLI *on a TTY* send `x-kairos-interactive: 1`; kairosd then uses `risk::assess_direct` (forbidden rules only). Non-interactive callers (scripts, Claude via Bash — no TTY) and MCP (always `Author::Claude`, never direct) get the full policy. The header is trusted like the bearer token: it guards against agents, not against someone holding the token.
- **Every write is a changeset** with full pre/post images. Apply checks preconditions (item unchanged since proposal, semantic comparison); undo refuses with a diff if edited elsewhere. Semantic comparison ignores DTSTAMP/LAST-MODIFIED/SEQUENCE/X-* so iCloud's cosmetic rewrites don't block undo.
- **Dedup is view-only.** Never deletes; `dedup_never_drops_instances` test.
- **All-day DTEND is exclusive.** Ambiguous local times = first occurrence; non-existent = pre-gap offset (RFC 5545 §3.3.5, `time::resolve_local`).
- **Recurrence is evaluated in wall-clock time of the master's zone**, then resolved — 19:00 stays 19:00 across DST.
- **Lossless edits**: `build::edit_file` keeps unknown props (X-APPLE-*, attendees, alarms). Adopt strips ATTENDEE/ORGANIZER (iCloud would email invites).
- **Sidecar is authoritative** for kairos metadata; `X-KAIROS-*` is a mirror. `blocking` (Family events blocking the scheduler) is sidecar-only and preserved on rewrites.
- **Scheduler never overbooks**: shortfalls are reported per task; squeezes (short gaps, deadline lead, past the 3 h / 10 h comfort limits) are reported per block. Tiers ideal → shorter → squeeze; spread across days by free time; no work in `[[recovery]]` time after Sport. New blocks start ≥ `min_lead_minutes` (15) ahead.
- **Check-ins wait for Alec** (2026-10-13): nothing is decided the next morning; unanswered blocks are presumed done by the scheduler and only *recorded* as done after `notify.autodone_after_days` (7). Partly done = block trimmed with ✓, the rest replanned; missed = block removed, time replanned.
- **A due date is the task's attribute, never an event** (spec K5, 2026-10-24): no ⚑ event is written; the ⚑ mark is drawn from the task (`task::due_marks`, added to the index when it is read: kairosd's for the TUI, the CLI's local cache from the tasks `kairos sync` keeps). `kairos deadline` / MCP `create_deadline` set a task's due date or make a task with only the date. ⚑ events from before are retired when their date changes, and `migrate_deadlines` proposes the rest for approval at start. Removing a task's ⚑ in the TUI still drops the task and its planned time.
- **Tasks finish only when Alec says so** (complete, or "0 left" at a check-in). Remaining is in estimate units; done time is divided by the learned ratio. Auto-done blocks never close a task or feed the estimate log.
- **Replan on event changes**: `apply` sets `replan_soon` for any non-scheduler event write; the HTTP layer replans right after answering, the jobs loop on every tick (and after every sync). Stable, so it's a no-op unless work is displaced — or time was freed: busy spans are compared with the last plan's (`kv plan.busy_spans`), and freed time re-flows unpinned blocks ≥ 2 h ahead (`PlanInput.reflow_after`, Alec 2026-10-14).
- **Project time is measured, and set aside by planning a task for the project** (spec K6, 2026-10-24; sessions and the proposer removed 2026-10-25). Worked time comes from Claude Code transcripts (`kairos-store::worked`: a span opens at a prompt typed in a session whose folder is a registry repo and closes after 10 quiet minutes), sent by the Mac's `kairos sync` and measured on the server by kairosd every 5 minutes (`POST /api/worked`, kept per machine in `kv`), drawn as a faint bar (`EventKind::Worked`, never a calendar event, never busy, no check-in). A project's time spent (`Engine::projects`) is the measured spans and the checked-in blocks of its tasks **merged as spans of time**, so a block worked through Claude counts once. There are no project sessions, no proposals and no diamonds: `retire_sessions` (at start) turns an old session's `project_id` into `Meta.locus` (the project's registry id) and drops the `proposals` table; `[projects]` keeps only `key` and ignores the old settings.
- **Your own events can be "not going"** (spec K6): `Meta.not_going`, sidecar-only like `blocking` and preserved on rewrites; greyed, not busy for the scheduler (which replans).
- **Projects are the locus registry's repos** (spec K7, 2026-10-24): a project task carries `registry` (an id in `taxonomy/registry.toml`); `sync_registry` (start, and every 5 minutes) links projects by id, name or component name and adds one for every repo without; `create_project` accepts only registry names; a linked project shows the registry's name.
- **Links to locus** (spec K8): `Meta.locus` / `Task.locus` name a registry item (sidecar-only, validated against the registry). Enter on a linked item asks the locus window to open it (`locus_contract::link`, the `open-locus` file in locus's state directory, then `tmux switch-client`); kairos opens the day locus asks for (`open-kairos`).
- **Keys are the locus areas** (ADR-008, spec K4): Studies, Projects, Careers, and Personal's streams Sport, Social, Errands (`area = "Personal"`); Family stays the shared overlay. Earlier names resolve as `aliases`; an area name in a filter means all its keys; `inbox` is where keyless events go. `kairos doctor` checks the keys against `taxonomy/areas.toml`.
- **Projects are tasks** (2026-10-15): `kind = "project"`, `stage` = active/next/paused while open (done = task done, delete = drop); `project::Project` is the API/TUI view. `db.tasks()` excludes them (task list, scheduler, `find_task`); `db.projects()` lists them. Project changes are changesets (undo; Claude dropping an open one needs approval). A task belongs to a project by `project_id`; its checked-in blocks count as the project's time. Check-ins on sessions recorded before 2026-10-25 (`checkins.task_id` = the project id) still count.
- **Day load (10 h) excludes Social and Sport** (`day_load_excludes`); the 3 h continuity rule counts everything.
- **A task says what it is for; how it is planned follows from its estimate** (ADR-009, spec E1/E2, 2026-10-25): forms ask For (type-ahead over the registry and Personal's streams; the key follows from the item's area; `L` sets it afterwards) and no type; `planned_type` derives the shape (an hour or less one go, a short Personal one a chore, longer 2–3 h blocks, a course's sheet 5 h). A task for a repo belongs to its project.
- **Prep follows its interview** (spec E4): `Task.before` is an event uid; due = its start; `follow_before` re-reads it before every plan. A task for an application with no due date binds to that application's next linked event.
- **Nudging** (spec E3): Shift-↑/↓ 30 min, Shift-←/→ a day; weeks are `[` `]`. `T` and `P` open locus's Tasks page and Home's projects when the locus window runs (spec E7).
- **Chores take the smallest gap** (spec K9): task type `chore` (~20 min, one go) is scored into the smallest free gap it fits and away from deep-work hours; `GET /api/chores` feeds locus's Home.
- No `unwrap()`/`expect()` outside tests (workspace clippy lints deny them). `thiserror` in libs, `anyhow` in bins.

## Running things

```sh
scripts/ci.sh                      # fmt --check, clippy -D warnings, all tests, perf budgets (release)
cargo test -p kairos-core          # semantics: DST, all-day, recurrence, dedup, risk table, scheduler (incl. proptests)
cargo test -p kairosd              # engine end-to-end (definition-of-done flows), HTTP, MCP (+ restart mid-session)
cargo test -p kairos-tui           # insta snapshots; review changes with `cargo insta review`
cargo test --release -p kairos-cli --test perf -- --nocapture   # 5k-event budgets
cargo xtask anonymise fixtures/raw fixtures/golden --salt <random>
cargo run -p kairos-tui --example frame -- <config> <rfc3339-now> <w> <h> [keys]   # render one TUI frame as text
```
`KAIROS_NOW=<rfc3339>` fixes "now" for the CLI (tests, screenshots). `KAIROS_CONFIG` overrides the config path.

## Deployment state (2026-10-24: built and deployed from the locus repository)

Since 2026-10-24 kairos lives in the locus repository (`kairos/`, ADR-008) and reaches the server
only through locus's `deploy/deploy.sh`: `deploy/server-sync.sh` runs `cargo install --locked` for
kairosd and kairos when Rust sources moved, and manages the unit as `locus-kairosd.service`
(`deploy/systemd/`). The old `kairosd.service` was retired then (backup kept beside it). On the Mac,
`cargo install --locked --path kairos/crates/kairos-cli` from the locus checkout. The notes below
predate the move; paths to `~/server-projects/kairos` are the archived repository.

### Before the move (2026-10-13)

- compute-node: repo at `~/server-projects/kairos`; `kairosd` + `kairos` in `~/.cargo/bin`; `systemctl --user` unit `kairosd` **running with live = true since 2026-10-11**, bound to `100.117.10.28:7420` (Tailscale only; the server's own CLI uses that IP because `compute-node` resolves to 127.0.1.1 there). vdirsyncer 0.21 in `~/.local/share/kairos/venv` (symlinked into `~/.local/bin`), read-write. Secrets in `~/.config/kairos/secrets/` (api-token, ntfy-topic, apple-id, icloud-app-password). The `test` (ZZ-kairos-test) source was removed from the live configs 2026-10-13 (backup `config.toml.bak-2026-10-13`); re-add it before ever setting `live = false`.
- Mac: `kairos` via `cargo install`; the local copy is an rsync of the server's vdir (`sync.full_cmd`; vdirsyncer is no longer used on the Mac); launchd agent `com.kairos.sync` runs `kairos sync` every 60 s (log `~/Library/Logs/kairos-sync.log`). Keychain: kairos-api-token, kairos-icloud, kairos-apple-id. chezmoi, tmux and zsh snippets NOT applied (Alec's call); `deploy/chezmoi/…/config.toml.tmpl` renders to the Mac's live config.
- Phone: ntfy app subscribed, iPhone reaches kairosd over Tailscale (Alec, 2026-10-13).
- Update the server: `deploy/deploy.sh` from the laptop's locus checkout (was: a pull and two `cargo install`s in `~/server-projects/kairos`).

## Phase status

| Phase | Status |
|---|---|
| 0 Cleanup & scaffolding | Done; migration done by Alec 2026-10-11. Golden fixtures from real exports still to do. |
| 1 Sync plumbing | Done and verified (see findings). Emptied collections handled: kairos's own deletes push with `--force-delete` for that collection; collections emptied on iCloud are followed (`kairos_store::sync`). |
| 2 Core read path | Done (25 fixture-based semantic tests + unit tests). |
| 3 CLI + tmux/shell | Done; budgets met (5k events: next 8.5 ms, today 8.9 ms, TUI first frame 4.6 ms on M-series, release). |
| 4 TUI | Done (all views, toggles, dedup expansion, ghost blocks, check-in, snapshots). |
| 5 Write path | Done (risk table tests, changesets, undo, live guard, approvals in CLI/TUI/phone links). |
| 6 Tasks & scheduler | Done (EDF+slack, chunking, stability, shortfalls, check-ins, auto-done, estimate log). |
| 7 MCP | Done (stateless Streamable HTTP; restart-mid-session test). Check-ins, log work, start, reopen and project-time tools added 2026-10-13. Registration: `docs/claude-usage.md`. |
| 8 Phone | Done, then cut back (2026-10-25, plan B.4: "just noise"): ntfy pushes, approve buttons and the check-in page removed; the phone shows the iCloud calendars and takes events, and the Shortcut adds tasks. |
| 9 Remote MCP (optional) | Not started (needs OAuth). |

## Phase 1 findings (`scripts/verify-icloud.sh`, 2026-10-11)

- Family calendar discoverable over CalDAV: **yes** (collection id is a 64-hex hash; 225 items). Also discovered: `Erinnerungen ⚠️` (legacy reminders, VTODO only, ignored).
- `X-KAIROS-*` survives iCloud round-trip: **yes**
- `X-KAIROS-*` survives an iPhone edit: **yes** (title edit on iOS; iOS adds TRANSP:OPAQUE). Undo of the edited event correctly refused as a conflict.
- Tailscale: Mac ↔ compute-node ✓ (API reachable at http://compute-node:7420); iPhone (`phone`) ✓ (confirmed by Alec 2026-10-13).

## Decisions log

- Project sessions, proposed project time, the ntfy pushes and the phone pages removed (2026-10-25, your call and plan B.4): one way to set time aside for a project (a task for it), one place to check in (the calendar), one place to approve (locus's Approve). Project time no longer double counts a block worked through Claude.

- Keys (2026-10-24, superseding 2026-10-12): the locus areas (see Invariants). Before:
- Keys (2026-10-12): **Personal** (1, default, lavender; iCloud calendar renamed from Inbox, CalDAV id `home`), Engineering (2), Careers (3, includes OQTS), **Projects** (4, maroon `#eba0ac`; accepted project time — `[projects] key`), Sport (5), Social (6). Travel and OQTS were deleted on iCloud (empty), and ZZ-kairos-test is gone from iCloud too (only used when `live = false`). Events without a key go to key 1 (`Config::default_key`), not a hard-coded name. "personal" is also the *overlay* name. MCP instructions list the keys from config.
- Gmail and UK-holidays feeds removed too (Alec, 2026-10-11): nothing upcoming in Gmail; holidays not wanted. No `ics_readonly` sources are configured now; the feature stays.
- OQTS Google Workspace feeds dropped: secret iCal addresses are disabled by the Workspace admin and the calendars were empty. OQTS work goes in Careers.

- Own lossless iCalendar parser instead of the `icalendar` crate: round-trip fidelity (unknown props, param quoting) matters for writing back to iCloud.
- Recurrence via `rrule` on naive wall-clock times (UTC pseudo-zone), resolved with our RFC rule — deterministic DST behaviour independent of the crate.
- The CLI parses natural dates locally (good errors), sends RFC 3339 to the daemon.
- `next --tmux` is an inline segment for after the date in `status-right` (thin `U+E0B3` separator, like starship).
- Writes always go through kairosd, even on compute-node, so there's one changeset log.
- Changesets with > 8 ops are high-risk per spec; a big initial plan therefore needs one approval (unless Alec typed it interactively).
