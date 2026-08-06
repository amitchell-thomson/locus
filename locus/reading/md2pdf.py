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
    # Vertical space between ruled lines, in em. MEASURED, not assumed: Typst collapses a block's
    # `above` against the previous block's `below`, so consecutive rules sit ONE gap apart, not
    # two. At `[daily].rule_gap_em` 2.6 the delivered page measured 28.6pt = 10.1mm between rules
    # — wider than college-ruled paper and, in the owner's words, "way too far apart".
    rule_gap_em: float = 1.5

    # --- optional editorial styling (the daily page opts in; `locus read` does not) -------------
    # A delivered PAPER wants to look like the paper. The daily page is a different object: it is
    # picked up once, scanned in seconds, and written on, so it earns a type hierarchy, an accent
    # and a running header. Left None, every rule below is skipped and the render is byte-for-byte
    # what it was.
    accent: str | None = None          # hex, e.g. "#1B3A5F" — one colour, used sparingly
    sans_font: str | None = None       # display face for headings/anchors (serif stays for prose)
    running_header: str | None = None  # left-hand text; the page number is added on the right

    def _style_rules(self) -> str:
        """Show-rules for the editorial variant. Empty unless `accent`/`sans_font` are set."""
        if not (self.accent or self.sans_font):
            return ""
        accent = self.accent or "#000000"
        sans = self.sans_font or "DejaVu Sans"
        out = [f'#let accent = rgb("{accent}")']
        if self.running_header is not None:
            # Four loose pages where a tick on p4 answers a question on p3 — they have to say
            # which page they are. `final()` needs `context` to resolve the total.
            header = self.running_header.replace('"', '\\"')
            out.append(
                f'#set page(header: context {{\n'
                f'  set text(size: 7.5pt, fill: luma(45%), font: "{sans}")\n'
                f'  grid(columns: (1fr, auto), align: (left + bottom, right + bottom),\n'
                f'    [{header}],\n'
                f'    [#counter(page).display() / #counter(page).final().first()])\n'
                f'  v(-0.5em)\n'
                f'  line(length: 100%, stroke: 0.3pt + luma(82%))\n'
                f'}})'
            )
        out += [
            # The SECTION is the page's title (the date moved to the header), so it gets the
            # weight the date used to compete for.
            f'#show heading.where(level: 1): it => block(below: 1.1em)[\n'
            f'  #text(font: "{sans}", size: 15pt, weight: "bold", fill: accent,'
            f' tracking: 0.6pt)[#upper(it.body)]\n'
            f'  #v(-0.55em)\n'
            f'  #line(length: 100%, stroke: 1pt + accent)\n]',
            f'#show heading.where(level: 2): it => block(above: 1.5em, below: 0.7em)[\n'
            f'  #text(font: "{sans}", size: 9pt, weight: "bold", fill: luma(40%),'
            f' tracking: 1.1pt)[#upper(it.body)]\n]',
            f'#show heading.where(level: 3): it => block(above: 1.2em, below: 0.6em)[\n'
            f'  #text(font: "{sans}", size: 8.5pt, weight: "bold", fill: luma(45%),'
            f' tracking: 1pt)[#upper(it.body)]\n]',
            # Anchors are the page's reference marks, so they get their own markup rather than
            # borrowing bold: `**` is also used for "In progress" and "On the shelf", and styling
            # every strong span turned those blue too — and swallowed the item TITLE, which sits
            # inside the same `**...**` as its anchor.
            f'#let anc(s) = text(font: "{sans}", size: 9.5pt, weight: "bold", fill: accent)[#s]',
            '#let tickbox = box(width: 0.95em, height: 0.95em, radius: 1pt, '
            'stroke: 0.6pt + accent)',
            # Provenance was rendering two ways for one kind of information: monospace on Read
            # (`discovery · project:X`) and italic on Think. One quiet style — and the FONT has to
            # be set, or `raw` stays monospace however it is coloured.
            f'#show raw: it => text(font: "Libertinus Serif", size: 9.5pt, fill: luma(38%),'
            f' style: "italic")[#it.text]',
            # `emph` must KEEP its italic: a show rule replaces the element, so setting only the
            # fill silently rendered every italic line — including the status footer — upright.
            '#show emph: it => text(fill: luma(38%), style: "italic")[#it.body]',
        ]
        return "\n".join(out) + "\n\n"

    def typst_preamble(self) -> str:
        """Typst `#set` rules prepended to the converted body to impose the page geometry.

        `justify` + a slightly looser leading read better than Typst's defaults on e-ink; the
        page has no header/footer/numbering (a reading page, not a printed document).

        `horizontalrule` must be defined here. Pandoc's typst writer emits a bare
        `#horizontalrule` for a markdown `---`, but that binding lives in pandoc's own typst
        TEMPLATE, and we use only the converted BODY — so every `---` in every document
        delivered to the tablet has been rendering as nothing at all (verified 2026-07-30:
        zero full-width strokes in the output PDF). Silent, because an unknown identifier used
        as a value compiles to empty rather than failing. It is bound to a real ruled line now,
        which is also what gives the daily page (§9) the lines the owner writes corrections on.
        """
        rule_colour = "luma(78%)" if self.accent else "luma(65%)"
        return (
            f"#set page(width: {self.width_in}in, height: {self.height_in}in, "
            f"margin: {self.margin_in}in)\n"
            f"#set text(size: {self.font_pt}pt)\n"
            f"#set par(justify: true, leading: 0.7em)\n"
            # Lighter when styled: the rules are a guide for HIS ink, and at 0.4pt/65% they were
            # competing with the writing they exist to carry.
            f"#let horizontalrule = block(width: 100%, above: {self.rule_gap_em}em, "
            f"below: {self.rule_gap_em}em, line(length: 100%, stroke: 0.3pt + {rule_colour}))\n"
            # ALWAYS DEFINED, because the daily markdown calls them unconditionally and an
            # undefined typst identifier used as a FUNCTION is a hard compile error (unlike one
            # used as a value, which silently yields empty — the trap `horizontalrule` fell into).
            # The styled variant below rebinds them; unstyled they degrade to plain bold and a
            # grey square, so the same markdown renders either way.
            "#let anc(s) = text(weight: \"bold\")[#s]\n"
            "#let tickbox = box(width: 0.95em, height: 0.95em, radius: 1pt, "
            "stroke: 0.6pt + luma(35%))\n\n"
        ) + self._style_rules() + self._keep_rule(rule_colour)

    def _keep_rule(self, rule_colour: str) -> str:
        """A ruled writing line whose right-hand end carries a tick box — the last line of a
        decidable item (`compose_daily._render_think`).

        DEFINED HERE, NOT SPELLED OUT IN THE MARKDOWN, and defined LAST so it closes over whatever
        `tickbox` is finally bound to — the styled preamble rebinds it, and a `#let` captures the
        binding that is live where it is written, so defining this any earlier would silently give
        the daily page the grey unstyled box.

        MEASURED (2026-08-06). The composer used to emit this inline as `#block(above: 1.2em)[...]`
        and the delivered page put that rule 30.2pt below its neighbour where the other three sat
        19.8pt apart — visibly a different line spacing on the one item that has a box. Two causes,
        both invisible in the source: Typst collapses `above` against the previous rule's `below`,
        so the 1.2em never applied at all (the 1.8em gap won), and the tick box then made the block
        ~10pt TALL, pushing the stroke that far down inside it. So the box is `height: 0pt` and
        both halves are `place`d on its horizon: placement is out-of-flow, the block keeps a rule's
        zero height, and the stroke lands exactly one `rule_gap_em` down like every other line.
        The gap comes from the same config value the other rules use, so they cannot drift apart.
        """
        return (
            f"#let keeprule(label) = block(width: 100%, above: {self.rule_gap_em}em, "
            f"below: {self.rule_gap_em}em, box(width: 100%, height: 0pt)[\n"
            f"  #place(left + horizon, box(width: 72%, "
            f"line(length: 100%, stroke: 0.3pt + {rule_colour})))\n"
            f"  #place(right + horizon, box[#tickbox #text(size: 9pt, fill: luma(45%))[#label]])\n"
            f"])\n\n"
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
