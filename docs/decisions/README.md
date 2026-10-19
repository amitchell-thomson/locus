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
