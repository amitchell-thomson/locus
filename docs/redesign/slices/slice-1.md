# Slice 1 — Every screen, on fixtures

**State:** approved 2026-10-19, in progress · **Written:** 2026-10-19
**Plan:** F, row 1; B.4; D (all of it, incl. D.1 branch graph, D.2 symbols); G (first bullet);
R5, R9, R11. Runs beside Slice 0.

**Done when (from the plan):** you would be glad to keep it open, with fake data.

Underneath that, for this spec: every screen the plan names runs in the `locus` tmux window on
fixture data, in your symbols and colours, at 95 × 38; the open questions in plan G's first
bullet are settled in rounds with you at the screen; each screen's data is frozen as a typed
contract that a later slice's backend is built to fill.

---

## Decisions only you can make

**Answered 19 Oct 2026:** D1–D6 all as recommended (a, a, a, a, a, a). Runs in the session that
wrote the spec, renamed `locus slice 1`.

The original questions, for the record:

| # | Question | Options | Recommendation |
|---|---|---|---|
| D1 | Where the TUI code lives | a. a Cargo workspace at `tui/` in this repo (crates `locus-tui`, `locus-contract`) · b. its own repo, like kairos and keryx | **a.** The contracts belong in the shared root (plan D), which Slice 0 D3 made this repo; the TUI and its contracts change together through every round. The repos merge later anyway (B.1). |
| D2 | One look across locus, kairos and keryx (Mocha palette, borders, the D.2 glyph table) | a. a small `eco-theme` crate in `eco`, beside `eco-dates` (renamed 19 Oct, see Departures); locus uses it now, kairos and keryx move to it when next touched · b. copy kairos's theme into locus | **a.** The glyph table and palette are a shared contract in exactly the way `eco-dates` already is (keryx pins it by rev). A copy drifts: kairos's done glyph is already mislabelled in its own comment. |
| D3 | The contract's form | a. Rust types (serde) in `locus-contract`, JSON Schema exported from them; fixtures are JSON files validated against it; the Python service (Slice 0's HTTP service) tests its output against the same schema · b. pydantic models as the source, Rust generated from them | **a.** The TUI is the consumer and is built first; the screens define the data (plan D), so the types should live where the screens are. |
| D4 | Where fixture data comes from | a. hand-curated from your real projects: real names, real branch histories extracted once from git, real decision-log entries; no transcript text · b. generated from transcripts | **a.** Realistic without waiting for the secrets scrubber (Slice 0 step 0.9), and nothing private sits in a committed file. Stage labels and "where you are" lines are written by hand, since change records do not exist yet. |
| D5 | Which screens | a. every screen in screens v2, **plus** the ones the plan added after it and never drew: the learning system (goals, syllabus, a topic track, practice sheets), the ink-confirm view (C.5), the budget view (E), and the Decisions and Answered additions from 19 Oct · b. screens v2 only; the rest designed in their own slices | **a.** "the interface should be one of the first things we build in full" (18 Oct). The learning system is a key feature and has no screen at all yet. |
| D6 | Images in the terminal: the ink-confirm crop (C.5), and later scoreboard plots | a. real images through the kitty graphics protocol (Ghostty supports it; tmux needs `allow-passthrough on`) via `ratatui-image` · b. half-block or braille approximations only | **a**, proved in step 1.4 before any screen depends on it; if it misbehaves inside tmux, fall back to **b** and say so in that round. A crop of your handwriting rendered in braille is not legible enough to choose a passage from. |

---

## Rules for the prototype

- **Nothing leaves the process.** Every action key does its visual part (an item marks read,
  moves, or is struck) and the footer says what it *would* have done: `would send to tablet:
  "Why SMC over the flow"`. The only real effects are tmux: Alt-1..4 and the Project view
  following your project window. Fixture state resets on restart, or with `R`.
- **One size first.** Every screen is laid out for 95 × 38. At a larger size (the laptop, where
  locus has its own workspace) lists show more rows and nothing else changes, until you have seen
  it there and asked for more.
- **Keys are the plan's** (D): a letter focuses a section, Enter opens detail, Esc goes back,
  `/` searches, `?` shows keys. Alt-1..4 switch views (kairos owns the bare digits).
- **Looks are kairos's** (R11): Catppuccin Mocha, transparent background, rounded borders, blue
  focus, ` · ` separators, `…` truncation; one colour per area, matching kairos; colour adds
  meaning on top of the glyph, never instead of it (D.2).
- **Design mode** (plan D, a proposal): every screen carries a `live` flag. In this slice none is
  live, so the prototype is all design mode; later slices flip their screens on, and daily use
  hides any screen that is not.
- **Use is logged** (R9): every screen opened and key pressed goes to
  `~/.local/state/locus/tui-use.jsonl`, local only. `locus-tui --use-report` prints counts per
  screen and key; it is read at the start of each round, so we see what you actually reached for.

---

## Steps

Each step lists its check: the command or observation whose **printed output** proves it done.
Steps 1.1–1.4 are mechanical and can run under `/goal` (bound: 25 turns each). Step 1.5 is
design work: plain chat, you at the screen, never `/goal` (ADR-002).

### 1.0 The screens v2 source into the repo
`screens.tex` and `mock.py` exist only in `.v3work/` (git-excluded). The plan cites them as where
every screen is drawn.
- Copy both to `docs/redesign/screens/`, with the generated `mock/*.txt`.
- **Check:** `git ls-files docs/redesign/screens` printed.

