# kairos joins locus

**State:** draft, for your approval · **Written:** 2026-10-24 · **Runs in:** `locus slice 1`
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

| # | Question | Options | Recommendation |
|---|---|---|---|
| K-a | Where kairos sits in the repository | a. `kairos/` as a directory, its history kept (git subtree), its crates members of the one workspace · b. its crates moved into `crates/` beside locus's | **a.** Keeps its spec, notes and scripts together and its history readable; one workspace either way. |
| K-b | Your Personal calendar's events, when Personal becomes its streams | a. each event shown in one list with a suggested stream (Family, Sport, Social, Travel and errands), approved as one changeset · b. all to Travel and errands, re-file by hand later | **a.** One review, then done. |
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
  Travel and errands (new); Personal's own events re-filed per K-b. kairos's keys, filters (`1`–`4`
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
*(written when it is done: each check's evidence, anything that went differently, decisions taken)*
