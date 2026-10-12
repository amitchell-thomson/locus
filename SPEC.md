# Build prompt: `kairos` — terminal calendar + task scheduler (Rust)

> Paste this whole file into Claude Code at the root of a new, empty repo (or save it as `SPEC.md` and tell Claude Code: "Read SPEC.md and start with plan mode for Phase 0").
> The name `kairos` (Greek: *the opportune moment*) is a placeholder — rename freely. Do **not** name the binary `cal`: it collides with the Unix `cal` utility.

---

## 0. Who you are working for, and how

You are building a personal calendar system for Alec (Oxford Engineering student, macOS laptop + always-on Linux home server `compute-node`, iPhone). He supervises; you implement. He is technical (Python, some Rust exposure, Neovim/tmux/chezmoi user) but will not read every line — **tests and acceptance criteria are how he checks your work**, so they must be real.

Working rules:

1. Work strictly **phase by phase** (§9). At the end of each phase: run all tests, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, then stop and give a short report (what was built, what was tested, known gaps, anything you need from Alec). Do not start the next phase until he says so.
2. Start each phase in plan mode. Ask questions when a decision is genuinely his; pick sensible defaults otherwise and state them.
3. **Safety:** nothing may write to iCloud before Phase 5. From Phase 5 on, live writes require `--live` (or `live = true` in config); integration tests run only against a dedicated iCloud calendar named `ZZ-kairos-test`, never real calendars.
4. No `unwrap()`/`expect()` in non-test code paths that touch user data. Errors use `thiserror` (lib) / `anyhow` (bins).
5. Maintain a `CLAUDE.md` in the repo with architecture notes, invariants, and how to run tests, updated each phase.
6. Calendar bugs are **semantic** (recurrence, timezones, all-day dates, dedup) — the compiler won't catch them. Every such rule in this spec needs a test.

---

## 1. Goal and non-goals

**Goal:** a calendar Alec will actually use. That means:
- iCloud stays the source of truth for events → iPhone Calendar, iOS notifications and Siri keep working.
- A fast, uncluttered ratatui TUI with two overlays (**Personal**, **Family**), key-coloured, duplicates hidden.
- A task layer: tasks with estimates + deadlines get **scheduled automatically** into free time as calendar blocks (visible on the phone).
- Claude (Claude Code now; claude.ai later) can add/move/remove events and tasks and replan, with **low-risk changes auto-applied and high-risk changes requiring Alec's accept/reject**.
- Passive visibility: tmux status segment, `today` summary at shell start.

**Non-goals (v1):** replacing Calendar.app/iOS Calendar; writing to Google or Exchange calendars; a native CalDAV client (vdirsyncer does sync); multi-user anything; invitations/RSVP handling.

---

## 2. Environment facts (current state)

Apple Calendar currently aggregates four accounts:

| Account | Calendar | Decision |
|---|---|---|
| iCloud | `Privat` (default) | Rename → `Personal`; becomes the **inbox/catch-all** key and iOS default calendar |
| iCloud | `Family` (Family Sharing, shared) | **Family overlay.** Read-only except the "share to family" operation (§6) |
| Google Workspace (oqts.org) | `oqts@oqts.org` (shared society calendar) | Read-only import → key **OQTS** |
| Google Workspace | `alec@oqts.org` | Read-only import → key **OQTS** |
| Google Workspace | Holidays in the UK | Read-only **background layer** (non-blocking, hidden by default) |
| Exchange (Oxford) | `Calendar`, `Birthdays`, `timetable_MT24_1st_Year`, `timetable_HT25_1st_Year`, `Extracurriculars` | Not synced. Old timetables deleted; current timetable imported into **Engineering** (§9 Phase 0) |
| Google (personal Gmail) | `alec.mitchellthomson@gmail…` (incl. Gmail auto-added events, e.g. bookings) | Read-only import → **Imported** layer (dimmed); "adopt" action copies an event into a key calendar |
| Google | Holidays in Austria | Dropped (Alec can re-enable as a background layer) |
| Local | Siri Suggestions, Scheduled Reminders | Disabled (not CalDAV-visible anyway) |