### 1.1 Scaffold
- Per D1/D2: the Rust workspace (`locus-tui` binary, `locus-contract` library), and
  `locus-theme` in `locus-shared` with the palette, border style, `fade`, and the D.2 glyph table as named
  constants (`ATTENDED`, `NEEDS_YOU`, …), one source for every glyph.
- The app shell: a screen stack, section focus by letter, Enter / Esc / `/` / `?`, the footer
  (open · log · search), the use log, `R` to reset.
- `locus-tui --dump <screen> [--size 95x38] [--fixtures <set>]` prints a screen as text, which
  is what every later check uses; snapshot tests (insta, as kairos does) on the dumps.
- **Check:** `cargo test` printed green; `locus-tui --dump home` printed; a test asserting every
  dumped line is at most the requested width.

### 1.2 The `locus` tmux session
- A script that creates session `locus` with four windows: `locus-tui home`,
  `locus-tui project`, `kairos-tui`, `keryx-tui`.
- Alt-1..4 bound to those windows **only while the locus session is the current one**, so they
  never fire inside a project session.
- The follow hook (B.4): a tmux hook writes the session your project window switched to into a
  state file; the Project view reads it and changes project, unless you are scrolling or typing
  in it or have pinned it with `m`. Session names map to projects in the fixture registry
  (`locus-dev` → locus).
- The tmux lines go in a snippet in this repo; your `tmux.conf` sources it, and that one-line
  edit is shown to you before it is made.
- Tested on a separate tmux server (`tmux -L locus-test`) so nothing moves on your screen.
- **Check:** `tmux -L locus-test list-windows -t locus` printed; on the test server, switching a
  second client from `quast` to `nabla` changes the Project view's header, shown by
  `capture-pane -p` before and after; with `m` set, it does not.

### 1.3 Fixtures and contracts, first cut
Per D3/D4.
- The fixture registry: areas and streams (from Slice 0's taxonomy file, or a copy of its
  content if 0.8 has not landed), projects quast, nabla, locus, OQTS (oxdaq, website), the CV as
  a light Careers project, the courses, and the opportunities you name when this step starts.
- Branch graphs extracted once from the real git histories of quast, nabla, locus and OQTS, then
  frozen; stage labels by hand.
- For each screen, a first set of contract types and a fixture that fills them, including the
  awkward cases: an empty section, a long title, a project with nothing new, eight sessions, a
  failed job, a full scout queue.
- A second fixture set, `empty`, in which every section is empty.
- **Check:** `locus-tui --validate-fixtures` printed, every file OK against the schema; a grep
  of `crates/locus-tui/fixtures/` for key and token patterns printed with no matches.

### 1.4 First pass of every screen
Built from the screens v2 mockups as amended by plan v4 (Nerd Font glyphs, footer open · log ·
search, Explore's `[e]`, careers and other kinds of work in Home's project list), then the
screens v2 never drew:

| Screen | Reached by | Source |
|---|---|---|
| Home | Alt-1 | screens v2; D |
| Project | Alt-2 | screens v2; D; D.1 branch graph |
| System, with usage | `s` from Home | screens v2 |
| Budget | from System | **new** (E: week against the cap, the 5-hour window, by day and job type, top jobs, by project) |
| Explore | `e` from Home | screens v2; C.7 |
| Approve and auto log | `a`, `l` from Home | screens v2 |
| Ink confirm | Enter on an ink item in Approve | **new** (C.5: crop beside two or three numbered passages, a number or `x`) |
| Decisions | `d` from Project | screens v2 + 19 Oct: live status, link to the chat, `?` why, `c` into the project chat |
| Answered | `a` from Project | screens v2 + 19 Oct: read-only thread, `f` continue, `t` tablet, `c` open as a session |
| Changes | `n` from Project | screens v2 |
| Search | `/` anywhere | screens v2 + 19 Oct: Enter says where it would jump |
| Work section | in Home; `w` for the area chart | screens v2: rhythm grid, area chart |
| Interview prep | `p` from an opportunity's Project view | screens v2; C.10 |
| Learning | from Home (key settled in round 1) | **new** (C.11: goals, a syllabus shown before it starts, a topic track with its confidence bar and history, practice sheets) |
| Weekly review | `w` from Home in its slot | screens v2, five steps incl. the orphan check |
| Keys | `?` anywhere | **new** |

- The image path (D6) is proved first, on the ink-confirm screen inside tmux in Ghostty.
- **Check:** `--dump` printed for every screen in the table, at 95 × 38, with both fixture sets;
  no panics; startup time and the slowest frame printed (targets: under 100 ms to first frame,
  under 16 ms per frame, R5); a screenshot of the ink-confirm crop inside tmux.

