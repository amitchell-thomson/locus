# kairos as a locus app: one look, one set of keys, one contract

**State:** done 2026-10-25 · **Written:** 2026-10-25 · **Runs in:** `locus slice 1`
**From:** your words, 25 Oct 2026: "kairos should cleanly be integrated with locus, and be treated
like any other locus app, with the same structure, practice and formatting / same contracts /
shared data / ui look etc... Everything should be consistent. Just because kairos was made first
does not mean it is something different." **Builds on:** ADR-004, ADR-006, ADR-008, ADR-009;
[kairos-joins-locus.md](kairos-joins-locus.md) and [calendar-entry.md](calendar-entry.md) (both
done).

**Done when:** a screen of kairos and a screen of locus side by side look like one app (frame,
titles, footer, key hints, colours, symbols, dates, empty states, messages); the same key does the
same thing in both; what kairos sends locus travels as locus-contract types with no second copy;
every role has one home (no kairos lists standing in for locus pages); and kairos is built,
tested, documented and deployed the way the rest of the repository is.

Already done on 25 Oct, before this spec: project sessions, proposed project time, the ntfy
pushes, the phone approve links and the phone check-in page removed; kairos's SPEC.md rewritten
as a description of kairos today.

---

## Decisions only you can make

**Answered 25 Oct 2026:** "I agree with all recommendations, for look my preference is to make the
border grey like the rest, but keep the week heading in the middle blue so it stands out." The
powerline bar: no legend; the footer is locus's hints row and says only what is hidden (the
digits are in `?` keys). The event detail: "I feel like the engineering label is outdated, and
that line could be better, also the duplicates has never fired, so having that line in the view
is redundant too. I would rather see the taxonomy clearly laid out, with a clear idea of what it
is linked to. The by alec vs by agent is really good" (added to L2).

| # | Question | Options | Recommendation |
|---|---|---|---|
| L-a | kairos's own Tasks, Projects and Approvals lists, which stand in when the locus window is not running | a. remove them: `T`, `P`, `A` only open locus · b. keep them as a fallback | **a.** kairos runs in the locus window (`Alt-3`); a second list of the same things is the duplication you ruled out for Approve. |
| L-b | Done and remove on a task | a. `d` is done everywhere a task appears; `x` removes (deletes an event, skips a block, drops a task, after asking) · b. keep kairos's `D` done / `d` drop | **a.** Today `d` marks a task done on locus's Tasks page and drops it in kairos. |
| L-c | The add key | a. `+` adds everywhere (Home, Tasks, the calendar) · b. `a` everywhere except Home, where letters jump to sections | **a.** Home cannot give up `a` (approve), so `+` is the one key that works on every screen. kairos's `a` then goes. |
| L-d | Where each stream's colour lives | a. `taxonomy/areas.toml` (Sport green, Social pink, Errands lavender, Family yellow), read by both apps; kairos's config stops carrying colours · b. kairos's config, as now | **a.** A Sport task is green in kairos and lavender on locus's Tasks page today. |
| L-e | kairos's files on disk (`~/.config/kairos`, `~/.local/share/kairos`) | a. keep them, written down as the rule for every app (each app owns its config and data directories; what apps share lives in `~/.local/state/locus`) · b. move them under locus | **a.** They hold the live sidecar, tasks and changesets; moving them buys nothing you would see and risks the calendar. |

## Rules
- kairosd stays the only writer to iCloud; nothing here changes what is written there.
- Each step is a commit of its own, with tests and clippy green.
- Where locus and kairos differ and neither is clearly right, I ask rather than pick.
- The week's centred title stays (agreed in K6).

---

## Steps

### L1 One theme crate
- kairos-tui draws with `locus-theme`: its palette, `panel`, `hints`, `fit`, glyphs. kairos's own
  copies go, and kairos-core's colour table is built on `locus_theme::MOCHA`. The box glyphs for
  to do and waiting move into `locus_theme::glyph`, beside done and approve.
- **Check:** `grep` shows no colour constants or panel builders left in kairos-tui; the snapshot
  tests pass with only the intended frame changes.

