r"""Markdown -> reMarkable-tuned PDF.

Toolchain (chosen in Phase 0.5, proven end-to-end on a math-heavy sample):
  markdown --pandoc--> typst markup --typst--> PDF

Why this pair and not pandoc+LaTeX or WeasyPrint:
  - **Math fidelity is the priority** — the owner's notes carry GOT-OCR'd LaTeX. Pandoc's
    `typst` writer converts LaTeX math (`\int_{-\infty}^{\infty}`) into *native* Typst math
    (`integral_(- oo)^oo`) at conversion time, so the compile step needs no LaTeX packages and
    no network fetch. The render was verified faithful (integrals, roots, sub/superscripts).
  - **Fully in-env, no system binaries, no sudo** — both `pypandoc-binary` (bundles the pandoc
    binary) and `typst` (bundles the compiler) install via pip into the uv env, so the render is
    deterministic and unit-testable, matching the project's model-free-test ethos (CLAUDE.md §14).

Page geometry defaults target the actual device — a **reMarkable Paper Pro** (2160x1620 px @
229 dpi => a 7.07in x 9.43in screen; Phase-0 device ID `imx8mm-ferrari`), NOT the rM2 numbers in
the plan draft. A PDF page sized to the physical screen renders text at its true point size on
the tablet (reMarkable fits the page to the screen), so 11pt reads as a comfortable 11pt.

Both dependencies live behind the `[reading]` extra; importing this module without them raises a
clear install hint rather than an opaque ImportError.
"""

from __future__ import annotations

import tempfile
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class PageGeometry:
    """Physical page size + text size for the target device, in inches / points.

    Defaults match the reMarkable Paper Pro screen so the page fills it 1:1. All values are
    config-tunable (`[reading]`) — a different device (rM2: 6.21 x 8.28in) is just a config
    change, not a code change.
    """

    width_in: float = 7.07
    height_in: float = 9.43
    margin_in: float = 0.5
    font_pt: float = 11.0

    def typst_preamble(self) -> str:
        """Typst `#set` rules prepended to the converted body to impose the page geometry.

        `justify` + a slightly looser leading read better than Typst's defaults on e-ink; the
        page has no header/footer/numbering (a reading page, not a printed document)."""
        return (
            f"#set page(width: {self.width_in}in, height: {self.height_in}in, "
            f"margin: {self.margin_in}in)\n"
            f"#set text(size: {self.font_pt}pt)\n"
            f"#set par(justify: true, leading: 0.7em)\n\n"
        )


_INSTALL_HINT = (
    "The [reading] extra is required for `locus read`: `uv pip install -e '.[reading]'` "
    "(installs pypandoc-binary + typst; both bundle their binaries, no system packages)."
)


def _import_toolchain():
    """Import pypandoc + typst, translating a missing extra into an actionable message."""
    try:
        import pypandoc  # noqa: WPS433 (local import: optional extra)
        import typst  # noqa: WPS433
    except ImportError as exc:  # pragma: no cover - exercised only without the extra
        raise RuntimeError(_INSTALL_HINT) from exc
    return pypandoc, typst


def markdown_to_typst(md_text: str, *, title: str | None = None) -> str:
    """Convert a markdown string to a full Typst document (preamble filled in by the caller).

    A `# title` heading is prepended when `title` is given and the body does not already open
    with a level-1 heading, so a delivered note is self-identifying on the tablet.
    """
    pypandoc, _ = _import_toolchain()
    body = pypandoc.convert_text(md_text, to="typst", format="markdown")
    if title and not md_text.lstrip().startswith("# "):
        # Escape the two characters that are Typst-significant at the start of heading text.
        safe = title.replace("\\", "\\\\").replace("#", "\\#")
        body = f"= {safe}\n\n{body}"
    return body


def render_markdown_to_pdf(
    md_text: str,
    out_pdf: Path,
    *,
    geometry: PageGeometry | None = None,
    title: str | None = None,
) -> Path:
    """Render markdown text to a device-tuned PDF at `out_pdf`. Returns the output path.

    The Typst source is written to a temp file in an isolated dir (so the compiler's `root`
    is the temp dir, never the repo) and compiled in-process.
    """
    geometry = geometry or PageGeometry()
    _, typst = _import_toolchain()

    typst_source = geometry.typst_preamble() + markdown_to_typst(md_text, title=title)
    out_pdf = Path(out_pdf)
    out_pdf.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "doc.typ"
        src.write_text(typst_source, encoding="utf-8")
        typst.compile(str(src), output=str(out_pdf))
    return out_pdf


def render_markdown_file(
    md_path: Path,
    out_pdf: Path,
    *,
    geometry: PageGeometry | None = None,
) -> Path:
    """Render a markdown file to a PDF, titling it from the file stem when it has no `# ` H1."""
    md_path = Path(md_path)
    text = md_path.read_text(encoding="utf-8")
    return render_markdown_to_pdf(
        text, out_pdf, geometry=geometry, title=md_path.stem.replace("_", " ")
    )
