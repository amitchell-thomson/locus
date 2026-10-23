# Slice 0 — Plumbing

**State:** approved 2026-10-19, ready to start · **Written:** 2026-10-19
**Plan:** F, row 0; B.6; C.2; C.3; C.5; E.

**Done when (from the plan):** tablet pushes and readback work with no hand steps, the library takes
new documents again, and nothing old runs at night.

---

## Decisions only you can make

**Answered 19 Oct 2026:**
- **D1: a**, off now.
- **D2: b**, harvest off, pull kept until the scout.
- **D3: a**, the locus repo: "it is shared data under a shared contract."
- **D4: a**, today's tools only: "the endpoints may change as we refine things."
- **D5: a**, 0.1 first.
- **D6:** passwords: no change needed ("it does not matter if you know it ... Do not worry"); 0.11 is dropped. Skill upload: when 0.4 lands, explain how to upload it so it works everywhere, and hold a discussion on whether other skills are needed or the MCP carries everything.

The original questions, for the record:

| # | Question | Options | Recommendation |
|---|---|---|---|
| D1 | The old nightly word-matching links (`locus link`) | a. turn off now · b. keep running until the new linking job (Slice 5) | **a.** v4 retires them, and nothing new is built on them. Existing links stay in the DB; search does not need new ones. |
| D2 | Reading discovery (weekly harvest, hourly pull) | a. both off now · b. harvest off, pull kept until the scout · c. both kept | **b.** The harvest accepted 4 of 2,571; the pull is what reads your marks on documents in `/Reading`, which you still use. |
| D3 | Where the shared taxonomy (areas, streams, repository registry) lives | a. a TOML file in the locus repo, served by the HTTP service · b. a separate small repo · c. inside kairos | **a.** One place, versioned with the plan; kairos reads it from the service with a cached local copy, so kairos still works offline. |
| D4 | How far the HTTP service goes in Slice 0 | a. replace the MCP tools only (retrieve, documents, markups, push) · b. also the endpoints later screens need | **a.** Later slices add endpoints when their screens are wired; designing them now would guess at contracts Slice 1 has not settled. |
| D5 | Order of work | a. 0.1 first, then the rest in the order below · b. as listed but with 0.6 (HTTP service) last | **a.** |
| D6 | Your hand steps | re-upload the remarkable-explainer skill to claude.ai after 0.4 (the password change was dropped, see the D6 decision above) | Please do it; the slice cannot close without it. |

---

## Steps

Each step lists its check: the command or observation whose **printed output** proves it done,
which is also the `/goal` condition when the step runs unattended.

### 0.1 One deploy flow — laptop → GitHub → server
Today work happens on the server and the laptop copy was 230 commits behind (synced 19 Oct).
- A `deploy/deploy.sh` run from the laptop: push the branch, then on the server `git pull
  --ff-only`, `uv sync`, restart the locus units that changed.
- Server checkout refuses to deploy over uncommitted changes (prints them and stops).
- **Check:** a trivial commit made on the laptop shows on the server (`git log -1` printed from
  the server) and `systemctl --user status` for the restarted units is printed `active`.

### 0.2 The library takes documents again
Found 19 Oct: last ingest 6 Sep; **no file watcher runs on the server** (no `locus-watch` unit
exists) and four papers dropped on 31 Jul still sit in `data/incoming/paper/`.
- Confirm that is the cause (journal, timer list, last-ingest per path). If it is not, stop and
  report what is.
- Add a `locus-watch` user unit (or fold intake into an existing timer), with the
  `locus-failure@` hook like the others.
- Ingest what is waiting, one process at a time (flock).
- **Check:** document count printed before and after; a test PDF dropped into `incoming/paper/`
  is printed by `locus list` within 15 minutes; no quarantines.

### 0.3 Nothing old runs at night
- Off: `locus-daily.timer`, `locus-daily-pull.timer`; per D2, `locus-discover-harvest.timer`.
- `locus-maintain`: drop `review --enrol`, `--enrol-concepts`, `--write-questions`,
  `--write-concept-questions` (recall) and `intent --act`; per D1, drop `link`. What remains:
  `structure` for newly ingested documents, then backup on its own timer.
