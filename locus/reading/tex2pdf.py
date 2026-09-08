r"""LaTeX -> reMarkable-tuned PDF. The default shape for a document Claude WRITES for him.

WHY THIS EXISTS ALONGSIDE `md2pdf`
----------------------------------
`md2pdf` renders markdown the owner (or a pass) already wrote: markdown --pandoc--> typst -->
PDF. That is the right pipeline for text that arrived as markdown, and it stays.

This module is for the other direction — a document composed FOR him to read on paper, where
the agent is the author. He asked for those in LaTeX (2026-09-08), and the reasons are
properties of the source language, not of taste:

  - **Equations.** Markdown carries math as an opaque `$...$` span that pandoc must translate.
    LaTeX *is* the math language, so `align`, `cases`, `\substack`, spacing corrections and
    numbered/referenced equations survive because nothing translates them.
  - **Formatting.** The author can reach for `\vspace`, `tabular*`, `minipage`, `\marginpar`
    and a real float placement algorithm. Under markdown those are unreachable at any price.
  - **Images.** `\includegraphics` with a `[width=...]` key and a float, versus markdown's
    single unsized inline form.

So the interface rule (CLAUDE.md §10 does NOT yet cover this — the daily page is deliberately
untouched here): **anything Claude writes for the tablet is authored in LaTeX and compiled by
this module.** `send_markdown` remains for text that was already markdown.

FRAGMENT OR WHOLE DOCUMENT — BOTH, AND THE DISTINCTION IS AUTOMATIC
-------------------------------------------------------------------
Requiring a full preamble on every call would make the common case (a page of prose and three
equations) verbose enough that the model would start omitting the geometry, and a document
typeset for US Letter looks wrong on a 7-inch screen in a way that is easy to miss on a laptop.
Requiring a fragment would throw away the control the language was chosen for.

`render_latex_to_pdf` therefore checks for `\documentclass`. A fragment is wrapped in the
device-tuned preamble below; a full document is compiled EXACTLY as given, geometry and all.
That is the escape hatch: when the author wants a two-column layout or a custom class, they
write the preamble and this module gets out of the way.

ENGINE
------
`tectonic` first, `pdflatex` second. Tectonic is preferred because it fetches and caches the
packages a document actually asks for, so a preamble can use `booktabs` without anyone having
provisioned a TeX distribution first; measured on this machine, a cold compile that downloaded
`enumitem`/`fancyhdr` took 2.6s and every subsequent compile 0.68s.

HONEST DEPARTURE FROM `md2pdf`'s "no system binaries" PROPERTY. md2pdf's toolchain installs via
pip (`pypandoc-binary`, `typst`) and is therefore reproducible from the lockfile alone. There is
no equivalent pip-bundled LaTeX, so this module shells out to a binary that has to exist on the
machine. That is a real regression in reproducibility and it is the price of the language; the
mitigation is that `available_engine()` names the miss and the install command rather than
failing with an opaque `FileNotFoundError` deep in a subprocess.

The preamble sticks to packages that work under BOTH engines (no `fontspec`, no `unicode-math`),
because the fallback has to produce the same document rather than a different one that happens
to compile.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from locus.reading.md2pdf import PageGeometry

# `\documentclass` may be preceded by comments, `\RequirePackage`, or a `%!TEX` magic line, so
# this is a search rather than a prefix test — and it ignores a commented-out one, because a
# fragment that *mentions* the macro in a comment is still a fragment.
_DOCUMENTCLASS = re.compile(r"^[^%\n]*\\documentclass", re.MULTILINE)

# Control characters cannot appear in TeX source: a NUL truncates the file for some readers and
# the C0 range is meaningless to the tokenizer. This codebase has been bitten by exactly this
# byte class before — maths PDFs extract symbol-font glyphs as control codes (CLAUDE.md §6), and
# that text can be pasted straight into a document being written ABOUT those PDFs. Tab, newline
# and form feed are legal TeX and are kept.
_CONTROL_CHARS = re.compile(r"[\x00-\x08\x0b\x0e-\x1f\x7f]")

_ENGINES = ("tectonic", "pdflatex")

_INSTALL_HINT = (
    "No LaTeX engine found. Install tectonic (preferred — it fetches its own packages: "
    "`cargo install tectonic`, or your distro's `tectonic` package) or a TeX distribution "
    "providing pdflatex (`apt install texlive-latex-recommended texlive-latex-extra`)."
)


def available_engine(preferred: str | None = None) -> str:
    """Name the LaTeX engine this machine can actually run, or say what to install.

    `preferred` (from `[reading].latex_engine`) is honoured when present on PATH; otherwise the
    search falls through the built-in order rather than failing, because a config value naming
    an engine that was never installed should not make the feature unusable.
    """
    order = [preferred, *_ENGINES] if preferred else list(_ENGINES)
    for engine in order:
        if engine and shutil.which(engine):
            return engine
    raise RuntimeError(_INSTALL_HINT)


def is_full_document(tex: str) -> bool:
    """True when `tex` carries its own `\\documentclass` and must be compiled untouched."""
    return bool(_DOCUMENTCLASS.search(tex))


def latex_preamble(geometry: PageGeometry) -> str:
    r"""The device-tuned preamble wrapped around a FRAGMENT.

    Geometry comes from the shared `PageGeometry` — the same object `md2pdf` and the daily page
    read from `[reading]` — so the LaTeX and Typst paths cannot drift into typesetting for
    different-sized paper.

    Choices that are about e-ink specifically, not about LaTeX defaults:

    - `\linespread{1.06}` and ragged-right. Justification on a 6.07in text column at 11pt opens
      rivers that a backlit screen hides and e-ink does not; the reMarkable's contrast makes
      inter-word stretching legible as unevenness.
    - `parskip` rather than paragraph indentation. These are notes read in minutes, and a blank
      line between paragraphs is a stronger scanning cue than a 15pt indent.
    - `hidelinks`. Coloured link boxes render as grey smears on a greyscale screen, and there is
      nothing to click on paper.
    - No `fontspec`. It would give better faces but binds the document to XeTeX/LuaTeX, and the
      pdflatex fallback has to produce the same document rather than a near-miss.
    """
    return "\n".join(
        [
            f"\\documentclass[{geometry.font_pt:g}pt]{{article}}",
            f"\\usepackage[paperwidth={geometry.width_in}in,"
            f"paperheight={geometry.height_in}in,margin={geometry.margin_in}in]{{geometry}}",
            "\\usepackage[T1]{fontenc}",
            # MEASURED, not cosmetic (2026-09-08). Without it the pdflatex fallback died with
            # "pdfTeX error (font expansion): auto expansion is only possible with scalable
            # fonts" and produced NO PDF: T1 Computer Modern resolves to bitmap fonts on a
            # stock TeX Live, and `microtype`'s expansion needs Type 1. Latin Modern is the
            # same design in a scalable format, so the fallback now renders the same document
            # rather than being a fallback in the docstring only.
            "\\usepackage{lmodern}",
            "\\usepackage{amsmath,amssymb}",
            "\\usepackage{graphicx}",
            "\\usepackage{booktabs}",
            "\\usepackage{enumitem}",
            "\\usepackage{microtype}",
            "\\usepackage[hidelinks]{hyperref}",
            "\\usepackage{parskip}",
            "\\usepackage{ragged2e}",
            "\\linespread{1.06}",
            # `\RaggedRight`, NOT the built-in `\raggedright`. MEASURED, not preferred: plain
            # `\raggedright` does `\let\\\@centercr`, so `\\` stops ending a table row and the
            # next `\midrule` dies with "Misplaced \noalign". Every document containing a table
            # failed to compile until this was swapped (caught 2026-09-08 on the first realistic
            # sample — three equations, a list and one booktabs table). ragged2e's version
            # leaves `\\` alone and hyphenates properly instead of only stretching interword
            # space, which matters more here than usual: the text column is 6.07in.
            "\\RaggedRight",
            "\\setlength{\\emergencystretch}{2em}",
            "\\pagestyle{plain}",
            # Lists at LaTeX's default leading eat a third of a 9.43in page. Tightened, but not
            # to `nosep` — the items still need to read as separate on a low-contrast screen.
            "\\setlist{topsep=0.3em,itemsep=0.2em,parsep=0pt}",
        ]
    )


def _title_block(title: str) -> str:
    r"""A heading for a document that opens straight into prose.

    The tablet shows no filename while a document is open, so a note that does not name itself
    is unidentifiable three pages in — the same reason `markdown_to_typst` prepends an H1. It is
    a plain `\section*`, not `\maketitle`: `\maketitle` on `article` spends ~1.5in of a 9.43in
    page on vertical centring and a date nobody asked for.
    """
    return f"\\section*{{{escape_text(title)}}}"


def escape_text(text: str) -> str:
    r"""Escape the ten characters TeX reads as syntax, for text that is NOT LaTeX source.

    Used on the `title` argument, which arrives from a tool call as a plain string and may
    legitimately contain `&`, `%`, `_` or `#`. NOT used on the body: the body IS LaTeX and
    escaping it would defeat the entire point of this module.
    """
    out = text.replace("\\", "\\textbackslash{}")
    for char in ("&", "%", "$", "#", "_", "{", "}"):
        out = out.replace(char, f"\\{char}")
    return out.replace("~", "\\textasciitilde{}").replace("^", "\\textasciicircum{}")


def build_document(
    tex: str,
    *,
    geometry: PageGeometry | None = None,
    title: str | None = None,
) -> str:
    r"""Return compilable LaTeX source for `tex`.

    A full document is returned unchanged (minus control characters). A fragment is wrapped in
    `latex_preamble` and given a `\section*{title}` when `title` is set and the body does not
    already open with a sectioning command of its own.
    """
    tex = _CONTROL_CHARS.sub("", tex)
    if is_full_document(tex):
        return tex

    geometry = geometry or PageGeometry()
    body = tex.strip()
    opens_with_heading = re.match(r"\\(section|subsection|chapter|part|title)\*?\s*[{\[]", body)
    if title and not opens_with_heading:
        body = f"{_title_block(title)}\n\n{body}"

    return (
        f"{latex_preamble(geometry)}\n"
        f"\\begin{{document}}\n"
        f"{body}\n"
        f"\\end{{document}}\n"
    )


# A LaTeX log is hundreds of lines of font and package chatter around the handful that say what
# broke. Returning the whole thing to an MCP client buries the actionable line; returning
# nothing makes the failure unfixable. These are the two engines' error markers.
_TECTONIC_ERROR = re.compile(r"^(error|!)\s*:?\s*(.*)$", re.IGNORECASE)
_LATEX_ERROR = re.compile(r"^(?:!|l\.\d+)\s*(.*)$")
_MAX_ERROR_LINES = 12


def _explain_failure(output: str) -> str:
    """Pull the lines that name the LaTeX error out of an engine's noise."""
    lines = [ln.rstrip() for ln in output.splitlines()]
    hits = [ln for ln in lines if _TECTONIC_ERROR.match(ln.strip()) or _LATEX_ERROR.match(ln)]
    if not hits:
        # No recognised marker: the tail is still far more informative than the head, because
        # TeX prints its chatter first and dies last.
        hits = [ln for ln in lines if ln.strip()][-_MAX_ERROR_LINES:]
    return "\n".join(hits[:_MAX_ERROR_LINES])


