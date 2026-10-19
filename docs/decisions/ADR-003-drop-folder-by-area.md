# ADR-003 — The drop folder is organised by area

**Status:** accepted
**Date:** 2026-10-19
**Your words:** "keep it then, but we no longer need the sub-folders, as the system does that for us"; then, told the system cannot label yet, "go with the area folders" (19 Oct)

## Context
`~/LocusDrop/` on the laptop is the hand path into the library: the `com.locus.outbox` launchd
job (`~/.local/bin/locus-outbox.sh`) rsyncs it every 60 s to the server's `data/incoming/`, where
`locus-watch` (Slice 0.2) ingests it. It had five subfolders, `papers/ coursework/ projects/
career/ notes/`, because `ingest_pipeline._category` takes the first folder under `incoming/` as
the document's category. Those mixed two things: what kind of file it is (readable from the file)
and what it is for (not readable from the file). The plan labels everything with an area
(Studies, Projects, Careers, Personal) and lists the tablet, the notes repository, Canvas,
Claude's documents and accepted reading as the ways sources arrive; a hand drop is not on it.

A flat folder was chosen first, on the understanding that the system labels arrivals. It does
not yet: labelling on arrival is a later slice, so a flat drop would be filed `uncategorized`.

## Decision
Keep the drop folder as a way in, for files that reach no other path (a downloaded PDF, lecture
material before the Canvas pull, a career document). Organise it by area, the same tree as the
notes repository and the tablet (plan C.9):

```
~/LocusDrop/
  studies/<course>/   projects/<name>/   careers/<company or general>/   personal/
  (root)              let the system decide: `uncategorized` until the labeller exists
```

Subfolders are optional. The area and course/project folders persist when emptied; deeper ones
are cleared once sent.

## Consequences
- `_category` maps each area onto the category the library still keys behaviour on
  (`AREA_FOLDERS`: studies -> coursework, projects -> project, careers -> career). `personal/`
  gets its own category, `personal`, not `note`. The older folder names keep working.
- **Nothing dropped into an area folder counts as his writing** (`ingest_pipeline.is_area_drop`,
  checked by both `structure.propose._is_owner_authored` and `state.owner_authored_sql`). Before
  this, a text file whose category was note/project/career counted as his, so a third-party
  `.docx` in `personal/` (then `note`) or `careers/` would have had positions drawn from it and
  attributed to him (§3.4). His writing arrives through the notes repository and the tablet,
  which are recognised by location. Accepted cost (your call, 19 Oct: "let them flip"): a
  write-up dropped in `projects/` is a source, and that includes the two OQTS specs already
  filed under the old `projects/` folder, which shares the area's name. Their stored beliefs and
  links stay; nothing new is drawn from them. The old singular folders (`career/`, `notes/`)
  are unchanged.
- The course or project folder (`studies/control/`) is not stored in a column. It survives in
  `source_uri`, the incoming path, which is where the labeller of a later slice reads it. A
  folder-labelled file should then need no approve-list item.
- **Repository drops are retired.** A directory under `incoming/projects/` used to be ingested
  whole as a code-repo snapshot (`locusdrop:<name>`); it is now an area folder of ordinary
  files. Documents already stored under `locusdrop:` keys are untouched. Repository ingest is
  removed by the plan anyway (C.3).
- The outbox script lives outside the repository, in `~/.local/bin/`. It should move into the
  repository with the other laptop job when transcripts sync (Slice 0.10).

## What would make this wrong
If the labeller proves reliable enough that the folders are never used, they can go; the root
already works. If the old categories are retired, `AREA_FOLDERS` maps to areas directly.
