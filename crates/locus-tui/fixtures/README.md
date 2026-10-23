# Fixture sets

Every screen runs on these until its backend exists (plan D; Slice 1 spec, D4). One JSON file
per contract (`locus-contract`); the layout is in `src/fixtures.rs`. Check them with

    cargo run -p locus-tui -- --validate-fixtures

which loads every set strictly (unknown fields fail) and cross-checks the files against each
other.

## default: Saturday 24 Oct 2026, 21:40

**Real, extracted once on 20 Oct and frozen:**
- Branch graphs: quast, nabla, OQTS, CV from their git histories (`tools/branch_graph.py`;
  quast's branches merged by fast-forward were read from their reflogs). locus's August merges
  came from branches whose names did not survive, so those lanes carry the merge subject.
- locus's Changes: its real git history from 19 Oct to the snapshot, each commit's subject as the
  line and its message as the detail, with the six answers joined in (`tools/project_changes.py`).
  nabla's nightly observation commits are marked routine, so they fold.
- locus's Answered: six questions you asked about locus in the Slice 1 design chat (20-21 Oct),
  in your words, with the answers condensed from what was said; four are linked to the taxonomy
  decision they were about.
- Decision titles and dates: quast `docs/decisions/` (§8 entries), nabla and locus ADRs.
  The `chose` / `over` / `evidence` lines are condensed from them by hand; no quotes of yours.
- The Jump Trading prep: the four-round format, the topics and their minutes a day, the seven
  daily sheets, from the tablet (`/Notes/jump_trading`). Practice-session scores are
  approximate: a problem counts as right when the error log (v6) has no line for it.
- The ink-confirm crop: page 5 of the QUAST dossier of 15 Oct with your ink on it, rendered at
  300 dpi from the tablet bundle (`composite_pages_with_margins`), cropped to the right column.
  The candidate passages are that page's text.
- The 8-week area chart: commits per repository per day, at half an hour a commit
  (`tools/make_work.py`).

**Written by hand:** stage labels, "where you are", sessions, chores, scout finds, answers,
usage, the approve list, the auto log, the weekly review, this week's work spans. Courses and the
CV have no decision log, so no `decisions.json`.

**The awkward cases** (spec 1.3): an empty section (OQTS's sessions and chores, locus's chores);
a long title (nabla's hazard-model find, quast's decision titles); a project with nothing new
(OQTS); eight sessions (nabla); a failed job (the tablet push, HTTP 429) and a failed session;
a full scout queue (7 of 7); a day with no work (Friday); an inactive project with a tmux
session (keryx); a calendar item across the clocks going back (Futsal, Sunday 25 Oct).

## empty

The same registry with every section empty (`tools/make_empty.py`, regenerated from default).
