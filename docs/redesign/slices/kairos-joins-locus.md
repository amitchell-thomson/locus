# kairos joins locus

**State:** done 2026-10-24 (K-e, archiving the old repository, due 31 Oct) · **Written:** 2026-10-24 · **Runs in:** `locus slice 1`
**Decision:** [ADR-008](../../decisions/ADR-008-kairos-joins-locus.md). **From:** Slice 1's
Calendar round (slice-1.md, round 6). **Takes over:** Slice 0's step 0.8 (kairos on the shared
taxonomy), which Slice 0 drops when you confirm it to that session.

**Done when:** kairos is built, tested and deployed from this repository; its calendars are the
areas; a task's due date is an attribute; it reads its projects from the locus registry; its TUI
moves with the arrows and Tab and looks like locus; and locus and kairos show each other's items,
each opening the other.

Underneath that: nothing that works today stops working at any point. kairosd stays the only
writer to iCloud throughout, every change to your calendars is a kairos changeset you can undo,
and the moves that touch iCloud are shown to you before they are applied.

---

## Decisions only you can make

**Answered 24 Oct 2026:** all as recommended (K-a a, K-b a, K-c a, K-e a, with the old repository
archived once this work is done). The Personal stream is named **Errands** ("note errands should
be 'Errands'"). K-d: **Tanker-flow** is a Projects repo ("one of my biggest projects that is just
about finished", on the server at `~/server-projects/tanker-flow`); **Jane Street** is a Careers
entry with its company and no application yet ("just notes from an event I went to, have not
applied yet, may decide to, may not"). Both are in the registry, inactive until there is a view
to give them.

| # | Question | Options | Recommendation |
|---|---|---|---|
| K-a | Where kairos sits in the repository | a. `kairos/` as a directory, its history kept (git subtree), its crates members of the one workspace · b. its crates moved into `crates/` beside locus's | **a.** Keeps its spec, notes and scripts together and its history readable; one workspace either way. |
| K-b | Your Personal calendar's events, when Personal becomes its streams | a. each event shown in one list with a suggested stream (Family, Sport, Social, Errands), approved as one changeset · b. all to Errands, re-file by hand later | **a.** One review, then done. |
| K-c | The registry file | a. `taxonomy/registry.toml` and `taxonomy/areas.toml` in this repo, read directly on the server and through kairosd on the laptop until the HTTP service exists · b. wait for the HTTP service | **a.** Unblocks this and Slice 0's tablet capture (0.12), which matches folders to registry names. |
| K-d | Registry entries the tablet has and the fixtures lack | Careers/Jane Street, Projects/Tanker-flow | Tell me what each is (an application? a past project?) and I add them. |
| K-e | The old kairos repository | a. archived (read-only) after a week of the moved build running cleanly · b. kept | **a.** |

## Rules
- **Nothing that works stops.** Each step leaves kairos building, its tests green, and the
  deployed daemon running. Steps that touch iCloud print the changeset and wait for your
  approval; nothing else writes to a real calendar (kairos's `live` guard stays as it is).
- **History is kept.** kairos's 92 commits come with it.
- **Names are stable.** Registry names are what Slice 0's tablet capture matches on (course names
  exactly as on Canvas); a rename is a decision, not a tidy-up.

---

## Steps

### K1 Move in, nothing else
- `git subtree add --prefix=kairos` from the kairos repository, history kept; its crates join
  the root workspace; its workspace dependencies merged into the root's; toolchain and lints
  reconciled (kairos denies `unwrap`/`expect` outside tests, as locus does).
- Its CLAUDE.md invariants become a section of this repository's CLAUDE.md (single writer, live
  guard, changesets and undo, risk computed never supplied, lossless edits, check-ins wait).
- **Check:** `cargo test --workspace` green, including all of kairos's tests and snapshots;
  `kairos/scripts/ci.sh` passes from the new place; the TUI frame dump identical before and after.

### K2 Deploy from here
- `deploy/deploy.sh` and `deploy/server-sync.sh` gain the Rust build (`cargo install --locked`
  for kairosd and the kairos CLI) and kairosd's systemd unit under `deploy/systemd/` (named in
  the `locus-*` scheme server-sync manages); the Mac's `cargo install` and launchd agent point here.
- The MCP endpoint and phone links are unchanged (same daemon, same port).
- **Check:** on the server, kairosd running from this repository's build; `kairos doctor` clean on
  both machines; a test-calendar write and its undo; a week later, K-e.

