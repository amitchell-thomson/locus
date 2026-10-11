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

- compute-node: repo at `~/server-projects/kairos`; `kairosd` + `kairos` in `~/.cargo/bin`; `systemctl --user` unit `kairosd` **running with live = false**, bound to `100.117.10.28:7420` (Tailscale only). vdirsyncer 0.21 in `~/.local/share/kairos/venv` (symlinked into `~/.local/bin`). Secrets in `~/.config/kairos/secrets/` (api-token and ntfy-topic generated; iCloud + Google secrets still needed from Alec).
- Mac: nothing installed yet (templates in `deploy/chezmoi`, steps in `docs/deploy.md`).
- Update the server: `cd ~/server-projects/kairos && git pull && cargo install --locked --path crates/kairosd && cargo install --locked --path crates/kairos-cli && systemctl --user restart kairosd`.

## Phase status

| Phase | Status |
|---|---|
| 0 Cleanup & scaffolding | Code done. Alec: `docs/migration.md`; real exports → golden fixtures. |
| 1 Sync plumbing | Configs/units/templates done, daemon deployed. Needs iCloud creds; then `scripts/verify-icloud.sh` (Family discoverability, X-KAIROS round-trip, phone-edit check). |
| 2 Core read path | Done (25 fixture-based semantic tests + unit tests). |
| 3 CLI + tmux/shell | Done; budgets met (5k events: next 8.5 ms, today 8.9 ms, TUI first frame 4.6 ms on M-series, release). |
| 4 TUI | Done (all views, toggles, dedup expansion, ghost blocks, check-in, snapshots). |
| 5 Write path | Done (risk table tests, changesets, undo, live guard, approvals in CLI/TUI/phone links). |
| 6 Tasks & scheduler | Done (EDF+slack, chunking, stability, shortfalls, check-ins, auto-done, estimate log). |
| 7 MCP | Done (stateless Streamable HTTP; restart-mid-session test). Registration: `docs/claude-usage.md`. |
| 8 Phone | ntfy approvals/summaries/check-in page done; Shortcut steps in `docs/shortcut.md`. Needs ntfy app subscribed. |
| 9 Remote MCP (optional) | Not started (needs OAuth). |

## Phase 1 findings (fill in after `scripts/verify-icloud.sh`)

- Family calendar discoverable over CalDAV: _unknown yet_
- `X-KAIROS-*` survives iCloud round-trip: _unknown yet_
- `X-KAIROS-*` survives an iPhone edit: _unknown yet_ (sidecar is authoritative either way)
- Tailscale: Mac ↔ compute-node ✓ (API reachable at http://compute-node:7420); iPhone is on the tailnet (`phone`), reachability to be confirmed from the phone.

## Decisions log

- Own lossless iCalendar parser instead of the `icalendar` crate: round-trip fidelity (unknown props, param quoting) matters for writing back to iCloud.
- Recurrence via `rrule` on naive wall-clock times (UTC pseudo-zone), resolved with our RFC rule — deterministic DST behaviour independent of the crate.
- The CLI parses natural dates locally (good errors), sends RFC 3339 to the daemon.
- `next --tmux` is an inline segment for after the date in `status-right` (thin `U+E0B3` separator, like starship).
- Writes always go through kairosd, even on compute-node, so there's one changeset log.
- Changesets with > 8 ops are high-risk per spec; a big initial plan therefore needs one approval.
