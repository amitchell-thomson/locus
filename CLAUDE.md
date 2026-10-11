# kairos — notes for Claude

Terminal calendar + task scheduler on top of iCloud. Spec: `SPEC.md`. Alec checks work through tests and acceptance criteria, so every semantic rule has a test.

## Layout

| crate | role |
|---|---|
| `kairos-core` | Pure logic, no network/SQLite. `config` (schema + validation), `constraints`, `ics` (lossless RFC 5545 parser/writer), `time` (DST rules), `normalize` (VEVENT → series), `expand` (recurrence), `dedup`, `view` (overlays/filters), `dates` (natural input), `risk` (§6 policy), `sched` (§7 scheduler), `build` (writing events), `changeset`, `task`, `api` (shared request/response types). |
| `kairos-store` | Disk + network for reads: `vdir` (vdirsyncer layout, atomic writes), `load`, `cache` (instance index behind `next`/`today`/TUI), `db` (SQLite: sidecar, tasks, changesets, check-ins), `feeds` (Google ICS conditional GET), `client` (blocking kairosd client). |
| `kairosd` | Single writer. `engine` (intents → changesets → risk → apply/approve/reject/undo; tasks; plan; check-ins), `http` (axum API + signed phone links), `mcp` (rmcp, stateless Streamable HTTP), `notify` (ntfy), `jobs` (sync, feeds, morning/evening pushes, auto-done). |
| `kairos-cli` | `kairos` binary (all commands, `--json`, `next --tmux`, `doctor`, `sync`, `tui`). |
| `kairos-tui` | ratatui front end (`app` = state/keys, testable; `ui` = drawing). |
| `xtask` | `cargo xtask anonymise`, `cargo xtask check-fixtures`. |

Data flow: iCloud ⇄ vdirsyncer (rw on compute-node, **read-only** on the Mac) ⇄ `~/.local/share/kairos/vdir` → `cache::Index` → CLI/TUI. Writes: CLI/TUI/MCP/Shortcut → kairosd HTTP → engine → vdir file → `vdirsyncer sync <pair/collection>` → iCloud. Google feeds → `~/.local/share/kairos/feeds/<id>.ics`.

## Invariants (don't break these)

