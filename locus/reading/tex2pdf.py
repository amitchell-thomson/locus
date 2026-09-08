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

The engine name from config is a PREFERENCE, not a command: `available_engine` falls through to
whatever is installed. This was true in the docstring and false in the code until 2026-09-08 —
`render_latex_to_pdf` wrote `engine or available_engine(engine)`, which short-circuits on the
non-empty configured name and never looks at PATH. Nothing surfaced it because the process that
sends is not the process you test in: the MCP server runs under `uv run`, whose PATH lacks
~/.local/bin where `tectonic` is installed, so every MCP send died on FileNotFoundError with
/usr/bin/pdflatex available and unconsulted, while the same call from a login shell worked. If
you touch engine selection, test through `render_latex_to_pdf`, not `available_engine` alone —
a unit test of the helper passes either way, which is why one existed and caught nothing.

HOUSE STYLE, AND WHICH HALF OF IT IS ENFORCED
---------------------------------------------
Two rules arrived with the two-column change (2026-09-08). They are enforced differently on
purpose. **No em dashes** is checkable, so `build_document` raises on one (`_EM_DASH`). **No
assistant register** — no throat-clearing, no "it's worth noting", no summary that restates the
section it just ended — is a judgement, so it lives in the MCP tool docstring and the
`remarkable` command where the authoring model reads it. A regex for the second would fire on
honest prose, and a rule that fires wrongly gets ignored, taking the enforceable one with it.

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

# The body sizes the `extsizes` classes actually implement. Stock `article` implements only
# 10/11/12 and SILENTLY typesets anything else at 10pt — `\documentclass[9.5pt]{article}`
# compiles clean, reports nothing but "Unused global option" in the log, and renders at 10pt.
# Measured 2026-09-08 by compiling both and reading `\f@size` out of the PDF. Hence `extarticle`
# below, and hence `[reading].latex_font_pt` validating against this tuple at config load.
CLASS_SIZES_PT = (8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 17.0, 20.0)

# Em dashes are banned outright from agent-authored documents (his instruction, 2026-09-08).
# Enforced rather than merely documented because it is the one house-style rule that is exactly
# checkable: `---` and U+2014 are unambiguous, so a guard here cannot be wrong about what it
# found. The rest of the house style (no assistant register, no throat-clearing) is a judgement
# and lives in the tool docstring and the `remarkable` command, where a human or the authoring
# model applies it — a regex that tried would fire on legitimate prose.
#
# It raises rather than rewriting. Substituting a comma, a colon or parentheses changes what the
# sentence CLAIMS, and this module never edits his documents' meaning; the author is in the loop
# and can recast the clause. Note this only ever sees prose the AGENT wrote: his own words reach
# the device through `send_markdown`, which does not pass through here.
#
# `--` (en dash, for ranges like 3--5) is deliberately untouched.
_EM_DASH = re.compile(r"---|—")

_ENGINES = ("tectonic", "pdflatex")

# Ceiling on ONE engine invocation. Public because `[reading].latex_timeout_s` documents this as
# the fallback and `send_latex` passes the configured value over it.
#
# MEASURED (2026-09-08). The predecessor was a hardcoded 180s, and two cold-cache runs came in at
# 201.5s and 149.7s — STRADDLING it. Note which was which: 201.5s was a ONE-LINE fragment and
# 149.7s a document with two TikZ pictures, a pgfplots axis and an equation. The document is not
# the variable; the ~51MB fetch of the pgf/tikz tree is, and it crosses a network. Warm, both
# compile in ~1.2-1.3s.
#
# So 180s was not a limit that always failed — it sat INSIDE the natural variance of the thing it
# was cutting off, which is the worst place for a threshold to sit and why this presented as an
# intermittent "the server is down" rather than as a reproducible error. The tell that it was
# never about the LaTeX: a full brief and the minimal fragment sent to isolate it failed
# IDENTICALLY, because `_DIAGRAM_SETUP` loads tikz and pgfplots unconditionally, so document size
# does not change what the FIRST compile must download.
#
# 900s is ~4.5x the slower observation, deliberately. A ceiling picked to just clear one
# machine's timing on one day is the same mistake with a larger number.
DEFAULT_TIMEOUT_S = 900.0

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


