# ADR-004 — The shared Rust crates live in the locus repository

**Status:** accepted
**Date:** 2026-10-19
**Your words:** "why are we calling it eco? ... should it not be called something more accurate
to what it is" (19 Oct 2026, which renamed `eco` to `locus-shared`); then, offered its own repo,
a submodule, or folding in: "fold the shared crates into the repo now" (19 Oct 2026).

## Context
`eco` (14 Oct) held crates shared by kairos and keryx (`eco-dates`), and Slice 1 added
`locus-theme` to it (spec D2). The locus window depended on it through a relative path
(`../../locus-shared/...`), which only works on a machine with both repos checked out side by
side. The alternative, a git pin, would make every theme change in a design round a commit
there, a push, and a pin bump here. A submodule was ruled out: two commits per change and a pin
that is easy to forget, so a checkout can build old code while looking current (CLAUDE.md §3).

Plan B.1 merges the repositories "once the shared root has real contents". The shared root now
holds the taxonomy (Slice 0, D3), the screen contracts and the theme.

## Decision
1. One Rust workspace at the root of this repo: `Cargo.toml` and `crates/` holding `eco-dates`,
   `locus-theme`, `locus-contract` and `locus-tui` (fixtures in `crates/locus-tui/fixtures/`).
   This replaces Slice 1's `tui/` (spec D1), since the workspace now holds more than the TUI.
2. `eco-dates` was copied verbatim (identical to `locus-shared` main at d2191b2); `locus-theme`
   from the `locus-shared` `theme` branch (b74d7a0).
3. kairos and keryx keep their current pins, which still resolve: the GitHub repo is archived,
   not deleted, and redirects from `eco`. Each moves its dependency to this repo
   (`git = ".../locus"`, `rev = "..."`) when next touched; Cargo finds a crate by name anywhere
   in a repository.
4. This is the first piece of B.1 only. Moving kairos and keryx themselves into this repo stays
   a later decision.

## Consequences
- A theme or contract change during the design rounds is one commit.
- kairos and keryx builds fetch this repo for two small crates (4.3 MB of history, against
  kairos's own 10 MB).
- A change here to a crate kairos or keryx uses does not reach them until their pin is bumped,
  as before.

## What would make this wrong
If kairos or keryx need a shared-crate change on a cadence that differs from locus's, or the
repo's Python side starts getting in the way of their builds; then the shared crates move back
out, into a repo that holds only them.
