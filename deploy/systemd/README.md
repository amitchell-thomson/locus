# Scheduled Locus runs (systemd user timers)

Until 2026-07-29 nothing in Locus was scheduled — every verb was hand-run, so the system only
improved when the owner personally drove it. That is the failure mode the agent-layer plan warns
about (§9 longevity guardrails: it has to *replace hunting*, not add a chore). These units are the
plan's "systemd timers" (§10 Triggering).

**User units, not system units.** Everything runs as `alec` and needs that user's environment:
`claude -p` reads the subscription OAuth from `~/.claude`, `rmapi` its token from `~/.rmapi`, and
`locus` the `.env` API key. A system unit would have none of them. `loginctl enable-linger alec`
is required so the timers survive logout — verify with `loginctl show-user alec -p Linger`.

## Install

Not by hand. `deploy/deploy.sh` (run on the laptop, on `main`) pushes, fast-forwards the server
checkout, and runs `deploy/server-sync.sh`, which copies every unit here that differs into
`~/.config/systemd/user/`, removes locus units the repo no longer has, reloads, and makes the set
of enabled units equal to `enabled` in this directory. Turning a timer on or off is an edit to
`enabled` and a deploy. A timer-fired oneshot picks up new code at its next firing and is never
restarted; long-running services (the `.service` lines in `enabled`) restart on every deploy that
moves the code.

## What runs when

| Timer | Cadence | Does | Cost |
|---|---|---|---|
| `locus-watch` | every 5 min | `locus watch --once` — ingest anything dropped into `data/incoming/` | free (local models) |
| `locus-capture` | every 30 min | `locus capture-sync` — pull reMarkable, transcribe changed notebooks, enrich, ingest | **$0 when nothing changed** |
| `locus-backup` | 02:00 daily | `locus backup` | free |

heuris (`locus-heuris`, `locus-heuris-derive`), the HTTP service (`locus-http`), the public
listener (`locus-public`) and kairosd (`locus-kairosd`) are listed in `enabled` with their own
comments. The old engine's units (`locus-maintain`, `locus-discover-pull`, `locus-daily`,
`locus-daily-pull`, `locus-discover-harvest`, `locus-remarkable-receiver`) were deleted with its
code in Slice C (C2).

**Why a 30-minute capture timer is safe.** `capture-sync` keys each document on a hash of its
rendered raster, so an unchanged notebook is skipped before any model call. The first scheduled run
on a fully-captured staging dir reported `0 captured, 12 unchanged, $0.0000`. Spend tracks how much
you actually write, not how often the timer fires.

## Watching them

```sh
systemctl --user status locus-capture.service     # last run + tail of its output
journalctl --user -u locus-capture.service -n 50  # what the last capture runs did
systemctl --user list-timers | grep locus         # next/last fire times
```

Every run is also journaled in `agent_runs` (crash-safe, with cost), so `locus status` and
`agent/budget.spent_today` see scheduled spend exactly as they see manual spend.