- Unit files changed in `deploy/systemd/` (the source), deployed by 0.1; nothing edited only on
  the server.
- The code for the daily page and recall stays in the tree this slice (removal is its own commit
  later in the slice, once the timers have been off for a night without anything breaking).
- **Check:** `systemctl --user list-timers` printed with only capture, maintain, backup,
  discover-pull (and watch); after one night, `journalctl` for maintain printed showing only the
  kept steps.

### 0.4 LaTeX to the tablet
- Fix `reading/tex2pdf.py` around line 571: compiler output decoded without `errors=`, so a
  non-UTF-8 byte in a log crashes the render. Test with a planted byte.
- `to_remarkable` preamble: one column, even margins on all four sides (the v2–v4 documents'
  geometry: 7.07 × 9.43 in, 0.95 in margins), replacing two columns at 9pt.
- `remarkable-explainer` skill (source: `.claude/skills/remarkable-explainer/SKILL.md` in this
  repo) rewritten to match: one column, even margins, no column-width maths advice. Same voice
  rules.
- **Check:** a test explainer pushed with `locus read` arrives in `/Inbox`; its first page
  rendered to PNG and printed shows one column with equal margins; the planted-byte test passes.

### 0.5 Tablet folders
- Create the push folders the plan uses: `/Inbox`, `/Reading` (In-Progress, Finished), `/Jot`,
  and the notes tree `studies/ projects/ careers/ jot/` (plan C.9). Existing folders are left
  alone; nothing is moved or deleted.
- **Check:** `rmapi ls` printed for each.

### 0.6 One HTTP service
Today every chat starts its own `locus mcp` over SSH (2,325 processes for 75 calls), and the
repo's `.mcp.json` points at a server-only path, so locus tools fail on the laptop.
- One long-running service on the server, reachable over Tailscale only, bearer token from an
  env file (never committed); MCP over HTTP for Claude clients, plain JSON endpoints for the TUI
  later. Per D4: the existing tools (`retrieve`, `list_documents`, `inspect_document`,
  `markups`, push) only.
- Every call logged on arrival and completion (the existing `mcp-log` idea).
- `.mcp.json` points at the service URL; the token comes from the environment.
- **Check:** from a laptop Claude session, `retrieve` returns results; the service log printed
  shows the call's start and end; `ps` printed on the server shows one service process, not one
  per call.

### 0.7 Budget logging
- One wrapper for every background `claude -p` job: it records job name, model, input and output
  tokens, cache tokens, duration and exit status from the JSON output, in one table.
- `locus budget` prints today's and this week's totals by job.
- Calibration against `/usage` is yours (one dossier build, one answer, one ink readback) and
  happens when those jobs exist (Slice 2); this step only makes sure every job is counted.
- **Check:** one wrapped test job run; `locus budget` printed showing its row.

### 0.8 kairos on the shared taxonomy
Per D3. Areas: Studies, Projects, Careers, Personal. Streams under Personal: Family, Sport,
Social, Travel with Errands; Studies, Projects and Careers filter as whole areas (plan B.3).
- The taxonomy file in this repo; served by the HTTP service.
- kairos (its own repo): keys migrated from Personal/Engineering/Careers/Projects/Sport/Social;
  the existing events re-keyed in **one** kairos change set, shown to you before it is applied
  (kairos queues anything over 8 operations for your approval anyway).
- **Check:** kairos TUI filters printed/screenshotted showing the new areas and Personal's
  streams; the kairos change set id printed and approved by you.

### 0.9 Secrets scrubber (before 0.10)
- A scrubber applied to every transcript before any job or index reads it: API keys and tokens
  by pattern, private keys, passwords near `password`/`pw` words, the kairos bearer token, and
  a list of known secrets kept in an env file.
- Scrubbed copies are what is stored; originals are never copied off the laptop unscrubbed.
- **Check:** a test corpus with planted secrets of each kind printed before and after; none
  survive; normal text unchanged (diff printed).