**Personal keys = separate iCloud calendars** (Alec creates these in Phase 0):
`Personal` (inbox), `Engineering`, `Careers`, `Sport`, `Travel`, `OQTS`, `Social`.

Rationale (do not "improve" this into a label/CATEGORIES scheme): iOS Calendar cannot display or set `CATEGORIES`, so labels would make the phone a second-class client and phone-captured events would be unlabelled. Accepted costs: one key per event; re-keying = moving between calendars (handled via CalDAV delete+create, preserving UID where iCloud allows). Free-form tags may additionally live in `CATEGORIES` for TUI search only.

Machines: macOS laptop, `compute-node` (Linux, always on). Network between them and from the iPhone: **Tailscale** (confirm with Alec in Phase 1; if absent, set it up). Dotfiles: chezmoi with OS-conditional templates. Theme: Catppuccin Mocha. Time zone: Europe/London.

---

## 3. Architecture

```
iPhone / Calendar.app ─┐                         (capture on the go, notifications)
                       │ CalDAV
                 iCloud (truth for events)
          ┌────────────┴─────────────┐
   vdirsyncer (read-write)     vdirsyncer (READ-ONLY, read_only = true)
   on compute-node              on Mac, every 2–5 min (launchd)
          │                            │
   ~/.local/share/kairos/vdir     ~/.local/share/kairos/vdir
          │                            │
   kairosd (server daemon)        kairos TUI / CLI (read local vdir;
    - single writer to iCloud      writes go to kairosd over Tailscale)
    - task store (SQLite)
    - metadata sidecar (SQLite)
    - scheduler
    - risk policy + changesets + undo log
    - HTTP API  (/api/...)
    - MCP server (/mcp, Streamable HTTP)
    - ICS fetcher for read-only Google sources
    - ntfy push (approvals, check-ins)
```

Key decisions:

