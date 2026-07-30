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

```sh
cp deploy/systemd/* ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now locus-capture.timer locus-maintain.timer locus-backup.timer \
  locus-daily.timer locus-daily-pull.timer
systemctl --user list-timers | grep locus
```

## What runs when

| Timer | Cadence | Does | Cost |
|---|---|---|---|
| `locus-capture` | every 30 min | `locus capture-sync` — pull reMarkable, transcribe changed notebooks, enrich, ingest | **$0 when nothing changed** |
| `locus-maintain` | 03:30 daily | `locus link`, `locus structure --ingested-since <2 days ago>`, then `locus review --enrol` | ~$0 link (cached) + per new doc; enrol is free |
| `locus-backup` | 02:00 daily | `locus backup` | free |
| `locus-daily` | 05:30 daily | `locus daily` — compose the reMarkable page and push the PDF | free (aggregate-only) |
| `locus-daily-pull` | hourly | `locus daily-pull` — read the annotated page back and route it | **$0 unless the page changed** (hash-guarded) |

**Why a 30-minute capture timer is safe.** `capture-sync` keys each document on a hash of its
rendered raster, so an unchanged notebook is skipped before any model call. The first scheduled run
on a fully-captured staging dir reported `0 captured, 12 unchanged, $0.0000`. Spend tracks how much
you actually write, not how often the timer fires.

**Why `--ingested-since` and not `--since`.** `--since` filters on the AUTHORED date. Handwriting is
dated by the device's `ModifiedClient` — when it was written — so a note authored three weeks ago
but ingested last night would be skipped by `--since` and never structured. `--ingested-since` asks
the question a scheduled run actually has: *what has arrived since I last looked?* Without it the
nightly run either re-bills the whole corpus every night or silently misses new capture. The window
is two days, not one, so a missed timer does not drop a day's work.

## Watching them

```sh
systemctl --user status locus-capture.service     # last run + tail of its output
journalctl --user -u locus-maintain.service -n 50 # what the nightly run did
systemctl --user list-timers | grep locus         # next/last fire times
```

Every run is also journaled in `agent_runs` (crash-safe, with cost), so `locus status` and
`agent/budget.spent_today` see scheduled spend exactly as they see manual spend.

## Not scheduled, deliberately

`locus retitle` and `locus export-obsidian` stay manual: both rewrite derived state wholesale and
are better run when you are watching. Phase 3's daily-page composition will add a fourth timer.