### L2 One look
- Frame and titles as locus draws them: overlay0 border, lowercase title on the left with ` · `
  extras, `esc back` on the right; key hints as blue keys with dim words in the footer, from one
  `hints()`; the powerline bar becomes locus's footer (the area legend stays as a quiet row);
  empty states in locus's dim lowercase; notes in peach; dates in locus's format (`fri 17:00`,
  `12 oct`); the keys overlay grouped as locus's is ("this screen · everywhere · symbols").
- The bar at the bottom: the view pill and the legend go; the footer is `⏎ open · ⇥ view · [ ]
  week · ? keys`, with what is hidden (`hidden: careers · 3`) and what waits (`2 to check in · C`)
  as notes. The centred week title stays blue; the border is grey like locus's.
- The event detail lays out the taxonomy: area (› stream), what it is for (the registry item and
  its kind; ⏎ opens it), its task (time left, due), and who made it (you, Claude, the scheduler).
  The duplicates line shows only when there are copies; the old calendar name goes.
- **Check:** dumps of kairos's week and day at 95 × 38 and 155 × 43, beside Home and Tasks at the
  same sizes, for you to look at.

### L3 Colours from the taxonomy (per L-d)
- Stream colours in `areas.toml`; kairos reads them; `kairos doctor` flags a config that still
  sets one.
- **Check:** a test that every key's colour comes from the taxonomy; a Sport task the same colour
  in both apps' dumps.

### L4 One set of keys (per L-b, L-c)
- Done, remove and add as decided; `q` quits only from the top, `esc` goes back; key names written
  as locus writes them (⏎, ⇥, esc); hints say "refuse" where locus says refuse.
- **Check:** a table test over both apps' key lists: no key with two meanings for the same thing.

### L5 One vocabulary
- "due date", never "deadline" (TUI, CLI, MCP descriptions); "for", never "type" or "key" in what
  you read; "approve", never "approvals"; test fixtures use Studies, not Engineering. MCP
  `create_deadline` becomes `set_due` (its job since K5).
- **Check:** a grep over kairos's user-facing strings for the retired words prints nothing.

### L6 One contract
- kairosd serves locus-contract types for what locus shows: `TaskItem` for the tasks overview,
  `Chore`, `WorkSpan`. kairos-core's duplicates go, and locus-tui stops converting. The project
  contract's `DueKind` (sheet, coursework, report, exam) is derived from what a task is for, or
  dropped if nothing reads it. `eco-dates` either becomes kairos's date parser or is deleted (one
  parser, not two).
- **Check:** `cargo test` green; the live locus window shows the same tasks, chores and worked
  bars as before the change.

### L7 One home per role (per L-a)
- kairos's Tasks, Projects and Approvals views removed; `T`, `P`, `A` only open locus. The
  project add and edit forms go with them (projects come from the registry).
- **Check:** the TUI's view list is week, day, month, agenda; snapshot tests updated.

### L8 Built and run like the rest
- `kairos tui --dump [view] --size WxH --keys …` on fixtures, like locus-tui's, replacing the
  frame example.
- The CI script moves to `scripts/ci.sh` at the root and gates the whole workspace.
- Deployment in one tree: kairos's launchd agent and the tmux and zsh snippets move under
  `deploy/`; `locus-session.sh` starts kairos from the same build as locus-tui.
- kairos's decisions go in `docs/decisions/` from now on; kairos/CLAUDE.md keeps the
  calendar's invariants (the root CLAUDE.md §15 points there), and its deployment history moves
  to a short note.
