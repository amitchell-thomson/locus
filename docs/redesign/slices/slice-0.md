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

### 0.12 Capture from the cloud (added 23 Oct, approved)
Found during 0.5: nothing has reached the capture staging directory since 1 Aug. The agent on
the tablet that pushed renders to `locus-remarkable-receiver` has stopped (a tablet update is
the likely cause), so handwritten notebooks (Loop A) have not reached the library for twelve
weeks, and Loop B's change detection, which keys on the same renders, has seen nothing either.
Reading marks are still recorded by the hourly cloud sweep. Separately, capture stats every
non-excluded document on each run (about 80 cloud calls every 30 minutes), which fits the HTTP
429s in the logs.
- **Source:** the reMarkable cloud, as reading ink already is. One `rmapi ls -l` per folder for
  modified times; a document whose time moved since its last capture is fetched as a bundle
  (`data/cache/rmdoc/`). No per-document `stat`; back off on 429 and never read a missing
  entry as a deletion (§14).
- **What a document is decides how it is read** (Alec, 23 Oct: "pdfs are source, notebooks are
  my writing, and any pdf claude sends should have my notes AND what my notes where about
  ingested. Notes on their own are often meaningless"). Inside the area folders (`Studies/`,
  `Projects/`, `Careers/`) and at the root (`Jot`):

  | Document | Read as |
  |---|---|
  | a notebook (no PDF in the bundle, `RmDoc.is_notebook`) | his writing: rendered and captured by Loop A as now, area from the folder |
  | a PDF Locus sent (its content hash is recorded at push) | Claude's document, never ingested itself; his ink on it is read as marks, and each note is promoted WITH the passage it was written against (`promote_reading_notes`, the passage quoted as context, never as his words) |
  | any other PDF (a sheet, slides, a paper) | a source, ingested from the bundle's bytes with the folder's area; his ink on it read as marks and promoted the same way |

  The same rule applies to a Locus PDF in `Inbox/`, so ink he writes before filing it is not
  lost. Excluded: `Admin`, `Archive`, `trash`; `Reading` stays Loop B's.
- Every push records the content hash of the PDF it sent, which is what makes the second row
  known rather than guessed. The PDFs already on the device from earlier pushes are recorded
  once, from the push log and the `Inbox` listing.
- **Folder levels** (Alec, 23 Oct: nesting inside courses and projects, e.g. a course's
  submodules from Canvas): level 1 is the area (`Studies`, `Projects`, `Careers`, matched
  case-insensitively); level 2 is the course, repo or application, matched to the registry by
  name (in `Careers`, against an application's `company`, falling back to its name, since a
  firm's folder spans its applications); anything deeper is his own organisation and decides
  nothing. The full device path is
  stored with each capture so the Canvas pull (Slice 7) can map submodules later. A level-2
  folder that matches nothing in the registry keeps its area and is named by `locus status`,
  never filed under nothing silently.
- The receiver unit leaves `deploy/systemd/enabled`; the tablet agent is left where it is.
- Backfill: everything changed since 1 Aug is captured, under the existing per-run
  transcription cap, so the billed catch-up spreads over a few ticks.
- **Check:** a page written in Jot is printed by `locus list` within an hour; a note written
  in the margin of a Locus PDF is printed by `locus` search together with the passage beside
  it, and that PDF's own text is not in the library; a sheet PDF put in a course folder is
  printed by `locus list` as a source; one capture run's journal printed showing its cloud call
  count; a day of `locus-capture` logs with no 429.

### 0.5b Tablet tidy (added 23 Oct; done by Alec by hand, 23 Oct)
**Done:** the root is `Admin Archive Careers Inbox Jot Projects Reading Studies trash`, listed
twice. Level-2 folders not yet in the registry: `Careers/Brevan Howard`, `Careers/Jane Street`,
`Projects/Tanker-flow` (for 0.8's taxonomy). The table below is kept as the record of the plan.

One home per thing, by area, at the root (Alec, 23 Oct: two high-level places for course
material "seems complicated"). Inside a folder, notebooks are his and PDFs are sources or
Claude's documents (0.12), so nothing needs a separate tree.

```
Inbox/      new from Claude, not yet read; he files each into its area once read
Jot         capture notebook
Studies/    one folder per course (exact Canvas names)
Projects/   quast, nabla, locus, oqts
Careers/    general, Jump Trading, G-Research, Millennium
Reading/    In-Progress, Finished, Proposed
Admin/      paperwork
Archive/    retired: the daily pages, old proposals
```

Moves are done with `rmapi mv`, which keeps each document's id, so marks stay attached; nothing
is deleted. Rows marked ? are open until Alec answers them, and nothing moves before he agrees.

| Now | Goes to |
|---|---|
| `Notes/studies/*`, `Notes/careers/*`, `Notes/projects/*` (made in 0.5) | `Studies/`, `Careers/`, `Projects/`; then `Notes/` is removed when empty |
| `Daily/` | `Archive/Daily` |
| `Reading/Proposed` (9 from 31 Jul) | `Archive/Proposed` |
| `Personal/Millennium Criteria` | `Careers/Millennium` |
| `Personal/logo ideas` | ? which project; then `Personal/` is removed |
| `Feedback` (root notebook) | ? |
| `Notes/engineering/3YP` | `Studies/B3 Group Design Project` |
| `Notes/engineering/B4 Information Engineering Systems` | ? `Studies/B14 Information Engineering Systems` if B4 is a typo |
| `Notes/engineering/Modules`, `optimisation` | ? |
| `Notes/oqts/Sponsor Notes`, `Notes/projects/oqts/OXDAQ` | `Projects/oqts` |
| `Notes/projects/pour-runbook` | ? |
| `Notes/jump_trading/*`, `Notes/rough_notes/Jump call` | `Careers/Jump Trading` |
| `Notes/rough_notes/G-Research` | `Careers/G-Research` |
| `Notes/rough_notes/Jane Steet Social` | `Careers/general` |
| `Notes/rough_notes/Recommendations` | ? |
| `Notes/rough_notes/Locus taxonomy` | `Projects/locus` |
| `Notes/brevan_howard/*` | ? `Careers/Brevan Howard` |
| `Notes/quantum_ml/*` | ? `Projects/quast` |
| Inbox: OXDAQ milestone 1, OXDAQ specification v2.1 | `Projects/oqts` |
| Inbox: quantum dot primers (2), QADAPT Decoded, two-qubit gate, barrier control, QUAST physics and people, QUAST dossier | `Projects/quast` |
| Inbox: Jump and Millennium question set, Jump error log v2 | `Careers/Jump Trading` |
| Inbox: Millennium Interview Prep (margin) | `Careers/Millennium` |
| Inbox: Locus design brief v1, plan v2, screens v2, plan v3, information and links, How a deploy reaches the server | `Projects/locus` |
| Inbox: S1 Tracking ICU patients (22 Oct) | stays if unread, else `Studies/B18 Biomedical Modelling and Monitoring` |

The `Notes` root setting (`[capture].notes_root`) and the category map follow the new folders in
0.12, so the order is: 0.12's code deployed first, then the moves, so that no capture run sees
a half-moved tablet with the old rules. **Check:** `rmapi ls` printed for the root and each
folder, listed twice (a rate-limited listing can leave entries out).

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