# TikZ/pgfplots setup, kept out of `latex_preamble`'s list purely because the cycle list needs
# literal `\\` separators and reads better as one raw block.
#
# The e-ink-specific part is the DEFAULTS, not the packages. The tablet is greyscale, so a plot
# that separates its series by colour separates them by nothing: `cycle list name=eink` below
# distinguishes them by dash pattern first and only then by grey level. Line widths are set
# above LaTeX's defaults for the same reason a hairline rule vanishes on a reflective screen.
#
# `cycle list name` is set at TOP LEVEL and not folded into the `every axis` style below, which
# is where it was first written and where it silently did nothing: pgfplots installs its own
# default cycle list over anything `every axis` sets, so every plot came out in the stock blue
# and red. Grepping the preamble for the key could not tell the difference, which is why
# `test_plots_default_to_greyscale` renders a real plot and asserts on the PIXELS instead.
#
# `bar cycle list` is a SECOND, separate default that `ybar` installs over the first, initially
# blue and red fills. Fixing only the line cycle list left every bar chart coloured, and the
# first version of the pixel test missed it by plotting lines — so the test now renders one of
# each. Bars separate by fill level, since a dash pattern on a filled rectangle reads as noise.
_DIAGRAM_SETUP = r"""\usepackage{tikz}
\usetikzlibrary{arrows.meta,positioning,calc,fit,shapes.geometric,decorations.pathreplacing,patterns}
\usepackage{pgfplots}
\pgfplotsset{compat=1.18}
\tikzset{>=Stealth}
\tikzset{every picture/.append style={line width=0.6pt,font=\footnotesize}}
\pgfplotscreateplotcyclelist{eink}{%
black,solid\\%
black,dashed\\%
black,dotted\\%
black,dashdotted\\%
black!55,solid\\%
black!55,dashed\\%
}
\pgfplotsset{cycle list name=eink}
\pgfplotsset{bar cycle list/.style={cycle list={%
{black,fill=black!12},%
{black,fill=black!42},%
{black,fill=black!70},%
{black,fill=white},%
}}}
\pgfplotsset{every axis/.append style={%
  line width=0.5pt,tick style={line width=0.4pt},%
  label style={font=\footnotesize},tick label style={font=\scriptsize},%
  legend style={font=\scriptsize,draw=black!40}}}"""


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
    - **Two columns, always** (his instruction, 2026-09-08). On a 7.07in page with 0.5in margins
      that is two ~2.9in columns, which is close to a journal measure and is why the body size
      drops with it: `[reading].latex_font_pt` defaults to 9pt where the markdown path stays at
      11pt. The consequence an author has to hold in mind is that a display equation wider than
      2.9in will overflow its column silently-looking (LaTeX logs an Overfull \hbox and renders
      it hanging into the gutter), so wide maths wants `split`/`aligned` or the full-width
      `equation*` inside a `figure*`. Tables and figures that need the full measure use the
      starred floats, `table*` and `figure*`.
    - `extarticle`, not `article`. Only so that 9pt is really 9pt: see `CLASS_SIZES_PT`.
    - TikZ and pgfplots are loaded for every document (`_DIAGRAM_SETUP`). The tablet is for
      LEARNING something, and a diagram drawn beside the prose is most of what makes that work
      on paper, so the packages have to be there without the author declaring them — the same
      contract the docstring already makes for amsmath and booktabs. Measured cost of carrying
      them on a document that draws nothing (2026-09-08, tectonic warm, best of 3): 1.13s
      against 0.65s without, so ~0.5s per send. The first compile after a version bump also
      pays a one-off ~34s while tectonic fetches the pgf tree.
    """
    if geometry.font_pt not in CLASS_SIZES_PT:
        allowed = ", ".join(f"{s:g}" for s in CLASS_SIZES_PT)
        raise ValueError(
            f"font_pt={geometry.font_pt:g} is not a size the document class implements "
            f"(allowed: {allowed}). LaTeX would accept the option and silently typeset at a "
            "different size, so this refuses instead."
        )
    return "\n".join(
        [
            f"\\documentclass[{geometry.font_pt:g}pt,twocolumn]{{extarticle}}",
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
            _DIAGRAM_SETUP,
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
            # 3em, not the 2em the single-column layout used. Emergency stretch is the last
            # resort TeX reaches for before letting a line stick out, and a 2.9in column gives
            # it far less room to break a long identifier or URL than a 6.07in one did.
            "\\setlength{\\emergencystretch}{3em}",
            # The gutter. Wide enough that two columns of 9pt text do not read as one, narrow
            # enough not to spend measure that the body needs at this size.
            "\\setlength{\\columnsep}{0.22in}",
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

    Wrapped in `\twocolumn[...]` since the layout went two-column: the optional argument is the
    one place LaTeX sets material across the full measure at the top of a page. Without it the
    title is a `\section*` sitting in the left column, which reads as the first section's
    heading rather than as the document's name.
    """
    return f"\\twocolumn[%\n  \\section*{{{escape_text(title)}}}%\n  \\vspace{{0.4em}}%\n]"


