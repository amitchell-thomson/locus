# kairos as a locus app: one look, one set of keys, one contract

**State:** proposed, waiting for your approval · **Written:** 2026-10-25 · **Runs in:** `locus slice 1`
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
*(when the steps are done)*