### 0.10 Laptop transcripts reach the server
- Laptop `~/.claude/projects/` → scrubbed (0.9) → server, on a schedule (launchd) over
  Tailscale. Server transcripts are read in place.
- **Check:** a new laptop chat appears scrubbed on the server within 30 minutes; path and a
  redacted line printed.

### 0.11 Skills (with you, when 0.4 lands)
Per D6: how to upload the rewritten skill so it works on every machine and client, then a
discussion: which other skills are worth having, versus carrying everything in the MCP tools.

### 0.12 Notebook capture from the cloud (added 23 Oct, approved)
Found during 0.5: nothing has reached the capture staging directory since 1 Aug. The agent on
the tablet that pushed renders to `locus-remarkable-receiver` has stopped (a tablet update is
the likely cause), so handwritten notebooks (Loop A) have not reached the library for twelve
weeks, and Loop B's change detection, which keys on the same renders, has seen nothing either.
Reading marks are still recorded by the hourly cloud sweep. Separately, capture stats every
non-excluded document on each run (about 80 cloud calls every 30 minutes), which fits the HTTP
429s in the logs.
- Capture reads from the reMarkable cloud, as reading ink already does: one `rmapi ls -l` per
  folder for modified times; a document whose time moved since its last capture is fetched as
  a bundle (`data/cache/rmdoc/`) and rendered with `capture/rmdoc` (blank canvas for a native
  notebook) into the staging directory as `<uuid>.pdf`. Loop A and Loop B are unchanged
  downstream. No per-document `stat`; back off on 429 and never read a missing entry as a
  deletion (§14).
- What is captured as his writing: `/Notes/**` and notebooks at the root (`Jot`). Excluded:
  `Inbox`, `Docs`, `Archive`, `Daily`, `Admin`, `trash`; `Reading` stays Loop B's.
- The receiver unit leaves `deploy/systemd/enabled`; the tablet agent is left where it is.
- Backfill: everything changed since 1 Aug is captured, under the existing per-run
  transcription cap, so the billed catch-up spreads over a few ticks.
- **Check:** a page written in Jot is printed by `locus list` within an hour; a margin note
  written on a document in `Reading/In-Progress` is transcribed; one capture run's journal
  printed showing its cloud call count; a day of `locus-capture` logs with no 429.

### 0.5b Tablet tidy (added 23 Oct)
Leave the tablet with one clear home for everything: `Inbox` (from Claude, not yet read), `Jot`,
`Notes` (his writing, by area), `Docs` (documents Claude wrote, filed by area once read),
`Reading`, `Admin` (paperwork), `Archive` (retired: the daily pages, old proposals). Moves are
done with `rmapi mv`, which keeps each document's id, so marks stay attached. The mapping is
agreed with Alec before anything moves; nothing is deleted. **Check:** `rmapi ls` printed for
the root and each folder.

---

## Departures from the plan

- **The library's intake fix is new** (found 19 Oct, after the plan's Slice 0 was written); it is
  0.2.
- **The drop folder is organised by area** (ADR-003, 19 Oct): `~/LocusDrop/` has
  `studies/ projects/ careers/ personal/` in place of the old category folders, mapped onto
  the stored categories; the watcher's repository drops are retired.
- **Ordering:** the scrubber (0.9) must land before transcripts sync (0.10); the plan listed them
  the other way round.
- **Daily-page and recall code removal** is deferred to the end of the slice, after one night
  with the timers off, so that turning them off and deleting them are separate, reversible steps.

## How it will run

One named session (`locus slice 0`) on the laptop. 0.1 first, in chat (it sets up how everything
else deploys). Then 0.2–0.10, each under its own `/goal` with the check above and a bound of 25
turns; any step that needs a decision not covered by D1–D6 stops and asks. 0.8 stops before
applying the kairos change set. Slice 1 (the ratatui prototype) can start in a separate session
at the same time.

## Report

*(written when the slice closes: done, evidence per check, deviations, decisions taken)*
