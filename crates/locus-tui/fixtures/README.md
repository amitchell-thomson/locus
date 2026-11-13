# Fixture sets

Every screen runs on these until its backend exists (plan D; Slice 1 spec, D4). One JSON file
per contract (`locus-contract`); the layout is in `src/fixtures.rs`. Check them with

    cargo run -p locus-tui -- --validate-fixtures

which loads every set strictly (unknown fields fail) and cross-checks the files against each
other.

## default: Saturday 24 Oct 2026, 21:40

The fallback a live window replaces: tests and `--dump` draw it, and a live Home shows none of
it as current (cleanup C3, 13 Nov: the Project, Application, Decisions, Changes, Answered, Prep,
Explore, Review, Search and ink screens went, with their fixtures and the tools that made them).

**Real, extracted once on 20 Oct and frozen:** the 8-week area chart, commits per repository per
day at half an hour a commit (`tools/make_work.py`); application stages, from your mail.

**Written by hand:** stage labels, sessions, chores, usage, the approve list (in the kinds kairos,
keryx and the facts ledger send), the auto log, this week's work spans.

**The awkward cases** (spec 1.3): an empty section (OQTS's sessions); eight sessions (nabla); a
failed job and a failed session; a day with no work (Friday); an inactive project with a tmux
session (keryx); a calendar item across the clocks going back (Futsal, Sunday 25 Oct).

## empty

Every section empty (`tools/make_empty.py`, regenerated from default). Neither set carries the
taxonomy or the registry: both read the one copy in `taxonomy/` at the repository root.