- **Single writer**: only kairosd writes to iCloud. The Mac's vdirsyncer remote storage is `read_only = true`.
- **`live = false` by default**: new events are redirected into `test_calendar` (`ZZ-kairos-test`, intended key kept as `X-KAIROS-ORIGIN-KEY`); ops touching any other calendar are refused (`EngineError::LiveDisabled`). Checked at propose *and* apply.
- **Risk is computed, never supplied** (`risk::assess` over facts the engine gathers). Order: forbidden → high triggers → low. No approve tool in MCP.
- **Alec's own interactive edits skip approval** (Alec's request, 2026-10-11): the TUI and the CLI *on a TTY* send `x-kairos-interactive: 1`; kairosd then uses `risk::assess_direct` (forbidden rules only). Non-interactive callers (scripts, Claude via Bash — no TTY) and MCP (always `Author::Claude`, never direct) get the full policy. The header is trusted like the bearer token: it guards against agents, not against someone holding the token.
- **Every write is a changeset** with full pre/post images. Apply checks preconditions (item unchanged since proposal, semantic comparison); undo refuses with a diff if edited elsewhere. Semantic comparison ignores DTSTAMP/LAST-MODIFIED/SEQUENCE/X-* so iCloud's cosmetic rewrites don't block undo.
- **Dedup is view-only.** Never deletes; `dedup_never_drops_instances` test.
- **All-day DTEND is exclusive.** Ambiguous local times = first occurrence; non-existent = pre-gap offset (RFC 5545 §3.3.5, `time::resolve_local`).
- **Recurrence is evaluated in wall-clock time of the master's zone**, then resolved — 19:00 stays 19:00 across DST.
- **Lossless edits**: `build::edit_file` keeps unknown props (X-APPLE-*, attendees, alarms). Adopt strips ATTENDEE/ORGANIZER (iCloud would email invites).
- **Sidecar is authoritative** for kairos metadata; `X-KAIROS-*` is a mirror. `blocking` (Family events blocking the scheduler) is sidecar-only and preserved on rewrites.
- **Scheduler never overbooks**: shortfalls are reported per task. The engine plans new blocks ≥ 2 h 05 m ahead (the policy's "imminent" horizon) so normal replans auto-apply; blocks already inside that horizon are kept and counted as in progress.
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

## Deployment state (2026-10-11)

- compute-node: repo at `~/server-projects/kairos`; `kairosd` + `kairos` in `~/.cargo/bin`; `systemctl --user` unit `kairosd` **running with live = true since 2026-10-11**, bound to `100.117.10.28:7420` (Tailscale only; the server's own CLI uses that IP because `compute-node` resolves to 127.0.1.1 there). vdirsyncer 0.21 in `~/.local/share/kairos/venv` (symlinked into `~/.local/bin`), read-write. Secrets in `~/.config/kairos/secrets/` (api-token, ntfy-topic, apple-id, icloud-app-password, ics-gmail).
- Mac: `kairos` via `cargo install`; vdirsyncer (brew), remote `read_only = true`; launchd agent `com.kairos.sync` (`~/Library/LaunchAgents`, rendered from the chezmoi template by hand, every 180 s, log `~/Library/Logs/kairos-sync.log`). Keychain: kairos-api-token, kairos-icloud, kairos-apple-id, kairos-ics-gmail. chezmoi, tmux and zsh snippets NOT applied (Alec's call).
- Update the server: `cd ~/server-projects/kairos && git pull && cargo install --locked --path crates/kairosd && cargo install --locked --path crates/kairos-cli && systemctl --user restart kairosd`.

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
| 7 MCP | Done (stateless Streamable HTTP; restart-mid-session test). Registration: `docs/claude-usage.md`. |
| 8 Phone | ntfy approvals/summaries/check-in page done; Shortcut steps in `docs/shortcut.md`. Needs ntfy app subscribed. |
| 9 Remote MCP (optional) | Not started (needs OAuth). |

## Phase 1 findings (`scripts/verify-icloud.sh`, 2026-10-11)

- Family calendar discoverable over CalDAV: **yes** (collection id is a 64-hex hash; 225 items). Also discovered: `Erinnerungen ⚠️` (legacy reminders, VTODO only, ignored).
- `X-KAIROS-*` survives iCloud round-trip: **yes**
- `X-KAIROS-*` survives an iPhone edit: **yes** (title edit on iOS; iOS adds TRANSP:OPAQUE). Undo of the edited event correctly refused as a conflict.
- Tailscale: Mac ↔ compute-node ✓ (API reachable at http://compute-node:7420); iPhone is on the tailnet (`phone`), reachability to be confirmed from the phone.

## Decisions log

- Catch-all key renamed `Personal` → **`Inbox`** (Alec's iCloud calendar is named Inbox, CalDAV id `home`). Events without a key go to key 1 (`Config::default_key`), not a hard-coded name. "personal" remains the *overlay* name.
- OQTS Google Workspace feeds dropped: secret iCal addresses are disabled by the Workspace admin and the calendars were empty. OQTS work lives in the iCloud `OQTS` key.

- Own lossless iCalendar parser instead of the `icalendar` crate: round-trip fidelity (unknown props, param quoting) matters for writing back to iCloud.
- Recurrence via `rrule` on naive wall-clock times (UTC pseudo-zone), resolved with our RFC rule — deterministic DST behaviour independent of the crate.
- The CLI parses natural dates locally (good errors), sends RFC 3339 to the daemon.
- `next --tmux` is an inline segment for after the date in `status-right` (thin `U+E0B3` separator, like starship).
- Writes always go through kairosd, even on compute-node, so there's one changeset log.
- Changesets with > 8 ops are high-risk per spec; a big initial plan therefore needs one approval (unless Alec typed it interactively).
