# Locus redesign

Locus is being rebuilt from a document engine into a personal operating system: heuris (the
knowledge app), kairos (time), keryx (mail), one locus window, and the tablet. This folder is the
source of truth for that work. Read it before changing anything that the plan touches.

| File | What it is |
|---|---|
| [`plan.tex`](plan.tex) | **The plan, version 4 (final, 19 Oct 2026).** Every decision tagged with where it came from: `[you, date: "quote"]`, `[agreed date]`, `[proposal]`. Parts: A goal and rules R1–R11 · B the system · C heuris · D interface · E background budget · F build order · G still open. |
| [`info.tex`](info.tex) | Companion: the three kinds of information (sources, work, learning state), who sees what, and how links are made. |
| [`slices/`](slices/) | One spec per slice, written before the slice starts and approved by Alec. |
| `../decisions/` | Locus's own decision log, for decisions made from here on. |

Build the PDFs with `lualatex` (run twice; `enumitem.sty` is vendored here because MacTeX on the
laptop lacks it, and the server's lualatex is broken). Nerd Font glyphs need
`Renderer=HarfBuzz`.

## How the build is run

Agreed 19 Oct 2026 (ADR-002). Alec's part is planning and guidance; the method stops for him at
decisions and runs by itself through mechanical steps.

1. **Spec first.** Each slice gets `slices/slice-N.md`: steps, each with a check whose output
   proves it done; the decisions only Alec can make, listed up front; anything that departs from
   the plan, flagged. Nothing is built until he approves it.
2. **One named session per slice.** Mechanical steps run under `/goal` with a check that prints
   visible output and a turn bound ("… or stop after 25 turns"). Anything needing judgement
   stops and asks. Design work (Slice 1, the ratatui prototype) is plain chat with him at the
   screen, never `/goal`.
3. **Report last.** Each slice closes with a report in its spec file: what was done, evidence for
   every "done when", deviations, decisions taken. Big reports go to the tablet; his marks feed the
   next spec.
4. Code is written on the laptop, pushed to GitHub, and deployed to the server, which runs the
   services (plan B.6).

## Status

**9 Nov: cut to what is used.** The order below is paused, not deleted: a part comes back only when
you want an improvement to something you already use. The Project view (3) is set aside. First:
[`files-and-chat`](slices/files-and-chat.md), then heuris (ADR-011, awaiting your reading of
`heuris.tex`).

| Slice | What | State |
|---|---|---|
| files-and-chat | `~/files` mirrored with the tablet; the Home chat | done 9 Nov |
| 0 | Plumbing | 0.1–0.5, 0.7, 0.12, 0.13 done; 0.6, 0.9–0.11 open |
| 1 | Every screen on fixtures (ratatui prototype) | design rounds done; Home, calendar and mail in use; report not written |
| 2 | Dossier loop | — |
| 3 | Project view live | — |
| 4 | Home live | — |
| 5 | Linking job, Explore and the scout | — |
| 6 | Learning system | — |
| 7 | Canvas, search, charts, budget view | — |