- **Single writer.** Only `kairosd` writes to iCloud (humans on phone/Calendar.app aside). The Mac's vdirsyncer is read-only. This gives one place for the risk policy, changeset log and undo, and avoids two machines racing on ETags. TUI/CLI on either machine send writes to `kairosd`'s API.
- **Mac viewing works without the server.** The Mac pulls from iCloud directly, so if `compute-node` is down Alec can still see everything; writes then fail with a clear message (Calendar.app remains a fallback).
- **After every write**, `kairosd` triggers an immediate `vdirsyncer sync` of the affected collection (don't wait for the timer).
- **Read-only external sources** (Google ×3) are ingested via their *secret iCal addresses* (HTTPS GET, `If-None-Match`/`If-Modified-Since`, every 10 min) — no Google OAuth. Stored in a separate read-only vdir subtree.
- **Metadata sidecar.** kairos-specific fields (created_by, kind, flex, task_id, origin_key, adopted_from) are stored in SQLite keyed by `(calendar, UID)`, and *mirrored* as `X-KAIROS-*` properties on events kairos writes. Phase 1 must test whether iCloud and iOS edits preserve `X-` properties; the sidecar is authoritative regardless, so a phone edit that strips them loses nothing.
- **Workspace crates:** `kairos-core` (model, parse, recurrence, dedup, overlays, risk policy, scheduler — pure, no I/O where possible), `kairos-store` (vdir + SQLite), `kairos-cli`, `kairos-tui`, `kairosd` (API + MCP + jobs).
- **Libraries (suggested, verify current versions):** `icalendar` (parse/write), `rrule` (expansion), `chrono` + `chrono-tz`, `ratatui` + `crossterm`, `rusqlite`, `axum`/`tokio`, `rmcp` (MCP, Streamable HTTP), `reqwest`, `notify` (fs watch), `serde`, `toml`, `clap`, `insta` (snapshot tests), `proptest`.

---

## 4. Data model

**Source**: `{ id, kind: icloud | ics_readonly, path/url, overlay: personal|family|background|imported, key: Option<Key>, writable: bool }` — all from `config.toml`, nothing hard-coded.

**Event (normalised view):** uid, source, key, title, start/end (timed with tz, or all-day `NaiveDate` range with exclusive end), recurrence instance id, location, transparency, alarms, metadata (from sidecar), `kind: event | deadline | task_block`.

**Deadline:** a VEVENT with `X-KAIROS-KIND:deadline`, in the relevant key calendar, title prefixed `⚑ `.
- Date-only deadline → all-day event.
- Timed deadline (e.g. "Fri 17:00") → zero-duration timed event (`DTSTART = DTEND`) with VALARMs at −1 day and −2 h (configurable).
- Deadlines are **non-blocking** for the scheduler.

**Task** (SQLite in kairosd): id, title, key, estimate (minutes), remaining (minutes), deadline (Option<date | datetime>, links to a deadline event), earliest_start, priority (1–3), splittable, min_chunk, max_chunk, preferred_window (e.g. `morning`), status (`open|done|dropped`), notes, created_by.

**Task block:** a VEVENT in the task's key calendar, `X-KAIROS-KIND:task_block`, `X-KAIROS-TASK-ID`, `X-KAIROS-FLEX:true`, `created_by: scheduler`. Visible on the phone like any event.

**Changeset:** id, author (`alec|claude|scheduler`), created_at, ops[] (create/update/delete/move_to_calendar with full pre-image + post-image), risk (`low|high`, computed by policy, never supplied by the caller), status (`pending|applied|rejected|undone|failed`), summary text.

---

## 5. Overlays, keys, dedup

- **Personal overlay** = iCloud key calendars + OQTS imports. **Family overlay** = iCloud Family. **Background** = UK holidays (toggle, off by default). **Imported** = Gmail calendar (dimmed, toggle).
- Each key has a configurable Catppuccin Mocha colour; Family has its own.
- **Dedup is view-only — never delete.** Two event instances are duplicates if:
  1. same UID (and same recurrence-id), or
  2. normalised titles match (case-fold, strip punctuation/emoji/`⚑`, collapse whitespace; then Jaro–Winkler ≥ 0.92 or token-set equality) **and** starts within ±15 min **and** same local date **and** both timed or both all-day.
- Precedence for which copy is displayed: Family > iCloud personal > OQTS import > Imported. The surviving item shows a `⧉` badge and the overlays it represents; a key in the TUI expands the cluster.
- **Must-not-merge tests:** two different lectures at the same time in different rooms with different titles; a recurring event's moved instance vs. its original slot; "Squash" on two different days.

---

## 6. Write operations and risk policy

The policy lives in `kairos-core`, is evaluated by `kairosd` on every changeset, and is **never** decided by the caller (Claude cannot label its own change low-risk).

**Forbidden (rejected outright):** any write to a read-only source (Google imports, holidays); any write to Family except `share_to_family`.

**Low risk → auto-applied, logged, undoable, included in the daily summary:**
- the scheduler creating/moving/deleting **its own work blocks** (`created_by = scheduler`, `flex`, not pinned, no overlap with a fixed event) — **at any time and in any number** (Alec, 2026-10-12: work must move out of the way of new events at once, and can start 15 min from now; these ops are exempt from the 2 h and > 8 ops rules below);
- creating/updating/deleting Claude's flex task blocks where start > now + 60 min;
- creating a single (non-recurring) event in a personal key calendar that doesn't overlap a non-flex event;
- creating/updating tasks and deadlines;
- re-keying an event that Claude/scheduler created.

**High risk → `pending`, needs Alec's accept/reject:**
- modifying, moving or deleting any event **not** created by kairos (i.e. created by Alec on phone/Calendar.app, or pre-existing);
- `share_to_family` (always);
- any recurring-series change (whole series or `THISANDFUTURE`);
- any op touching an event that starts within the next 2 h (except the scheduler's own work blocks);
- creating an event overlapping a non-flex event;
- changesets with > 8 ops (the scheduler's own work-block ops don't count);
- deleting a deadline or dropping a task.

**Approval channels:** TUI approvals pane, CLI (`kairos approve <id>` / `kairos reject <id>`), and ntfy push with Accept/Reject action buttons (Phase 8). **The MCP server has no approve tool.** In Claude Code, Claude may run `kairos approve` via Bash only after Alec says yes, and Claude Code's own permission prompt is the second gate.

**Undo:** `kairos undo <changeset>` restores pre-images; refuses with an explanation if the event has since been edited elsewhere (ETag/sequence mismatch), and offers a diff.

**`share_to_family`:** moves an event from a personal calendar to Family (as Alec specified — move, not copy), stores `origin_key` in the sidecar so the TUI still colours it by key, and preserves alarms. Undo moves it back.

---

## 7. Scheduler (tasks → blocks)

Deterministic Rust code. Claude's job is to interpret intent, create and update tasks/estimates, call the scheduler and explain the result — **not** to hand-place blocks (except when Alec explicitly asks for a specific slot, which then becomes a pinned block).

**Inputs:**
- busy intervals = all events in personal overlays with `TRANSP:OPAQUE`, excluding deadlines, background, Imported, and **Family events** (a family member's appointment shouldn't block Alec's time). Family events can opt in as blocking via sidecar flag; Alec can toggle per event in the TUI;
- `constraints.toml`: profiles `term` and `vacation` (with date ranges), each with:
  - day window (e.g. 08:30–22:00) and sleep protection;
  - meal buffers;
  - max focused hours/day;
  - preferred deep-work windows;
  - buffers around fixed events (default 10 min; 30 min if the location differs from the previous event's);
  - no-schedule periods (e.g. Saturday evening);
  - preferred lead time before a deadline (`deadline_lead_hours`, default 2 h: aim to finish ≥ 2 h before). A soft preference: when there is no room, work falls back to the time up to the deadline itself (never past it) and the plan warns that the task is *squeezed* into the last 2 h;
  - comfort: at most `max_continuous_minutes` (3 h) of back-to-back events + work without a `break_minutes` (30 min) break, and at most `max_day_load_hours` (10 h) of events + work a day (events in `day_load_excludes` — Social, Sport — don't count);
- recovery: no work for `minutes` after events in a `[[recovery]]` key (Sport: 60 min — tired, showering), not even squeezed. Only the scheduler's work is kept out: fixed events (compulsory lectures) stay where they are;
- task types (`[[task_types]]`): `sheet` (5 h default, 2–3 h blocks, ≥ 1 h), `coursework` (estimate required, 2–3 h blocks, revised at check-ins), `admin` (45 min default, one 30–60 min go);
- `weekend_start` (10:00): Saturdays and Sundays start later (task work and proposals);
- `squeeze_block_minutes` (40): the shortest squeezed block (a 1 h gap between lectures minus 10 min travel each side).

**Algorithm (2026-10-12 scheme):**
1. Earliest-deadline-first with slack ordering, priority as tiebreak.
2. **Tiers:** place work in the best tier it fits, a lower one only for what's left: *ideal* (the type's 2–3 h blocks, ≥ 2 h before the deadline, comfortable) → *shorter* (down to the type's minimum block) → *squeeze* (down to 40 min, up to the deadline, first within then past the comfort rules). Every squeezed block is reported with its reasons ("into a short gap", "into the last 2 h before …", "back-to-back with other commitments", "onto an overloaded day").
3. **Spreading:** a task's hours are shared across the days before its deadline in proportion to each day's free time (a sheet due Friday → blocks on two or three days; one free day → all there). Within a day, the best preferred-window / deep-work fit wins, then the earliest. No crumbs: a remainder shorter than any block is absorbed or the split adjusted.
4. Never schedule within the next 15 min (`min_lead_minutes`).
5. **Stability:** on replan, keep already-placed future blocks that remain feasible; only move a block when required (or when a squeeze can be undone). Report how many blocks moved. A block Alec moves by hand becomes pinned.
6. **Infeasibility is a first-class output:** if work doesn't fit, report per-task shortfall ("B4 sheet: 2.5 h short before Fri 17:00") and the options (extend window, drop/lower priority, cut estimate). Never silently overbook.
7. **Learned estimates:** once a type has ≥ 3 finished tasks, remaining time is planned × the median actual/estimate ratio (clamped ×0.7–×2), shown in the plan ("≈ B4 sheet: planning 6 h for 4 h left (sheet tasks usually take ×1.50)").
8. **Proposed project time:** after task work, one session a day (`max_per_day`), from 09:00 (`earliest`), in the day's longest comfortable free gap (1–3 h, within the daily work cap, next 7 days; none on a day with an accepted session — Alec can still add more himself) is offered as greyed "Project time?" blocks in the TUI (kairosd only — nothing written to iCloud). Alec accepts one (`y`, then says what to work on → an event) or drops it (`d`, never proposed there again).

**Start now:** `n` in the TUI on a task (tasks view) or on one of its blocks / its ⚑ (week/day view) puts a pinned block from now (rounded down to the slot) for a full block of the task's type, stopping at the next fixed event or the deadline; it replaces the task's unpinned blocks it overlaps, and the plan reflows.

**Repeating events:** `create_event` takes `repeat` (`every` week|day, `term_only`, `until`). Term-time repeats are one series per term from the `term` profile's dates (RRULE … UNTIL the term's last day), so the vacation is empty and iPhone/iCloud see plain weekly series. Recurring → needs approval unless Alec typed it.

**Completion tracking:**
- Evening check-in (ntfy, Phase 8; TUI prompt before then) lists today's blocks with done / partial / missed.
- With no answer by 09:00 the next day, blocks count as done (configurable).
- Missed or partial time returns to `remaining` and triggers a replan.
- Done time uses up the estimate at the learned rate (with sheets at ×1.5, 90 min of work uses 60 min of the estimate).
- **Tasks never finish by themselves** (2026-10-12): at 0 remaining a task stays open; the check-in on the block that uses it up (and every coursework block) asks "how much is left? (0 = finished)", the TUI tasks view shows "planned time used up — d finished · + more time", the morning summary lists it, and Claude is told to ask.
- Log estimate vs. actual per task (only for tasks Alec finished — not guesses) for the learned estimates.

**Triggers:** task created/updated, any event write through kairosd (right after the request), every sync (phone / Calendar.app / Family changes), check-in results, manual `kairos plan`. Replans produce a changeset; the scheduler's own block ops are always low-risk, so they auto-apply. Coursework check-ins also ask "how much is left?" and replan with the answer.

**Tests:**
- property tests that no block overlaps a busy interval or violates constraints;
- golden scenarios (overloaded week, deadline today, recurring lectures, DST week);
- a stability test: adding an unrelated task shouldn't move more than the minimum number of blocks.

---

## 8. Interfaces

### CLI `kairos`

Commands:
- `today`, `week`, `ls --from --to [--overlay] [--key] --json`, `next --tmux`
- `add "<title>" <when> [--key] [--dur]`
- `deadline "<title>" <when> --key`
- `task add|ls|done|edit`
- `plan [--dry-run]`
- `changes [--pending]`, `approve`, `reject`, `undo`
- `share-family <uid>`, `adopt <uid> --key`
- `sync`, `doctor` (checks sync freshness, daemon reachability, config)

Requirements:
- Natural-ish dates: "tue 19:00", "fri 17:00", "2026-10-23", "tomorrow 9".
- Ambiguity is an error, not a guess.
- `--json` on every read command, with a stable schema documented in `docs/json.md`.

**Performance budgets** (tested in CI with a 5k-event fixture):
- `kairos next --tmux` < 20 ms, served from a cache index maintained by a file watcher / last sync;
- `kairos today` < 50 ms;
- TUI cold start < 150 ms.

### TUI `kairos tui`

- **Views:** week (default) ↔ day on `Tab`, agenda on `g`, tasks `T`, approvals `A`.
- **Keys** (the ones that apply to the selection are shown on the bottom-right edge; `?` lists all):
  - `h/l` day, `H/L` week, `t` today, `j/k` select;
  - `p` Private (all Alec's calendars) / `f` Family, `1–6` one calendar (`b`/`i` only when holiday / imported layers exist);
  - `a` add → `e` event / `t` task; `e` edit, `m` move, `K` re-key, `s` share to Family;
  - `d` = remove, whatever it is: delete an event · skip a work block (the work goes elsewhere, never back into that slot) · remove a task's deadline (from its ⚑) · drop a proposal · drop a task;
  - on a ⚑: `e`/`m`/`d` change the *task's* deadline (not just the event);
  - `y` accept (proposals) / approve; `n` reject — or, on a task / block / ⚑, start it now; `B` Family: attending;
  - tasks view: `e` edit (title, calendar, type, time left, due — empty = no deadline), `D` done, `+` more time, `d` drop, `o` reopen a finished one;
  - `u` undoes Alec's (or Claude's) last change together with the replan it caused; `/` search, `?` help.
- **Rendering:**
  - deadlines: all-day ones in the top strip; timed ones as a `⚑` marker line at their time;
  - work blocks: a dashed left edge `┆` = a scheduler slot (kairos may move it); solid `▌` = an event or a block Alec placed himself;
  - duplicate badge `⧉`; proposed project time greyed `◌ …?`;
  - pending changes shown as ghost blocks with a diff in the approvals pane.
- Task edits carry through: a renamed / re-keyed task renames / moves its blocks and ⚑; finishing a task removes its ⚑ (if still ahead), dropping removes it always.
- Works at 80 cols (collapse to 3-day view) and scales up.
- Snapshot tests with `insta` for layouts.

### Shell & tmux

- tmux status segment: `kairos next --tmux` → e.g. `󰃰 Squash 19:00 (in 2h)`, or `⚑ B4 sheet 17:00` if a deadline is within 24 h.
- `kairos today --compact` for shell startup.
- All configs delivered as chezmoi templates (macOS: launchd; Linux: systemd user units).

### HTTP API + MCP (kairosd)

- **Transport:** Streamable HTTP on the Tailscale interface only, bearer-token auth, **stateless** request handling (no reliance on a long-lived session). Network drops then cost one retry, not a dead server.
- **Supervision:** systemd unit with `Restart=always`, `/healthz`, and structured logs (journald).
- **Why not stdio-over-SSH (the pattern that broke Locus):** an SSH drop kills the stdio pipe and the MCP session with it. **mosh does not fix this.** mosh synchronises terminal screen state over UDP; it is not a byte-stream transport and cannot carry MCP's JSON-RPC. A long-running HTTP service fixes it. The same migration is recommended for Locus.
- **Claude Code registration:** `claude mcp add --transport http kairos http://compute-node:<port>/mcp --header "Authorization: Bearer …"` (verify current CLI syntax).
- **MCP tools:**
  - `list_events(window, overlays?, keys?)`, `find_free(window, duration, constraints?)`
  - `create_event`, `update_event`, `delete_event`, `move_event`, `share_to_family`
  - `list_tasks`, `create_task`, `update_task`, `complete_task`, `create_deadline`
  - `plan(dry_run)`, `list_changesets(status?)`, `get_changeset(id)`, `undo(id)`
- Every write tool returns `{changeset_id, risk, status, summary}`. There is **no** approve tool.
- Tool descriptions must explain the risk policy so Claude sets expectations ("queued for your approval").
- Ship a `docs/claude-usage.md` and a Claude Code skill/CLAUDE.md snippet describing the workflows below.
  - **Weekly review (Sunday):** triage the `Personal` inbox (suggest keys), adopt useful Imported items, check deadlines, run `plan`, summarise the week ahead.
  - **Daily:** report today's plan and handle check-in results.

---

## 9. Phases (stop after each)

**Phase 0 — Cleanup & scaffolding** (mostly Alec, guided by you)
- Produce `docs/migration.md`, a checklist for Alec:
  - delete the MT24/HT25 timetable calendars;
  - turn off Siri Suggestions in Calendar and Siri settings, hide Scheduled Reminders and Birthdays;
  - remove Holidays in Austria;
  - review `Extracurriculars` and move anything future into the new key calendars, then disable the Oxford account's calendar;
  - rename `Privat` → `Personal` and set it as the default calendar on iPhone and Mac;
  - create `Engineering`, `Careers`, `Sport`, `Travel`, `OQTS`, `Social`, `ZZ-kairos-test` in iCloud;
  - fetch secret iCal URLs for the Google calendars;
  - create an iCloud app-specific password;
  - obtain the current-year Engineering timetable as an `.ics` file.
- Cargo workspace, CI script, CLAUDE.md, config schema with example.
- Collect **anonymised golden fixtures** from Alec's real `.ics` exports, plus hand-written edge-case fixtures (below).

**Phase 1 — Sync plumbing**
- vdirsyncer configs: server read-write; Mac `read_only = true`. Passwords come from the macOS Keychain / systemd credentials via `password.fetch` — never plaintext in the repo.
- ICS fetcher for the read-only sources. launchd/systemd timers. chezmoi templates.
- Verify:
  - the Family calendar is discoverable over CalDAV (if not, stop and report options);
  - whether `X-KAIROS-*` survives iCloud round-trip **and** an edit on iPhone (record the result in CLAUDE.md);
  - Tailscale reachability Mac ↔ server ↔ iPhone.

**Phase 2 — `kairos-core` read path**
- Parse, normalise, recurrence expansion in a window, overlays/keys, dedup.
- Required tests:
  - **DST:** a weekly Sunday 19:00 Europe/London event stays at 19:00 local on 18 Oct, **25 Oct 2026** (clocks go back) and 1 Nov; same around **28 Mar 2027**. An event at 01:30 on 25 Oct 2026 (ambiguous local time) is handled deterministically and documented.
  - **All-day:** `DTEND` is exclusive (a single-day event on 23 Oct has `DTEND=24 Oct`); multi-day spans render on every covered day; no off-by-one in non-UTC zones.
  - **Recurrence:** RRULE with COUNT/UNTIL; EXDATE; RECURRENCE-ID override moved to another day (original slot gone, moved instance present); a cancelled instance; `THISANDFUTURE` splits.
  - **Floating times and events in other time zones** (e.g. Europe/Vienna) display correctly in London time.
  - Dedup must-merge and must-not-merge cases (§5).
  - Golden fixtures parse without loss.

**Phase 3 — CLI read commands + tmux/shell integration**, with performance budgets enforced.

**Phase 4 — TUI (read-only)**: all views, toggles, dedup expansion, snapshot tests.

**Phase 5 — kairosd write path**
- HTTP API, sidecar, changesets, risk policy (exhaustive table-driven tests of §6), undo, immediate sync after write.
- Writes go only to `ZZ-kairos-test` until Alec flips `live = true`.
- TUI/CLI writes routed via API; approvals pane.

**Phase 6 — Tasks & scheduler** (§7), deadlines, `kairos plan`, infeasibility reports, completion tracking.

**Phase 7 — MCP server**
- Tools as above, Streamable HTTP, systemd hardening.
- A reconnection test: kill the network or restart the daemon mid-session and the next tool call still succeeds.
- Claude Code registration docs and the usage skill.

**Phase 8 — Phone & notifications**
- Self-hosted or ntfy.sh topic (private, auth) for approvals (Accept/Reject action buttons → API), evening check-ins, and a daily morning summary.
- An **iOS Shortcut** "Add task" (structured prompts: title, key from a menu, estimate, deadline date/time) that POSTs to `/api/tasks` over Tailscale. Export the Shortcut steps in `docs/shortcut.md`.

**Phase 9 (optional) — Remote MCP for claude.ai / mobile**
- Expose `/mcp` publicly via Tailscale Funnel or Cloudflare Tunnel **only with proper OAuth** as required by the MCP authorization spec and claude.ai custom connectors (check current docs).
- This gives write access to Alec's calendar, so no authless or obscure-URL shortcuts.

---

## 10. Definition of done (v1 = Phases 0–8)

- Alec's phone, Mac Calendar.app, and the TUI on both machines show the same events within 5 min of any change.
- No Siri/old-timetable clutter; every visible event is in a key, Family, or a deliberately toggled layer.
- "Add a B4 problem sheet due Friday 17:00, about 4 hours" (via Claude Code or the Shortcut) results in:
  - a deadline event;
  - blocks scheduled before it, visible on the iPhone;
  - an infeasibility warning if they don't fit.
- Moving one of Alec's own events via Claude produces a pending changeset that is accepted or rejected from the phone.
- `kairos undo` reverses any applied changeset.
- All tests, clippy and performance budgets pass; CLAUDE.md is current.
