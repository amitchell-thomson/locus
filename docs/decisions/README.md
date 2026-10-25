# Decision log

One file per decision that commits locus to an approach or closes off an alternative, so that
later it is possible to reconstruct *why*, and to notice when the reasoning has expired.
Reversing a decision takes a new record saying so, never silent drift.

Decisions settled before 19 Oct 2026 live in [`../redesign/plan.tex`](../redesign/plan.tex),
each tagged with its source. This log starts where the plan stops.

**Format: provisional.** Borrowed from nabla's ADRs, with a field for Alec's own words. The one
format every repository will share, checked so an edit can never break it, is chosen in Slice 2
(plan C.6); this log will be converted to it then.

`ADR-NNN-short-name.md`, from [`TEMPLATE.md`](TEMPLATE.md).

| ID | Decision | Status |
|---|---|---|
| [ADR-001](ADR-001-plan-in-repo.md) | The plan lives in the repository; CLAUDE.md gains a redesign section on top | accepted |
| [ADR-002](ADR-002-build-method.md) | Spec per slice, `/goal` for mechanical steps, chat for design, report last | accepted |
| [ADR-003](ADR-003-drop-folder-by-area.md) | The drop folder is organised by area; repository drops retired | accepted |
| [ADR-004](ADR-004-shared-crates-in-locus.md) | The shared Rust crates (`eco-dates`, `locus-theme`) and the locus window live in one workspace in this repo | accepted |
| [ADR-005](ADR-005-data-directory.md) | The data directory is `data/`, not `vault/`; stored paths migrated (0037) | accepted |
| [ADR-006](ADR-006-what-the-registry-holds.md) | One global taxonomy: every item has an area (where) and a kind (what); registry kinds repo, course, application; the Project window shows repos only | accepted |
| [ADR-007](ADR-007-two-windows-build-and-question.md) | The project window builds; the locus window questions and learns, and follow-ups stay there | accepted |
| [ADR-008](ADR-008-kairos-joins-locus.md) | kairos moves into this repository as a sub-app, speaks locus's taxonomy, reads its registry, and links both ways | accepted |
| [ADR-009](ADR-009-for-not-type.md) | Adding anything asks what it is for (a registry item or a Personal stream, by type-ahead); how a task is planned follows from its estimate, not a chosen type | accepted |
