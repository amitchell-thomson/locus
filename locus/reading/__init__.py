"""Reading-delivery layer (agent-layer plan §8.5, Phase 0.5).

The reverse of the capture transport: renders a markdown document to a reMarkable-tuned PDF
and pushes it to the tablet via `rmapi put`. Useful day one (read any repo doc / note on the
device) and, more importantly, it is the smallest thing that exercises the server->device push
channel end-to-end — the channel the daily page and every reading loop (Phases 1/3) depend on.

Two small, separately-testable pieces:
  - `md2pdf`  — markdown -> device-tuned PDF (pandoc md->typst, then the typst compiler).
  - `deliver_remarkable` — `rmapi put` to a device folder, behind an injectable runner.
"""

from __future__ import annotations

from locus.reading.md2pdf import PageGeometry, render_markdown_file, render_markdown_to_pdf

__all__ = ["PageGeometry", "render_markdown_to_pdf", "render_markdown_file"]