# The heading a fragment opens with, if it opens with one, and its text.
_OPENING_HEADING = re.compile(
    r"\\(?:section|subsection|chapter|part|title)\*?\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}"
)


def _opening_heading_repeats(body: str, title: str) -> bool:
    """True when the fragment opens with a heading that IS the title, so printing both duplicates.

    Compares the TEXT, not merely the presence of a heading. The presence test this replaces
    suppressed the title whenever a document opened with any `\\section{...}` at all, which is how
    a normal document opens — so a fragment beginning "1. The project in one sentence" was
    silently delivered with no title on it. That is worse under the two-column layout than it was
    before: `_title_block` is the only thing set across the full measure, so losing it loses the
    one piece of the page that announces what the document is, and the tablet shows no filename
    while a document is open.
    """
    match = _OPENING_HEADING.match(body)
    if not match:
        return False
    normalise = lambda text: " ".join(text.split()).casefold()  # noqa: E731
    return normalise(match.group(1)) == normalise(title)


def _reject_em_dashes(text: str, *, where: str) -> None:
    r"""Raise if `text` contains an em dash, in either the `---` or the U+2014 spelling.

    See `_EM_DASH` for why this is enforced here and why it raises instead of substituting.
    The message quotes the surrounding words because `---` is invisible in a wall of LaTeX and
    "there is an em dash somewhere" is not a fixable report.
    """
    hits = list(_EM_DASH.finditer(text))
    if not hits:
        return
    samples = []
    for match in hits[:3]:
        start, end = max(0, match.start() - 35), min(len(text), match.end() + 35)
        snippet = " ".join(text[start:end].split())
        samples.append(f"  ...{snippet}...")
    more = f" (and {len(hits) - 3} more)" if len(hits) > 3 else ""
    raise ValueError(
        f"em dash in the {where}{more} — this document style does not use them. Recast the "
        "sentence, or use a colon, a semicolon, parentheses or a full stop. An en dash (--) "
        "for a numeric range is fine.\n" + "\n".join(samples)
    )


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
    `latex_preamble` and given a full-measure title when `title` is set and the body does not
    already open with a sectioning command of its own.

    "Already opens with one" means a heading whose TEXT is the title, not merely any heading:
    a document that opens with its first numbered section still gets titled. See
    `_opening_heading_repeats`.

    Raises `ValueError` on an em dash anywhere in the body or the title (see `_EM_DASH`).
    """
    tex = _CONTROL_CHARS.sub("", tex)
    # Before the full-document early return: a `\documentclass` document is still something the
    # agent wrote, and the house style is about the prose, not about who supplied the preamble.
    _reject_em_dashes(tex, where="document body")
    if title:
        _reject_em_dashes(title, where="title")
    if is_full_document(tex):
        return tex

    geometry = geometry or PageGeometry()
    body = tex.strip()
    if title and not _opening_heading_repeats(body, title):
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


# `\includegraphics[key=val]{name}` — the optional argument is skipped, and `name` may carry a
# subdirectory and may omit its extension (graphicx resolves that itself).
_INCLUDEGRAPHICS = re.compile(r"\\includegraphics\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}")

# The extensions graphicx will try for an extensionless name, in the order it tries them.
_GRAPHICS_SUFFIXES = (".pdf", ".png", ".jpg", ".jpeg")


def _stage_graphics(source: str, tmp_dir: Path, resource_dir: Path) -> None:
    r"""Copy every image the document includes into the compile directory.

    WHY NOT `TEXINPUTS`, which is what this used to rely on: it does not work for graphics under
    tectonic. Tectonic runs XeTeX behind its own I/O layer, which resolves images relative to the
    input file rather than through the TeX search path, so `\includegraphics{fig.png}` with the
    raw store on TEXINPUTS failed with "Unable to load picture or PDF file" while the file sat
    exactly where the variable pointed (measured 2026-09-08 against a real corpus figure). The
    docstring claimed the relative form worked and no test compiled a document with an image in
    it, so the claim survived a year of being false.

    Copying is engine-independent and bounded: only the handful of files a document actually
    names get copied, into the temporary directory that is already the compile root. TEXINPUTS
    stays set, because it still does the job it really does — finding `.sty`/`.tex` includes.

    A named file that is not in `resource_dir` raises here rather than 40 lines into a TeX log.
    """
    for name in dict.fromkeys(_INCLUDEGRAPHICS.findall(source)):
        name = name.strip()
        # An absolute path needs no staging: both engines read it directly, and rewriting it
        # would break a caller that deliberately pointed outside the resource directory.
        if not name or Path(name).is_absolute():
            continue

        candidates = [resource_dir / name]
        if not Path(name).suffix:
            candidates += [resource_dir / f"{name}{suffix}" for suffix in _GRAPHICS_SUFFIXES]
        found = next((c for c in candidates if c.is_file()), None)
        if found is None:
            raise RuntimeError(
                f"the document includes '{name}' but no such image is in {resource_dir}. "
                "Give `\\includegraphics` a filename that exists there, or an absolute path."
            )

        # Land it under the name the document used, so the reference resolves unchanged.
        dest = tmp_dir / name
        if found.suffix and not Path(name).suffix:
            dest = tmp_dir / f"{name}{found.suffix}"
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(found, dest)


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
    timeout_s: float | None = None,
) -> Path:
    r"""Compile LaTeX source to a PDF at `out_pdf`. Returns the output path.

    `tex` may be a fragment or a whole document (see `build_document`). Compilation happens in
    an isolated temporary directory so the engine's write path is never the repo — the same
    containment `render_markdown_to_pdf` gets from Typst's `root`.

    `resource_dir` is where relative `\includegraphics{fig}` names are resolved from: every image
    the document names is copied into the compile directory before the engine runs (see
    `_stage_graphics`, and note that setting TEXINPUTS alone does NOT achieve this under
    tectonic). It is also prepended to TEXINPUTS, which is what finds a relative `.sty`/`.tex`
    include. Absolute paths in `\includegraphics` are left alone. `send_latex` defaults this to
    the raw store so corpus figures are includable by their stored filename.

    `timeout_s` caps ONE engine invocation (the pdflatex path runs two, each capped). It defaults
    to `DEFAULT_TIMEOUT_S`, which is sized for the one-off cold-cache package fetch rather than
    for a compile — see that constant for the measurement.

    Raises `RuntimeError` carrying the engine's own error lines. It raises rather than degrading
    for the reason `send_markdown` does: every caller reports to a human who can fix the source,
    and a blank PDF delivered to the tablet is the silent-failure class this codebase exists to
    resist (CLAUDE.md §3). A TIMEOUT is raised as `RuntimeError` too, deliberately: the native
    `subprocess.TimeoutExpired` is a `SubprocessError` and so slipped through every caller's
    `except RuntimeError`, arriving at the MCP client as an empty transport failure.
    """
    # ALWAYS through `available_engine`, never `engine or available_engine(engine)`. That form
    # short-circuits on any truthy name, and `[reading].latex_engine` is a non-empty string with
    # a default — so every real send skipped the PATH check entirely and the documented
    # fall-through was unreachable code. It failed exactly where you would least see it: the MCP
    # server is launched `uv run`, whose PATH does not carry ~/.local/bin, so `tectonic` (which
    # lives there) raised FileNotFoundError while /usr/bin/pdflatex sat on that same PATH unused.
    # `available_engine` already treats its argument as a PREFERENCE and falls through, which is
    # what both this module's and `config`'s docstrings promised the whole time.
    engine = available_engine(engine)
    # Defaulted from a module constant, NOT by reading config here: this module takes `geometry`
    # and `engine` as arguments for the same reason — `send_latex` owns the config object and
    # passes what it holds, and a renderer that loaded `config.toml` itself would make every test
    # that renders depend on a gitignored file (CLAUDE.md §13).
    limit = float(DEFAULT_TIMEOUT_S if timeout_s is None else timeout_s)
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
        if resource_dir:
            _stage_graphics(source, tmp_dir, Path(resource_dir).resolve())

        # pdflatex needs a second pass to resolve refs/ToC; tectonic reruns internally and a
        # second invocation would only pay the cost twice.
        passes = 1 if engine == "tectonic" else 2
        for attempt in range(passes):
            try:
                proc = subprocess.run(
                    _engine_argv(engine, src, tmp_dir),
                    capture_output=True,
                    text=True,
                    cwd=tmp_dir,
                    env=env,
                    timeout=limit,
                )
            except subprocess.TimeoutExpired as exc:
                # `TimeoutExpired` is a `SubprocessError`, NOT a `RuntimeError` — so before this
                # branch existed it flew past every caller's handler, including the MCP tool's
                # `except RuntimeError`, and reached the client as a bare transport failure with
                # no message. That is worse than an unhelpful error: the model reading it cannot
                # tell a timeout from a dead server, so the reported diagnosis was "Locus is
                # down" while Locus was fine and the engine was simply still fetching packages.
                # Re-raised as `RuntimeError` so it travels the path this module documents.
                raise RuntimeError(
                    f"LaTeX compile timed out ({engine}, pass {attempt + 1}) after "
                    f"{limit:g}s. This is USUALLY NOT the document: on a cold cache "
                    f"{engine} fetches the pgf/tikz package tree on first use, and "
                    "`_DIAGRAM_SETUP` loads tikz and pgfplots for every document, so a "
                    "one-line fragment pays the same fetch as a diagram-heavy one. Check the "
                    "network, then retry — a partly-filled cache resumes rather than "
                    "restarting — or raise `[reading].latex_timeout_s`. If the cache is "
                    "already warm, suspect an unterminated group or a runaway loop."
                ) from exc
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
    timeout_s: float | None = None,
) -> Path:
    """Render a `.tex` file to a PDF, titling a fragment from the file stem.

    `resource_dir` defaults to the file's own directory, so a document that includes a figure
    sitting beside it compiles the way it reads. `timeout_s` forwards to `render_latex_to_pdf`,
    so `locus read x.tex` gets the same cold-cache headroom a send does — this path compiles the
    same preamble and would otherwise have its own, stricter ceiling by accident.
    """
    tex_path = Path(tex_path)
    return render_latex_to_pdf(
        tex_path.read_text(encoding="utf-8"),
        out_pdf,
        geometry=geometry,
        title=tex_path.stem.replace("_", " "),
        engine=engine,
        timeout_s=timeout_s,
        resource_dir=tex_path.parent,
    )
