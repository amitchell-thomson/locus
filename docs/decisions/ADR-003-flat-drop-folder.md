# ADR-003 — One flat drop folder; the system labels what arrives

**Status:** accepted
**Date:** 2026-10-19
**Your words:** "keep it then, but we no longer need the sub-folders, as the system does that for us" (19 Oct)

## Context
`~/LocusDrop/` on the laptop is the hand path into the library: the `com.locus.outbox` launchd
job (`~/.local/bin/locus-outbox.sh`) rsyncs it every 60 s to the server's `vault/incoming/`, where
`locus-watch` (Slice 0.2) ingests it. It had five subfolders, `papers/ coursework/ projects/
career/ notes/`, because `ingest_pipeline._category` takes the first folder under `incoming/` as
the document's category. The plan does not list a hand drop among the ways sources arrive (info.tex
Part 1: tablet, notes repository, Canvas, Claude's documents, accepted reading), and it labels
everything with an area (Studies, Projects, Careers, Personal) assigned on arrival rather than
by where it was filed.

## Decision
Keep the drop folder as a way in, for files that reach no other path (a downloaded PDF, lecture
material before the Canvas pull, a career document). Make it flat: the five subfolders are
removed, and the outbox now deletes every emptied folder below the root.

## Consequences
- Until labelling on arrival exists (a later slice), a file dropped at the root is filed with
  category `uncategorized`. Search treats it like any other document. The labelling job should
  backfill `uncategorized` documents when it lands.
- A folder dragged in whole still arrives as a folder, and `_category` takes its name as a
  free-form category (`Control/` -> `control`). Harmless, and the labelling job overrides it.
- Repository snapshots (`projects/<repo>/`) no longer have a place; repository ingest is
  removed by the plan anyway (C.3).
- The outbox script lives outside the repository, in `~/.local/bin/`. It should move into the
  repository with the other laptop job when transcripts sync (Slice 0.10).

## What would make this wrong
If labelling on arrival proves unreliable for some kind of file, a folder hint is the cheap
fix: a subfolder name passed through as a suggestion to the labeller, not as the category.
