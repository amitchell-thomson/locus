# ADR-008 — kairos moves into the locus repository, and speaks its taxonomy

**Status:** accepted
**Date:** 2026-10-24
**Your words:** "Kairos should also migrate into the locus repo, I want everything as one system, with sub-apps that are all linked and work seamlessly together and all have context about each other. (keryx will later too)"; "The taxonomies definitely need to match"; "Kairos should DEFINIETELY link to locus, for sure for sure. They should work together and show similar things, and events added in one should show in the other and vice versa"; "This session should take all of the taxonomy work" (24 Oct 2026, Slice 1's Calendar round)
**Builds on:** ADR-004, ADR-006

## Context
kairos is a separate repository (six crates, its own spec, a daemon on the server that is the
only writer to iCloud, an MCP server, a launchd agent on the Mac). It labels things its own way:
calendars Personal, Engineering, Careers, Projects, Sport, Social and Family; deadlines as their
own object; projects as its own list. locus has since settled one taxonomy (ADR-006) and a
registry of projects, and the two describe the same week twice. ADR-004 folded the shared
crates into this repository and left moving kairos and keryx in as a later decision.

## Decision
kairos moves into this repository as sub-apps of one system: its crates join the workspace with
their history, its spec and notes alongside the redesign's, and it deploys through this
repository. It speaks locus's taxonomy: its calendars are the areas (Studies, Projects, Careers
whole; Personal as its streams, Family, Sport, Social, Travel and errands), a due date is a task's
attribute, a task has one type (sheet, coursework, admin, chore). It reads projects from the locus
registry rather than keeping its own list, and the two link both ways: what is added in one shows
in the other, and an item in one opens in the other. kairosd stays the only writer to iCloud; locus
writes calendar items only through it. keryx follows later, the same way.

## Alternatives
- **Keep kairos separate, linked over its HTTP API:** two repositories with one taxonomy would
  drift apart again with every change, and every cross-cutting change would be two commits, two
  reviews and a version pin.
- **Match the taxonomy but keep two project lists in sync:** two sources of truth for the same
  projects; the sync is a second system that can fail quietly.
- **Rewrite kairos inside locus:** throws away a working, tested, deployed system.

## Consequences
- One workspace: a contract or theme change reaches kairos in the same commit.
- kairos's invariants (single writer, live guard, changesets and undo, risk computed never
  supplied, lossless edits) become invariants of this repository, and its CLAUDE.md notes join
  this one's.
- The server and the Mac build kairosd and kairos from this repository; the kairos repository is
  archived once the moved build has run cleanly.
- iCloud calendars are renamed and some events re-filed, each change a kairos changeset with an
  undo.
- The work is carried out under its own spec ("kairos joins locus"), approved before anything in
  kairos changes; Slice 0 no longer holds the kairos taxonomy work.

## What would make this wrong
- If one repository makes kairos's build or deployment fragile (a locus change breaking the
  calendar), the workspace needs firmer boundaries: separate CI gates per sub-app.
- If iCloud does not accept the renames or the re-filing cleanly, the taxonomy is applied in
  kairos's own sidecar (each event's area) rather than as calendars.