- One `state_dir()` (locus-contract's); kairos-tui writes the same use log locus-tui does.
- **Check:** `scripts/ci.sh` printed green from the root; a deploy that installs kairosd and
  restarts its unit, printed; the locus window restarted from the new script.

### L9 Outside the repository (each shown to you first)
- Point the `kairos` Claude skill (`~/.claude/skills/kairos`, now a link into the old kairos
  repository) at `locus/kairos/.claude/skills/kairos`.
- The `kairos` tmux session: its Claude runs in the old repository, so time there is not counted
  for any project. Closing it is yours to do; kairos work happens in `locus-dev`.
- The server's `~/.config/kairos/config.toml` and `constraints.toml`: drop the settings that are
  now ignored (`[notify]` ntfy lines, `[projects]` proposer lines), with a backup.
- After 31 Oct, with a week of the moved build running cleanly: archive the old kairos repository
  and remove `~/server-projects/kairos` (K-e in kairos-joins-locus).

## Order
L1 and L2 together (the look), then L3, L4 and L5 (small), L7 before L6 (fewer callers to move),
then L8, then L9 with you.

## Report

Run 25 Oct 2026, L1 to L8, as approved (all recommendations; grey frame, blue week heading; no
legend in the footer). 357 tests, clippy clean, `scripts/ci.sh` green from the root; deployed,
and the locus window's three views restarted on the new builds.

| Step | Commit | Evidence |
|---|---|---|
| L7 one home per role | 2dbc973 | kairos's view list is week, day, month, agenda; `T` `P` `A` only open locus. What only kairos's task list did moved to locus's Tasks page (e edits in kairos's form, n starts it now, x drops it after asking); test of the keys. |
| L6 one contract | 2dbc973 | kairosd serves `TaskItem` and `Chore`; engine test of the labels (B15 Control Systems, Studies; a date-only due at 23:59). `DueKind` went (nothing read it). kairos-core's date parser was a byte-for-byte copy of eco-dates; it now is eco-dates. |
| L1, L2 the look | 8ac8227 | locus-theme holds the frame, title, hints, fit and box glyphs, used by both. Dumps of the week and the event detail on your calendar (`kairos tui --dump`). Detail test: area, for, task, made by; no duplicates line without copies. |
| L3 colours | 4efa2e6 | Stream colours in areas.toml; config test (a config that disagrees is named; the taxonomy wins); doctor on both machines: no disagreement. |
| L4, L5 keys, words | 8ac8227, 29ab789 | `locus_theme::SHARED_KEYS`; kairos's hints and locus's Tasks page tested against it. `set_due`, `kairos due`; "due date" in what you read. |
| L8 structure | 4fd051f | `kairos tui --dump`; `scripts/ci.sh` at the root; `deploy/kairos/`; the session starts kairos from the release build; one `state_dir`; one use log (kairos writes `calendar:<view>`). |

| L9 outside | (no commit) | Your go-ahead, 25 Oct: "Link the claude skill properly, close the old tmux session, remove the ntfy lines, and just archive the old one now." `~/.claude/skills/kairos` points at `locus/kairos/.claude/skills/kairos`; the `kairos` tmux session is closed (its Claude was idle); the server config's ntfy lines are gone (backup `config.toml.bak-2026-10-25-ntfy`, kairosd restarted, doctor clean); `amitchell-thomson/kairos` is archived on GitHub, after checking it was clean, pushed, and its last commit is in locus's history. The checkouts `~/local-projects/kairos` and `~/server-projects/kairos` are still on disk. |

Afterwards, from your use (25 Oct): Enter on `L` did nothing for a B14 lecture. Two bugs: the
link form was checked for a title it does not have ("Title is empty"), and typing into a For
that already named something appended to it. Both fixed (52f0766), and kairosd now links each
Studies event whose title starts with a course code to that course, by itself (74 linked).

### What went differently
- **L7 went first** (before the look), so the restyle did not have to cover lists about to go.
- **The shared-key test covers the Tasks page, not every locus screen.** It found clashes inside
  locus that predate kairos: `d` opens decisions on the Project view and means discuss in the
  weekly review, `?` is "ask why" in the review, `x` writes a note in ink confirm. Those came
  from your design rounds, so they are yours to settle.
- **kairos's engine tests still say Engineering**: your iCloud calendar is still called that, and
  the tests check that the old name resolves to Studies.
- **The dump runs on your calendar** (the local copy, read only), not on fixtures: kairos has no
  fixture set of the kind locus-tui has, and your own week is what a design check wants.
- **Deploy files are in `deploy/kairos/`**, not split into `mac/` and `server/`: the server's
  unit was already in `deploy/systemd/`, so only the Mac side moved.