### 1.5 Design rounds (with you)
You use it in your real window; we change it in chat. Each round starts from the use report
and ends with a short entry under **Rounds** below: what changed, why, your words. To settle
here (plan G and screens v2's own open questions):
- how the Project view stops being a wall of text, the branch graph first;
- the work section: rhythm grid by default, area chart one key away, still to prove;
- kairos (Alt-3) and keryx (Alt-4) as views of the locus window, reviewed page by page like
  the rest (20 Oct: "plan to include kairos and keryx in the review as well"); keryx at the
  window size. Changes land in their own repositories;
- whether New splits into kinds; a detail pane for Changes;
- whether Approve sits above Explore on Home;
- the weekly review in use;
- the new screens (Budget, Ink confirm, Learning) from scratch, since nothing on paper went
  before them.

No turn bound. The rounds end when you say the done-when holds.

### 1.6 Contracts frozen
- Each screen's contract types marked frozen; `crates/locus-contract/CONTRACTS.md` lists every screen, its types,
  which app fills them (plan B.2: kairos, keryx, heuris, locus core) and the slice that wires it.
- JSON Schema exported to `crates/locus-contract/schema/` for the Python side.
- A changed frozen type fails a test unless the schema version is bumped, so a later edit
  cannot silently break the backend that fills it (the same rule as the decision-log format,
  C.6).
- **Check:** the schema export printed with its file list; `cargo test` printed green with the
  freeze test included; `CONTRACTS.md` printed.

### 1.7 Report
Below, when the slice closes.

---

## Departures from the plan

- **Three screens are new**, never drawn on paper: Budget, Ink confirm, Learning (per D5). They
  get their first design in round 1, on screen rather than paper, as the plan's header says
  screens are now settled.
- **`locus-theme`** (per D2) is a crate outside the plan. The `eco` repo it lives in was renamed `locus-shared` on 19 Oct at your request ("should it not be called something more accurate"), and the crate with it; `eco-dates` keeps its name until kairos and keryx are next touched. Later the same day the shared crates were folded into this repo (ADR-004): one Rust workspace at the root, `crates/`, in place of D1's `tui/`; `locus-shared` is archived. Moving kairos and keryx onto it is
  **not** in this slice.
- **Your `tmux.conf` gains one line** (sourcing the snippet), shown to you before it is made.
- **The screens v2 source moves into the repo** (1.0); the plan cites it and it was git-excluded.
- **Design mode** is built as a per-screen flag, with nothing live this slice; the plan has it
  as a proposal.
- **The laptop picture in screens v2** (locus in the quick terminal) is superseded by plan B.4
  (its own workspace); the plan wins.

## How it will run

One named session (`locus slice 1`) on the laptop, beside Slice 0's. 1.0–1.4 each under `/goal`
with the check above and a bound of 25 turns; any step needing a decision not covered by D1–D6
stops and asks. 1.3 stops at the start to ask which opportunities to include. 1.5 is chat with
you at the screen. Nothing in this slice touches the server, so it cannot collide with Slice 0's
deploy work; the one shared input is Slice 0's taxonomy file (0.8), and 1.3 copies its content
if it has not landed yet.

## Rounds

*(one entry per design round)*

**Round 0, 20 Oct (the 1.4 check).** The crop rendered through kitty graphics inside tmux in
Ghostty: "image renders fine, could be larger and centered to fill that section of the screen,
but idea is really good, and rendering works well." Changed: the crop now scales to fill its
section, centred. And the synthetic stand-in crop was replaced with his real ink (page 5 of the
QUAST dossier of 15 Oct, 300 dpi): "What is wrong with my real handwriting sitting in the repo?
... I would rather be shown my own handwriting." D4's "no transcript text" is about chat
transcripts, not his own material. How the rounds run, his words: "slowly and methodically work
through refining every one of the pages ... so that it becomes something I know end to end and
feel effortless using." The order: Home; the Project view with Decisions, Answered and
Changes; the Application screen (ADR-006); Calendar (kairos); Mail (keryx); Explore; Approve, the log and Ink confirm; System and
Budget; the work chart; Search; Learning and Interview prep; the weekly review.

**Round 1, 21 Oct: Home (first pass).** From the use log: ↑/↓ pressed 162 times on Home
with nothing focused, Tab 5 times. Changed:
- *Moving around.* Tab and Shift-Tab cycle a screen's sections; ↑/↓ walk a section's rows and
  run on into the next section ("it should also cycle between sections or at least cycle
  between entries in sections until it reaches the bottom"). The selected row is a band across
  the full width with a `▌` ("more of an indicator of what is selected than the tiny little
  arrow ... but I do not want full boxes for everything").
- *What counts as a project.* "they are internship applications and my CV, they do not feel
  like projects." Home now has three sections, each headed with what it holds: projects ("what
  you build": quast, nabla, locus, OQTS, 3YP), careers ("applications and your CV"), learning
  ("what you are learning for"). The registry stays one list underneath, because each still
  needs a Project view and change records; that is the mechanism, not what Home calls them.
  [proposal, his to confirm]. kairos and keryx are parts of locus, not projects ("for the actual
  system kairos should be part of locus and not its own project, same with keryx"): their
  sessions map to locus.
- *Approve.* One "waiting for you" line near the top, by kind; the items are on the approve page
  ("I would rather see the approves in the approve window, with a smaller summary").
- *Explore.* Grouped by kind with headings and space between groups, titles only, no grading
  from Home ("not much should happen from home, it should just take you to somewhere useful").
  Enter opens the find on the Explore page.
- *Projects.* Each count says what it counts: "8 sessions", "2 new" ("the +1/+7 and the
  notifications are confusing"). A session line shows a plain summary of what it wants, not the
  raw command ("better simpler and more intuitive short explanations"): `Session.summary`.
- *Enter on a project* switches the window to Project (Alt-2) showing it, marked "from home";
  Esc goes back to Home and the Project view follows the project window again (his proposal).
- *Header.* The date, running jobs and "review ready" moved to a centred line inside the frame
  ("cramped between the tmux lines ... rarely read").
- *System.* Running jobs separated and pulsing while they run ("animate the running symbols
  slightly ... only when they are active right this instant"); the budget line names both
  limits: "5-hour window: you 34% · background 4%" ("too obscure").
- *Work.* The grid uses two columns a half hour at his width, and the bars fill the panel.
- *`R`* says what it clears: the prototype's ticks and strikes, nothing real.

*Round 1b, 21 Oct (Home, second pass).* "home now looks a lot better ... especially the better
descriptions of what needs me", the wider time tracker, the numbers-only approve line, the
careers/learning split ("a good addition"), the system line ("the background vs me 5h window
line ... very useful"). Changed: the moment centred in the top border, next and mail short at
its right ("on the top of the border ... in a shorter format"); the chores count and the
proposed deadline dropped from Home (the deadline is an approve item; chores are on the Project
view); a blank line between sections, titles bright and bold, rules quiet ("the grey text hides
in between the content"); the focused section marked by a thick blue bar on the frame beside
it; the running glyph turns (md-sync and md-autorenew alternate) instead of changing colour;
Explore trims its longest group first so each kind keeps a line. Kept: waiting for you at the
top (what he looks for first); 3YP a project ("in the format of a long running project").

*Round 1c, 21 Oct (Home, third pass).* "I like the highlighting of the sections." Changed: a
vertical line between careers and learning, ← and → crossing between them; the border's right
end short ("󰃭 futsal in 21h  󰇮 2": the event's first word and the time until it, "in 15m"
style); blocks kept and streak in the same columns as the hours ("really far away and look
disconnected"); today's day label picked out in the grid; the footer's Enter hint says what
Enter does for the focused section, and Tab is listed.

*Round 1d, 21 Oct (Home, fourth pass).* Changed: the week panel is a fixed-width table (a
heading with the total, hours and share of the week per project, one line for blocks kept and the
streak: "too weirdly aligned and like one block"); a now marker on today's row; the divider lights
up when learning is focused, instead of the frame's edge; "waiting for you" moved to the foot,
above system, with "󰅸 2 new" at its right like the project rows (recommended: he checks approvals
as a sign the system is working, and "work feels like a good header section"); quiet projects say
what they last did ("last 28 Aug  agent guidance..."); applications say where they stand and whose
move it is ("applied · waiting for them, 7 days"), the CV its last release (`ProjectView.status`,
`ApproveItem.new`). Not done, his call: pointing the weakest track at its goal.

*Round 1e, 21 Oct.* "home is fine for now." Explore uses spare rows for gaps between its groups
one at a time, instead of all or none. His window is 155 × 44. **Home: done for now.**

Seeds for later pages, his words kept for their rounds:
- *Project:* sessions active in the last 3 days shown, the rest behind `s`, each with a
  summary line; the ask key more visible, or a better key than `i`; the branch graph is broken
  when lanes cluster in a few days (nabla: vertical bars); empty space between New and
  sessions.
- *Explore:* best of the fortnight in bold; reading as three sections (in progress, done,
  proposed); the queue's purpose is not obvious; ask why a find was proposed in a small
  disposable chat; proper dividers per kind; the difference between queue and ideas is unclear.
- *Approve:* the stamp glyph repeats per group; too much empty space; see the 14 mails before
  archiving them; expand a jot to read it; more detail behind every item. Ink confirm: "looks
  really good and is a great idea".
- *Work chart:* fill the width; the x axis is unclear; detail underneath; too much empty space.
- *Auto log:* daily divisions for 7 days, then everything older.
- *Learning:* goals are purposes (a Jump application, general quant interview prep, personal
  learning, Jump internship preparation), not concepts; the goal taxonomy needs settling;
  tracks linking everything is right.
- *Keys:* fine.

**Round 2, 22 Oct: the Project view (first pass).** Changed:
- Home's rules here too ("consistent section highlighting and spacing rules as in with home"):
  gaps between sections, bright titles, the focused section's bar on the frame.
- *Where you are* draws the eye: a peach heading, labelled parts (now, short of) that wrap,
  and next on its own band after a blank line ("next should be given more space and be
  visually separated").
- A strip under it names what sits behind keys: decisions and how many to re-check, answered,
  the dossier's last rebuild, prep, the timeline. A rule for every page from here: "every big
  capability should be referenced in some way on every page, even if it is just a tiny bit of
  space."
- *Branches*: each lane hangs from main and rises back at its merge; lanes are packed into rows
  as git draws them; named on the bar with the short worktree name ("those fit better in small
  spaces"); milestones named above main ("I do not know what those two green dots are"); the
  selected lane's work listed underneath; columns are events in date order, not days.
- *New* grouped by kind, the kind named in a column on its first item; the unread marker on the
  right, as on Home.
- No decision numbers in text ("I do not know off the top of my head which number is what"):
  §-references and ADR numbers written out across the fixtures.
- *Sessions*: the full line is a plain summary of what each wants; the live count in the rule;
  the working glyph turns as on Home.
- *Chores* are life admin for a gap ("laundry, or responding to an email, or picking up
  furniture"), the same list on every project.
- Space opens the ask line ("I felt myself hitting / to go for ask"); `i` still works.
- ↑/↓ walk the sections in layout order (↑ from sessions jumped to branches).
- Opening an application from Home named the project window's project (nabla) over Jump's view.
Open: what an application is, against a project (below the round, his to settle).

**Round 2b, 22 Oct: the Project view (second pass).** Changed: the ask line pinned to the foot
("ask should always be pinned to the bottom"); next set apart by a dashed line, not a band;
"short of" is "shortfall"; the strip became a "more on this project" table at the foot (key,
what, state: "should be in a more sensible place, and laid out in a way that is easier to
read"); the selected branch drawn solid in its colour and the rest dimmed while focused; each
branch carries a plain-English summary of what was done on it (`Lane.summary` replaces
`happened`: "not this weird note form that is hard to decode"); milestones dropped ("too rare
and not enough info so are just added noise"). New split by kind: kept ("that is good").
Settled: ADR-006, the registry holds projects, courses and applications; the Project window
shows only projects; Home groups by area; the map is in the ADR.

**Round 2c, 22 Oct.** The selected branch on a light band in its own colour ("too bold now");
← and → walk the branch graph through the whole history, the window following the selection
("the ability to scroll back in history and see past branches too"); the "more on this project"
table removed ("weird and clunky") and its contents moved to the frame's bottom border as one
line, mirroring Home's top border. "next looks a lot better."

**Round 2d, 22 Oct.** Each key's capability placed where its context is: the dossier centred
in the top border ("relevant to everything"), decisions at the right of the open questions,
the full timeline in New's heading, answered at the far right of the ask line. Open questions
from the dossier under where you are ("remind me of what is next to address bigger picture").
← and → walk the branches from anywhere on the page, starting from the newest; nabla's graph now
holds its full 36-branch history (quast merged most work by fast-forward, so it has only four
branches to show). Home: a studies section beside careers (courses and coursework, 3YP among
them; learning opens from its last row), projects limited to the Projects area. ADR-006 revised:
areas say where, kinds say what, one global taxonomy; registry kinds repo, course, application.

**Round 2e, 22 Oct.** Answered at the right of the ask rule (recommended, matching decisions on
the open questions' rule); branch events at least six columns apart, so nabla shows eleven at a
time with a longer scroll ("do not be afraid of spreading them out"); the start of history
marked (main begins with a green dot, the axis says "start, 21 Sep"); Home's projects section
titled "personal projects" with no callout. His eight courses registered (B1, B2, B3, B14, B15,
B16, B18, B20, Canvas names); 3YP linked to B3 and shown on its row. ADR-006: one kind per thing
(two kinds means two linked things); task is one kind with a type (sheet, coursework, admin,
chore); a due date is an attribute, not a kind.

**Round 2f, 22 Oct. A rule for every page: sections size to their content.** Each section
gets its least useful height; spare rows go to whichever has more to show (hidden rows, then
fuller forms of its rows, such as a second line per find), and only what is left is space. A
section with nothing says so in one line. Compact forms put labels inline. His words: "the
sections should show more or less if they have more or less items to fill up any empty space";
"the formatting should be dynamic with how much information each section has ... This is a rule
across all of locus." Open, his to decide later: fillers for space that is truly empty (careers
when the applications resolve), "something ... that expands and contracts ... and also makes
the space not look like a big hole".
Changed: the Project view hands spare rows to sessions, chores, then the branch's story; Home's
Explore puts each kind's short name on its first find, adding gaps and a "why" line as room
allows; a thin line between courses and learning in studies; a blank line under the tab bar in
the locus session only (a second, empty tmux status line, set by locus-session.sh: it covers
Calendar and Mail too, where a TUI margin would not). Found on the way: the extra status line
left the sessions section no rows and a subtraction panicked; every screen is now tested at six
sizes from 60 × 20 to 200 × 60.

**Round 2g, 22 Oct.** "gap is MUCH better." Experiment, his to revert (`ui::SUMMARY_BOXES`):
any section left with four or more spare rows shows its summary in a quiet rounded box centred
in them, the one filler for every page ("I want any space filler to be the same sort of thing
for any section on any page"); careers (where the applications stand, the CV's last release),
Explore (the finds by kind, that the scout waits), the Project view (this week's hours, live
sessions, new, the latest change). Fixed: Home's sections walked out of drawn order (waiting
for you sat first in the list, last but one on the page); Explore's arrows walked finds that
were not drawn (the draw now records what it drew, in `App::shown`).

**Round 2h, 22 Oct.** Summary boxes off: "lets just leave it blank, and rely on the dynamic
formatting when it can, and leave blank when not" (`SUMMARY_BOXES = false`; the careers box
was liked, kept for later). **A rule for every page: every section's content starts in the
same column** (column 3, after the selection marker), as chores, open questions, New and
Explore did; branches, sessions, personal projects, careers, approve and system moved to it.
"Where you are" is a selectable section. Branches: at least eleven columns an event, so fewer
per window, and no limit on overlapping rows ("less branches visible per window, with no limit
to the number of overlapping branches"). Enter on a branch opens it inline: the full
plain-English summary and its change records (its commits until Slice 2, `Lane.records`, from
`tools/lane_records.py`), ↑ ↓ to scroll, Esc to close; while open it takes spare rows first and
New gives way to two items and a count. Session names size their column. Planned and
confirmed: the Application screen is its own page, after Decisions, Answered and Changes.
`--dump --press "<keys>"` replays keys before drawing, for checking states like these.

**Round 2i, 22 Oct.** Open questions are a selectable section like the rest (each question a
row; Enter will open the dossier where it is asked); the "where you are" rule's dash is quiet,
its title still orange; the selected branch's story is one block, its short name then its
summary on the same line, wrapping beneath, with "merged · 45 commits" or "open · 2 commits" at
the right (the git name, stage label and dates dropped). The Slice 0 session was sent ADR-006
with the course list and the kairos task and deadline direction before its step 0.8, so the
taxonomy file it writes matches the contracts.

**Round 2j, 22 Oct. A rule for every page: one page, no page scroll.** "I want every page to be
one page, and not have to scroll away from other bits. We should instead rely on the dynamic
formatting and the within section scrolling." Sections size to their content and scroll
inside themselves (sessions, an open branch's records); the page never scrolls as a whole.
nabla's 26 branches that carried only their last commit message got written summaries ("the
branch summary as a whole is worth more here than a commit summary"); change-record text stays
raw commit subjects until Slice 2. **The Project view: done for now.**

**Round 2k, 22 Oct. Decisions move to New.** "Open questions and decisions being next to each
other makes you feel like they are linked ... I was expecting open questions to relate to the
decisions when I opened them." Decisions leave the open questions' rule and sit on New's, as
`decisions 7 [d]` beside `full timeline [n n]`: New already lists new and re-checked
decisions, so the log they come from belongs with it. Narrow, the counts drop and the keys stay.

**Round 2l, 22 Oct. Where you are holds what is new; decisions get a section; ask is a
section.** "Decisions does deserve its own section (replacing where new is currently), and ...
new should move to the where you are section, so you see where you are, being now, shortfall,
new, next. New is more something that I look at rather than really interact with." Where you
are now reads now, shortfall, new, next, with the full timeline on its rule as `[n]` ("n alone
for the timeline is fine"); its new rows are still walkable, and Enter still opens one. New
gets three rows by default, the last counting the rest, and spare rows first. The decisions
section sits where New was: re-checks first with their reason on a line of their own, then
what is new, then the rest while they fit, a replaced decision last and named "replaced", not
by its number; Enter opens that decision on the Decisions page, `d` the full log. One
decision, one surface: a decision is marked `new` in the log (a contract field) and New never
lists one; the fixture check fails if it does. "I should be able to tab into the ask section,
and when I am in it, the whole dividing line on which 'ask' sits should be highlighted blue":
ask is the last section, its whole rule lights when focused, and Enter asks. Short of room, the
sections give way in turn (decisions, new, chores, the branch's story, sessions) down to one row
each, then where you are keeps a line per part, so the open questions always draw (they were
squeezed out at 95x36 before this round).

**Round 2m, 22 Oct. Enter on ask opens Answered.** "Space from anywhere makes more sense, I just
think we need a better result of enter when on ask." Space asks from anywhere, so Enter on the
focused ask line opens Answered, what was asked before; the hint line says so when it is
focused.

**Round 2n, 22 Oct. a reaches ask.** "Getting to ask should also be [a], and only when you are
already on ask does the press enter to go to answered appear." `a` focuses the ask line like any
section's letter, with `[a]` at the end of its rule; the answered count stays on the rule with no
key beside it, and the Enter hint shows only once you are there. Space still asks from anywhere.

**Round 2o, 22 Oct. Esc comes back to where you were; Enter opens the waiting answer.** Opening a
screen remembered nothing of where you were, so Esc from Answered (or Decisions, or the
timeline) landed on a Project view with nothing focused. Each screen now keeps its focus under
the one it opened, and selections were already kept, so Esc puts you back on the ask line or on
the decision you opened, on every page. With an answer waiting, the ask hint names it ("⏎ to read
the new answer: delayed acceptance") and Enter opens Answered on it and counts it read; with
none, Enter opens the list. Saved for the Answered round: Space on an answer asks a follow-up
with it as context; the ask line holds a question in flight until its answer lands; Esc while
typing keeps the draft.

**Round 2p, 22 Oct.** "Now and shortfall get highlighted at the same time, instead of being
separate selections": the shortfall is a selection of its own.

**Round 2q, 22 Oct. The Decisions page: a section per state, the entry in full beside it.**
"The different states of decisions should be clearly marked out sections, as currently there is
a huuuge amount of whitspace"; "list on the left and full detailed entry on the right hand
side, the decision detail right now is very short, I want more detail so that I can properly
understand everything about the decision. I like the labelled fields for decisions, but the
content of each field could be much better, and longer." What he opens it for: "checking why we
made a certain decision; checking what decisions are affected by a new one; learning about the
current state and why we are where we are; making sure I did not miss any assumptions/
decisions/ trade-offs".
- The list: to re-check (with the reason), under review, new since you last looked (marked,
  bold, how many days ago: "some way to highlight newest decisions ... more than just being at
  the top"), holding, replaced (dim). Each a section with its count, coloured by state, the
  blank space spread between them; titles wrap to two lines when everything fits. Side by side
  from 120 columns, stacked below that.
- The entry: title and state, then its links (replaces, replaced by, builds on, built on by),
  each a row Enter follows ("press enter on the replaces section and jump to the old decision,
  to see alongside the why on the replacing decisions why it was replaced"); then question,
  chose, why, you said ("definitely quote me if a decision has a quote from me"), over ("over
  is great! keeps me in the loop on other options so I can reopen questions on why another
  method might not be better now given some new detail"), assumes, costs, evidence, affects.
  ↑ ↓ walk the links and then scroll the entry.
- The contract: `Decision` gains question, why, assumes, costs, `you` as a list, `rests_on`
  (ids, so what builds on a decision is derived) and `affects` as a list; `DecisionStatus`
  gains `review`. Decision numbers are ids only, never shown; a test checks no page state shows
  one.
- The content, rewritten from the real logs (quast's `docs/decisions/`, nabla's ADRs, locus's
  ADRs; locus gains its taxonomy decision): every field from the source, nothing invented; 14
  quotes of his from the locus ADRs, one from nabla, none recorded in quast's entries. Three
  unsupported lines in the old quast and nabla fixtures were dropped (a rejected option and an
  affects line on the units decision; "H008 killed inside its budget", which no nabla source
  has). Open for him: quast's continuous-placement decision is listed as holding, but the log
  records that its restored guard and second opinion were dropped again on 30 Sept, so half
  its title is out of date.
- Reopen (`o`): "flag a decision as open/ or under review ... and start a chat with that
  decision as context ... maybe start in locus and if it actually is something that is wrong
  with the project then we should be able to pull it out as a working session into the project
  window". In the prototype it moves the decision to under review (and the Project view's
  section shows it reviewing); `o` again undoes it. The chat is Slice 3.
- What `d` does on a course or an application: decided with the Application screen.
- Tab and the arrows now pass over a section with nothing in it, on every screen.
- Later (seed, from his words): a view of the decision network, how decisions depend on each
  other and on the evidence under them. History: the research track ("decisions and their
  evidence as a dependency graph, so overturned evidence flags what rests on it") agreed 16 Oct
  to start after the dossier loop; "decision network/graph would be interesting to view" (19
  Oct, info.tex); the parked jot "a locus view of the decision network" (19 Oct). `rests_on` is
  the first edge of that graph.

**Round 2r, 22 Oct.** "Right arrow should take you to the description section": → crosses from
the list into the entry, as Enter does, and ← comes back to the same decision in the list.

**Round 2s, 22 Oct.** "There are two parts that say 'the full log [d]'": a "+N more" line only
counts; the key to the rest is on the section's heading, for decisions and for what is new.

**Round 2t, 22 Oct. The entry in sections.** "The sections should be divided by lines, with a
line of white space spacing between them and titles colored per section accordingly, with
section headers colored in colors that do not conflict with any others on the page." Each field
is a section under its own rule, a blank line between, content at column 3 and full width. The
states keep yellow, sky, peach, green and mauve and focus keeps blue, so the fields take the
rest: connections teal, question sapphire, chose rosewater, why flamingo, you said pink (each
quote with its date under it), over maroon, assumes lavender, costs red, evidence subtext,
affects overlay; why re-check and under review keep their state's colour. Where it was decided
moves to the state line.
"Bullets should use -, not the dot icon."

**Round 2u, 22 Oct.** "When up or down reaches the top/ bottom in the detail view, make it stay
there, dont jump back to the list": the entry holds the arrows at its ends; ← (or Tab, or Esc)
leaves it.

**Round 2v, 22 Oct. Ask about a decision, in place of "ask why".** "Can we have an ask section
pinned to the bottom of the detail side, scoped to questions about the decision." The ask line
sits at the foot of the entry as on the Project view: Space asks from anywhere on the page with
the decision on the right as context, `a` reaches the line, and Enter there scrolls the entry to
what you asked. The entry gains an "asked" section (answers that came back, then questions on
their way), with `o` offered beside it, so a question that finds a problem goes straight to a
review; asking never changes a decision's state. `Answer` gains `decision` (the id it was asked
about); no fixture answer is linked, since none was asked about a decision.

**Round 2w, 22 Oct. The entry in layers: a brief, then folded sections.** "I currently love the
concept and how it is turning out, but find the detail view a little too dense, and time
consuming to read such that I might skip over it." Built: (1) every entry leads with a brief,
two lines at most: what was chosen and the one biggest reason (`Decision.brief`, written for all
three projects from the sources); (2) after the connections, each field is folded to one line,
its name and a summary built from its content (the count and first item, or the names of the
options it beat), and Enter or → opens one at a time, ← folds it, ← again goes back to the list;
(3) `why` is now 2-4 short reasons (`Vec<String>`), and question, chose, over, assumes, costs,
evidence and affects were tightened item by item, nothing dropped; (5) in reading order: why,
chose, you said, over, assumes, costs, then question, evidence, affects. The decision text speaks
to him as "you", never "his" or "Alec". Not built yet: a re-check pointing at the assumption it
breaks (4).

**Round 2x, 22 Oct.** "Make hovering over a section automatically show its detail instead of need
to press enter": the section under the selection is the open one, so ↑ ↓ through the folds read
each in turn; ← goes back to the list. A one-paragraph section has no bullet.

**Round 2y, 22 Oct. The entry follows the ADR template; branch decisions; every state shows.**
"This new decision page has entries not present in the decision log, and I think there are also
perhaps too many entries ... instead of the in full, lets have them all as sections like
connections that expand when you hover over them ... whatever sections we have will either need
to be synced with the real decision log format in repos, or something needs to write up each
decision in this format." Agreed: seven sections after the brief, named for the ADR template
locus and nabla already use: connections, decision (was chose + why), you said, alternatives
(was over), assumes ("lets keep assumes instead of the clunky 'would be wrong if'"; the ADR's
What would make this wrong), consequences (was costs + affects), context (was question +
evidence). Each is a rule with its title and count, one summary line while folded, everything
when the selection reaches it. The fixtures were merged field by field, nothing rewritten.
- Sync: where a repo follows the template, locus reads its headings directly (no model); the
  brief everywhere, and every section for a free-form log (quast), come from a writer job in
  the dossier loop (Slice 2), each item citing the passage it came from, derived and
  regenerable; "you said" is only ever copied verbatim. The locus template gains `**Builds
  on:**` and `## Alternatives`; the same is suggested for nabla, in its own repo.
- "Branch decisions should also show, as I often work on branches and want those decisions to
  be viewable live as well": `Decision.branch`, shown on the state line and marked in the list
  (nabla's prospective-hypotheses decision, on compute-leadlag).
- "The other sections other than holding should still show, just with a note that says nothing
  here": every state shows, an empty one with a line ("nothing needs re-checking").

**Round 2z, 22 Oct.** "Holding should be at the top, then new, then the others in their order
now": holding, new, to re-check, under review, replaced, in the list, for Tab and in the
header's counts. An empty "new" whose new decision sits under another state says so.

**Round 2za, 22 Oct. Shorter: labels, short details, a one-line re-check.** "The page is pretty
dense still, I feel like each section is still too long and hard to parse ... my preference of
being shorter so that I actually read it." Every item in decision, alternatives, assumes,
consequences and context is now a `DecisionPoint` {label: two to five words, detail: one short
line}, at most four to a section. Folded, a section is its labels side by side; open, each label
with its detail beside it. The decision section no longer repeats the brief: it opens with the
specifics the brief leaves out. The re-check is a `DecisionPoint` too: what changed in a line,
the numbers under it. Briefs are 25 words at most. Condensed from the previous fixtures by agents
(minor items dropped; the full text stays in the repository's log, behind `f`), then the labels
that meant nothing on their own ("rests on it", "shortfall") relabelled from their details.
"Also highlight whatever decision on the left's detail is being show on the right": while you
read the entry, its decision keeps a band and marker in the list.
- The writer (recorded for the dossier loop, Slice 2): this condensed form is what he reads, so
  a job writes it for every repo, not only the free-form ones, since an ADR's own sections are
  paragraphs and the page would be dense again. It reads the log's sections (the template's
  headings where a repo follows it), writes the brief and the labelled points within these
  limits, each point citing the passage it came from, regenerated when the entry changes, and
  copies "you said" verbatim. `f` opens the full entry.

**Round 2zb, 22 Oct.** "The you said section is weird as the tiny date takes up the whole
section, and my statements are compressed, the date should be below each statement": your words
run the full width, where and when you said them dim underneath, a blank line between.

**Round 2zc, 22 Oct. Spacing by need; plain titles.** Agreed: in a label-and-detail section,
items that fit one line read as a table with no gap; if any item wraps, a blank line goes between
items. "I do not know if I like all of the colors for each section": the per-section title
colours of round 2t said nothing the rule did not, so titles are plain white, blue when open;
colour stays only where it means something (the states, the re-check, your words in pink, a
branch in teal).

**Decisions: done for now**, with his doubt recorded: "will have to see if I actually use it, as
I am unsure if I will given it is quite dense." The test is use, not opinion: the use log already
records each time the page opens (`--use-report`), so once it is live, two weeks with no opens
means it gets cut or rethought (the same rule as R9), and the weekly review is where that is
read. If it is opened but its entries are not, the brief alone may be the page.

**Round 3, 22 Oct. Answered.** "I like the answered page, it does not need to be that
complicated." locus gained six real answers: questions he asked about locus in this chat, in his
words, with the answers condensed from what was said; four link to the taxonomy decision and
show under its "asked".
- The split, recorded as ADR-007: "project window for code writing and implementing plans etc,
  and this window for learning and questioning things ... I feel I would be following up a lot
  and would want it to be ergonomic to do so." Follow-ups stay in the locus window, in the
  answer's thread: a follow-up line pinned at the foot of the answer, Space from anywhere on the
  page, the question shown under the answer while its reply is on its way. `c` is the one bridge
  to building (the thread moves to the project window as a working session). `f` is gone.
- Length: "the answers are a little short, and they should be allowed to be longer, but only
  when they need to be." The answering job writes to the length the question needs (a full
  explanation for "explain the physics" or "describe the strategy"), and a long answer scrolls:
  → or Enter into it, ↑ ↓, ← back. `t` (to the tablet) is offered only for an answer over about
  150 words: "if I asked a more explanatory question ... those would make longer explanations
  that it would actually make sense to send to the tablet."
- The answer leads with the question in full, then where it came from and the decision it is
  about; unread answers carry the marker. Esc while typing keeps the draft, on every ask line,
  and Space brings it back.

**Round 3a, 22 Oct.** "The prompt should just be follow up, with the grey text inside saying
'continue this thread', and c should say build in project window": done, the grey text showing
your draft instead when there is one.

**Round 3b, 22 Oct.** "The other hints should be the same format as in the ask window, the two
line hint split looks odd": on Answered and Decisions the page's keys move onto the ask line's
rule, at its right end, as the Project view's ask line carries answered, and the hint row under
it is gone. Keys that do not fit are dropped from the end. The follow-up line shows ␣ before its
grey text, as the ask line does.

**Round 4, 23 Oct. Changes.** "Same shape as decisions would be good, a you were here last would
be good too, showing what is new since I last opened the page. Also definitely fold routine
items, they would crowd out actual interesting work by me."
- The timeline down the left by day, newest first; the selected entry on the right: what it was,
  when, and its detail (a commit's message for now; a change record from Slice 2). → or Enter
  reads it, ↑ ↓ scroll, ← back. Side by side from 120 columns.
- `Changes.last_seen`: a peach line, "you were here, thu 22", with everything above it marked
  new, and the count in the title.
- `Item.routine`: mechanical work that recurs (nabla's nightly observations) folds into one row,
  "nightly observations · 6 runs", whose runs list on the right.
- locus has a real timeline: its commits since 19 Oct with their messages, and its answers.
  Commit subjects are shown as written, decision numbers included, until change records
  replace them.
- **Done for now, on the same probation as Decisions:** "this is something where I would
  probably better just see how it goes, similar to decisions I will have to see if I actually
  use it." The use log records each open; unopened for two weeks once live, it is cut or
  rethought.

**Round 5, 23 Oct. The Application screen (begun).** "Get rid of chores, I do not need them":
the chores section is gone from the Project view, and `Chore`, `ProjectView.chores` and
`Home.chores_fit` from the contract and every fixture. (The task type "chore" in the taxonomy is
kairos's and stays until he says otherwise.)

## Report

*(written when the slice closes: done, evidence per check, deviations, decisions taken)*
