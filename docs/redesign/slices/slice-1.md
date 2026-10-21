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
Changes; Calendar (kairos); Mail (keryx); Explore; Approve, the log and Ink confirm; System and
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

## Report

*(written when the slice closes: done, evidence per check, deviations, decisions taken)*