### K3 The taxonomy file
- `taxonomy/areas.toml` (areas, Personal's streams, kinds, task types) and `taxonomy/registry.toml`
  (repos, courses, applications, with area and kind, and for an application its company, which
  the tablet's per-firm Careers folders match on), per ADR-006; the fixture registry and the
  contract read from them; kairos reads them (directly on the server, through kairosd's API on the
  laptop, a cached copy offline).
- **Check:** a test that every fixture and kairos config name resolves in the file; Slice 0 told
  the path.

### K4 Calendars become the areas
- Studies (was Engineering), Projects, Careers, and Personal's streams: Family, Sport, Social,
  Errands (new); Personal's own events re-filed per K-b. kairos's keys, filters (`1`–`4`
  areas, streams under Personal) and day-load rule follow.
- **Check:** the changeset printed, approved by you, applied; the TUI's filter row showing the
  areas; a sample of re-filed events printed with their old and new calendars.

### K5 Tasks: one type each, due dates as attributes
- Task types sheet, coursework, admin, chore. A deadline stops being its own calendar object: it is
  the task's due date, drawn as a ⚑ mark on its day, not an event; `kairos deadline` becomes
  `kairos task add --due`. Existing ⚑ events migrate to due dates in one changeset.
- **Check:** kairos's scheduler and deadline tests rewritten to due dates and green; the migration
  changeset approved and applied; the week view showing a due mark.

### K6 The TUI joins locus's look and keys
- `kairos` title on the left of the border, the week kept in the centre.
- Arrows and Tab move (←/→ days, Shift-←/→ weeks, ↑/↓ items, Tab views); `hjkl` removed; `P`
  opens projects (the scheduler's key moves to `S`).
- Not going: any of your own events can be marked not going (greyed slightly, no longer blocking
  the scheduler), as Family events are opted in today.
- Even height: every hour the same height, filled with more hours rather than taller ones; ↑/↓
  at the edge scroll the day.
- Project time: proposed time (◌) and the diamonds go. Worked time is a faint bar after the fact,
  from locus's measure of your Claude sessions (no check-in); a focus block for one project is an
  ordinary work block you place, with the usual check-in.
- **Check:** kairos's snapshots updated and reviewed; frame dumps at the window size before and
  after; the key table in its docs.

### K7 One registry
- kairos's projects are registry entries: each existing kairos project matched to its registry id
  (listed for you to confirm), its time spent kept; new projects come from the registry, not from
  kairos.
- **Check:** the matching list approved; `kairos` projects view and locus's Project view showing the
  same projects.

### K8 Linked both ways
- An event or task can name the locus item it belongs to (an application, a course, a repo), kept
  in kairos's sidecar; an interview on an application's timeline is that event.
- locus reads kairos for Home's next event, the rhythm grid, chores and application events
  (through kairosd's API until the HTTP service exists); anything locus adds to the calendar goes
  through kairosd, never around it.
- Enter on a linked item in kairos opens it in locus (its window, at that item); from locus, an
  event opens in kairos at that day.
- **Check:** an event added in kairos shown in locus, and one added from locus shown in kairos; a
  linked interview opening its application; the single-writer test still green.

### K9 Chores that get done
- Chores are kairos tasks of type chore with a short estimate; the scheduler places them into real
  gaps as short blocks.
- Home: the moment line names one chore only when one fits a gap you are in now, and only in the
  slot the moment line already gives way to lower things (it never pushes out the running count,
  the review, the next event or mail); "waiting for you" lists chores overdue or put off, with how
  many times.
- **Check:** Home dumped at 95, 120 and 170 columns with a chore in a gap: nothing on the top line
  cut or pushed out; a put-off chore in waiting for you.

---

## Order
K1 and K2 first (a move, no behaviour change), then K3 to K5 (the taxonomy, with your approvals at
K4 and K5), then K6, then K7 to K9 (the joining). Each step ends with its check printed; each is a
commit of its own; the slice 1 design rounds carry on around it.

## Report

Run 24 Oct 2026, in one go at your request ("do the entire spec now in one go"). Every step is a
commit of its own; the full suite ends at 355 tests passing, clippy `-D warnings` clean, and
`kairos/scripts/ci.sh` green.

### Each check's evidence

| Step | Commit | Evidence |
|---|---|---|
| K1 move in | 0ec2f9c, 7c06753 | kairos 283/283 tests before and after; `kairos/scripts/ci.sh` ok from `kairos/`; TUI frames (week, agenda, tasks, month at 170x45, on a frozen copy of your data) byte-identical. |
| K2 deploy | e6cd2b4 | `locus-kairosd.service` active on the server, built from this repository; the old `kairosd.service` retired (backup beside it); `kairos doctor` clean on both machines; a write and its undo through MCP on the moved daemon (`cs-189bd00250`, created then undone). |
| K3 taxonomy | 153c159 | `taxonomy/areas.toml`, `taxonomy/registry.toml`; tests that the files load and that an entry outside the taxonomy is named; both fixture sets read them; kairosd serves `GET /api/taxonomy` (test). Slice 0 had the path already. |
| K4 calendars | 312f406 | Live configs converted on both machines (backups `config.toml.bak-2026-10-24-areas`); doctor: "keys are the locus taxonomy (from kairosd)"; the legend reads `1 Studies 2 Projects 3 Careers │ 4 Personal ▸ 5 Family 6 Sport 7 Social 8 Errands`. Re-filing: see below. |
| K5 due dates | f1b2207 | Deadline tests rewritten to due dates and green; the migration tested end to end; on the live calendars there were no ⚑ events, so it proposes nothing; a task's due date drawn as ⚑ in the week view (test). |
| K6 TUI | f20a055 | Snapshots reviewed and updated; frame dumps at your kairos window's 145x40 before and after (kept out of the repository, since they show your calendar): before, 06:00 to 23:00 at one row an hour with 14 empty rows below; after, two rows an hour from 08:00, ▲ marking the hidden early hours. Key table in `kairos/docs/tui.md`. |
| K7 one registry | f171fdb | Live: `cs-323d212cf6` linked quast, locus, nabla, tanker-flow, and oxdaq to OQTS (oxdaq is OQTS's component), and added CV and 3YP; `list_projects` shows the registry's projects with measured time. |
| K8 linked | 91133b0, 9c78e36 | Live: your 16 Oct "Millennium Interview", linked with `kairos edit --locus millennium` (kairos's own record, no iCloud write), appears on Millennium's timeline (19 events → 20); Home's next event and the rhythm grid (12.9 h this week then) come from kairosd (`--dump --live`). The single-writer tests still green. |
| K9 chores | 1b5b9b2 | Home at 95, 120 and 170 columns with a chore that fits now: nothing on the top line cut or pushed out (at 95 there is no room, so it is not named; at 120 by name; at 170 with its minutes); a put-off chore in waiting for you at all three. |

### What went differently

- **K2's write-and-undo check** used a project (kairos's own record) rather than the test
  calendar: the test calendar was removed from the live config on 13 Oct.
- **K4: iCloud calendars were not renamed.** kairos's keys are its own names for them, and the
  two whose names changed are now pinned to their collection id, so renaming them on iCloud (for
  the phone) is safe and optional. Your Personal calendar had only ten events, all past: the
  Millennium recruitment talk (16 Jun) to Careers is **waiting for your approval**
  (`cs-397358ab1c`); "Travel to Oli's" to Social **applied at once**, because kairos rates moving
  its own events as low risk, so it was not shown to you first (undo: `cs-5c021a872d`). The rest
  are errands and stay in Errands, which is the old Personal calendar. No event was suggested for
  Family: moving one there shares it with your family.
- **K6: details moved to `v`**, since Enter opens a linked item in locus (K8).
- **K6: worked time is measured from Claude Code's transcripts** in kairos-store, a stand-in for
  locus's own session measure (Slice 3's hooks). Time worked through the project view's `n`
  (a session you place) still counts after its check-in, so the two can overlap.
- **K7's matching was applied rather than listed first.** Every match is exact (an id, a name, or
  a component's name) and one changeset undoes it all. Only aulos (done) matches nothing.
- **K9: "put off" counts missed check-ins.** A skipped block is not recorded against its task.

### Decisions taken (yours to overrule)

- Personal's digit is 4 and Family 5; the streams are 6 to 8 (`[tui] personal_index`,
  `family_index`).
- Old key names stay as aliases (Engineering = Studies, Personal and Inbox = Errands), so done
  tasks, scripts and habits keep working with no data migration.
- Proposed project time is off by default, with its code and tests kept.
- A due date with nothing to plan (`kairos deadline`) is a task with estimate 0.
- The locus window lays the live calendar over the fixtures only in the running window and with
  `--dump --live`, never in tests.

### Still to do

- **K-e:** archive github.com/amitchell-thomson/kairos after a week of the moved build running
  cleanly, so on or after **31 Oct** (`gh repo archive amitchell-thomson/kairos`). The server's
  `~/server-projects/kairos` checkout can go then too.