def _engine_argv(engine: str, src: Path, outdir: Path) -> list[str]:
    """The command line for one compile pass."""
    if engine == "tectonic":
        # `-X compile` is the v2 CLI. `--keep-logs` is deliberately off: the log we want is on
        # stderr, and writing it next to the PDF would put a stray file in the send staging dir.
        return [engine, "-X", "compile", str(src), "--outdir", str(outdir)]
    return [
        engine,
        "-interaction=nonstopmode",
        "-halt-on-error",
        f"-output-directory={outdir}",
        str(src),
    ]


def render_latex_to_pdf(
    tex: str,
    out_pdf: Path,
    *,
    geometry: PageGeometry | None = None,
    title: str | None = None,
    engine: str | None = None,
    resource_dir: Path | str | None = None,
) -> Path:
    r"""Compile LaTeX source to a PDF at `out_pdf`. Returns the output path.

    `tex` may be a fragment or a whole document (see `build_document`). Compilation happens in
    an isolated temporary directory so the engine's write path is never the repo — the same
    containment `render_markdown_to_pdf` gets from Typst's `root`.

    `resource_dir` is prepended to `TEXINPUTS`, which is what makes `\includegraphics{fig}` with
    a RELATIVE path resolve. Without it only absolute paths work, and "images" was one of the
    three reasons this module exists, so the relative form has to work too — a figure is
    normally written beside the document that includes it, not addressed from `/`.

    Raises `RuntimeError` carrying the engine's own error lines. It raises rather than degrading
    for the reason `send_markdown` does: every caller reports to a human who can fix the source,
    and a blank PDF delivered to the tablet is the silent-failure class this codebase exists to
    resist (CLAUDE.md §3).
    """
    engine = engine or available_engine(engine)
    source = build_document(tex, geometry=geometry, title=title)

    out_pdf = Path(out_pdf)
    out_pdf.parent.mkdir(parents=True, exist_ok=True)

    env = dict(os.environ)
    if resource_dir:
        # The trailing "" keeps the default search path — dropping it would hide every package
        # the engine needs to find on its own.
        env["TEXINPUTS"] = f"{Path(resource_dir).resolve()}{os.pathsep}{env.get('TEXINPUTS', '')}"

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)
        src = tmp_dir / "doc.tex"
        src.write_text(source, encoding="utf-8")

        # pdflatex needs a second pass to resolve refs/ToC; tectonic reruns internally and a
        # second invocation would only pay the cost twice.
        passes = 1 if engine == "tectonic" else 2
        for attempt in range(passes):
            proc = subprocess.run(
                _engine_argv(engine, src, tmp_dir),
                capture_output=True,
                text=True,
                cwd=tmp_dir,
                env=env,
                timeout=180,
            )
            if proc.returncode != 0:
                detail = _explain_failure(f"{proc.stdout}\n{proc.stderr}")
                raise RuntimeError(
                    f"LaTeX compile failed ({engine}, pass {attempt + 1}):\n{detail}"
                )

        produced = tmp_dir / "doc.pdf"
        if not produced.is_file():
            raise RuntimeError(
                f"{engine} reported success but wrote no PDF — treating that as a failure "
                "rather than delivering nothing."
            )
        out_pdf.write_bytes(produced.read_bytes())

    return out_pdf


def render_latex_file(
    tex_path: Path,
    out_pdf: Path,
    *,
    geometry: PageGeometry | None = None,
    engine: str | None = None,
) -> Path:
    """Render a `.tex` file to a PDF, titling a fragment from the file stem.

    `resource_dir` defaults to the file's own directory, so a document that includes a figure
    sitting beside it compiles the way it reads.
    """
    tex_path = Path(tex_path)
    return render_latex_to_pdf(
        tex_path.read_text(encoding="utf-8"),
        out_pdf,
        geometry=geometry,
        title=tex_path.stem.replace("_", " "),
        engine=engine,
        resource_dir=tex_path.parent,
    )
