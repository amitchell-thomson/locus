"""LaTeX -> device PDF (reading/tex2pdf) and the LaTeX send path.

The owner asked (2026-09-08) that anything Claude WRITES for the tablet be authored in LaTeX
rather than markdown. These tests cover the three things that can silently go wrong with that:

  1. a fragment loses the device geometry (a US-Letter document on a 7in screen looks nearly
     right on a laptop and wrong on the tablet),
  2. a full document gets "helpfully" wrapped, so the author's own preamble is overridden by
     the one they wrote the document to replace,
  3. a compile failure reaches the caller as something other than the LaTeX error, leaving them
     no way to fix the source.

Source-shaping tests are pure string work and always run. The ones that actually compile are
skipped cleanly when no LaTeX engine is installed, matching how `test_reading.py` guards the
pandoc+typst toolchain. Delivery never touches a device — `rmapi` stays behind the injectable
runner from `test_reading.py`.
"""

from __future__ import annotations

import types
from pathlib import Path

import pytest

from locus.reading.md2pdf import PageGeometry
from locus.reading.tex2pdf import (
    build_document,
    escape_text,
    is_full_document,
    latex_preamble,
    render_latex_to_pdf,
)

from tests.test_reading import FakeRmapi


def _engine_available() -> bool:
    from locus.reading.tex2pdf import available_engine

    try:
        available_engine()
    except RuntimeError:
        return False
    return True


requires_latex = pytest.mark.skipif(
    not _engine_available(), reason="no LaTeX engine (tectonic / pdflatex) on PATH"
)


def _send_cfg(folder: str = "Inbox", engine: str = "tectonic"):
    """Config stub for the send path. Pinned, NOT read from config.toml — that file is
    gitignored, so a test inheriting it passes or fails per machine (CLAUDE.md §13)."""
    return types.SimpleNamespace(
        reading=types.SimpleNamespace(
            rmapi_binary="rmapi",
            target_folder="Daily",
            send_folder=folder,
            latex_font_pt=9.0,
            page_width_in=7.07,
            page_height_in=9.43,
            margin_in=0.5,
            font_pt=11.0,
            latex_engine=engine,
        )
    )


# ---------- source shaping (no engine needed) ----------

def test_preamble_carries_the_device_geometry():
    """The reason fragments are wrapped at all. A document that loses this is typeset for the
    wrong paper, which is invisible until it is on the tablet."""
    pre = latex_preamble(PageGeometry(width_in=7.07, height_in=9.43, margin_in=0.5, font_pt=11))
    assert "paperwidth=7.07in" in pre
    assert "paperheight=9.43in" in pre
    assert "margin=0.5in" in pre
    assert "\\documentclass[11pt,twocolumn]{extarticle}" in pre


def test_preamble_loads_the_packages_the_tool_promises():
    """The MCP docstring tells the client model it can use these without declaring them. If a
    package is dropped here, every document that took the docstring at its word fails to
    compile — and the model has no way to know why."""
    pre = latex_preamble(PageGeometry())
    for package in ("amsmath", "graphicx", "booktabs", "enumitem", "hyperref", "microtype"):
        assert package in pre


def test_preamble_avoids_engine_specific_packages():
    """`fontspec`/`unicode-math` would bind the document to XeTeX/LuaTeX, so the pdflatex
    fallback would produce a DIFFERENT document rather than the same one."""
    pre = latex_preamble(PageGeometry())
    assert "fontspec" not in pre
    assert "unicode-math" not in pre


def test_a_fragment_is_wrapped_and_titled():
    out = build_document("Some prose and $x^2$.", title="Kalman notes")
    assert "\\documentclass" in out
    assert "\\begin{document}" in out and "\\end{document}" in out
    assert "\\section*{Kalman notes}" in out
    assert "Some prose and $x^2$." in out


def test_a_full_document_is_compiled_untouched():
    """The escape hatch. A caller who wrote their own preamble asked for exactly it; wrapping
    it would silently override the layout they went to the trouble of specifying."""
    doc = (
        "\\documentclass[12pt]{article}\n"
        "\\usepackage[paperwidth=8.5in,paperheight=11in]{geometry}\n"
        "\\begin{document}\nMine.\n\\end{document}\n"
    )
    assert is_full_document(doc)
    assert build_document(doc, title="Ignored") == doc


def test_a_commented_documentclass_is_still_a_fragment():
    """`is_full_document` is a search, so a fragment that merely MENTIONS the macro in a comment
    must not be mistaken for a whole document — that would skip the preamble and fail to
    compile, with nothing in the source to explain why."""
    frag = "% you could use \\documentclass here\nJust prose.\n"
    assert not is_full_document(frag)
    assert "\\begin{document}" in build_document(frag, title="T")


def test_title_is_not_duplicated_when_the_body_opens_with_a_heading():
    out = build_document("\\section{Regime detection}\n\nBody.", title="Regime detection")
    assert out.count("Regime detection") == 1
    assert "\\section*{" not in out


def test_title_is_escaped_but_the_body_is_not():
    """The title arrives as a plain string and may hold TeX syntax; the BODY is LaTeX and
    escaping it would defeat the entire point of the module."""
    assert escape_text("Risk & return: 50% _done_") == "Risk \\& return: 50\\% \\_done\\_"
    out = build_document("$\\alpha_i = \\beta$", title="P&L 100%")
    assert "\\section*{P\\&L 100\\%}" in out
    assert "$\\alpha_i = \\beta$" in out          # body passed through verbatim


def test_control_characters_are_stripped():
    """Maths PDFs extract symbol-font glyphs as C0 control codes (CLAUDE.md §6), and that text
    gets pasted into documents written ABOUT those PDFs. A NUL in TeX source truncates the file
    for some readers. Tab and newline are legal TeX and must survive."""
    out = build_document("before\x00\x07after\n\tindented", title="T")
    assert "\x00" not in out and "\x07" not in out
    assert "beforeafter" in out
    assert "\tindented" in out


# ---------- compilation (needs an engine) ----------

@requires_latex
def test_fragment_compiles_to_a_real_pdf(tmp_path: Path):
    out = render_latex_to_pdf(
        "Prose, an equation \\begin{align} r &= Bf + \\epsilon \\end{align} and a list:\n"
        "\\begin{itemize}\\item one\\item two\\end{itemize}",
        tmp_path / "note.pdf",
        title="Factor model",
    )
    assert out.is_file()
    assert out.read_bytes().startswith(b"%PDF")


@requires_latex
def test_a_booktabs_table_compiles(tmp_path: Path):
    r"""REGRESSION (2026-09-08, caught on the first realistic sample, not by a test).

    The preamble originally used plain `\raggedright`, which does `\let\\\@centercr` — so `\\`
    stopped ending a table row and the next `\midrule` died with "Misplaced \noalign". Every
    document containing a table failed to compile, and `booktabs` is loaded BY this preamble and
    advertised in the MCP docstring, so the tool was inviting callers into the one construct it
    could not render. `ragged2e`'s `\RaggedRight` leaves `\\` alone.
    """
    out = render_latex_to_pdf(
        "\\begin{tabular}{lrr}\n\\toprule\nMethod & A & B \\\\\n\\midrule\n"
        "Proportional & 3.4 & 3.3 \\\\\nRisk parity & 3.0 & 2.5 \\\\\n"
        "\\bottomrule\n\\end{tabular}",
        tmp_path / "table.pdf",
        title="Sizing",
    )
    assert out.read_bytes().startswith(b"%PDF")


@requires_latex
def test_numbered_equations_and_lists_compile(tmp_path: Path):
    """The three constructs LaTeX was chosen for, in one document — `align` with numbering,
    inline math, and a tightened list. A preamble change that breaks any of them breaks the
    reason the format was picked."""
    out = render_latex_to_pdf(
        "Text with \\(w_i \\propto \\bar r_i\\).\n"
        "\\begin{align} r_i &= \\sum_{k=1}^{K} \\beta_{ik} f_k + \\epsilon_i \\end{align}\n"
        "\\begin{itemize}\\item one\\item two\\end{itemize}",
        tmp_path / "math.pdf",
        title="Factors",
    )
    assert out.read_bytes().startswith(b"%PDF")


@pytest.mark.parametrize("engine", ["tectonic", "pdflatex"])
def test_every_installed_engine_renders_the_same_document(engine, tmp_path: Path):
    r"""REGRESSION (2026-09-08). The fallback was documented and did not work.

    Under pdflatex the preamble died with "pdfTeX error (font expansion): auto expansion is only
    possible with scalable fonts" and wrote NO PDF — T1 Computer Modern resolves to bitmap fonts
    on a stock TeX Live and `microtype`'s expansion needs Type 1. `lmodern` fixed it. This ran
    green on tectonic throughout, which is the point: a fallback only tested through the
    preferred engine is a claim, not a fallback.

    Skips per-engine rather than for the pair, so a machine with only one still asserts on it.
    """
    import shutil as _shutil

    if not _shutil.which(engine):
        pytest.skip(f"{engine} not installed")
    fitz = pytest.importorskip("fitz")

    out = render_latex_to_pdf(
        "Prose, \\(x^2\\), and a table:\n"
        "\\begin{tabular}{lr}\\toprule A & 1 \\\\\\midrule B & 2 \\\\\\bottomrule\\end{tabular}",
        tmp_path / f"{engine}.pdf",
        title="Engine check",
        engine=engine,
    )
    with fitz.open(out) as doc:
        assert doc.page_count == 1
        assert doc[0].rect.width == pytest.approx(7.07 * 72, abs=2)


@requires_latex
def test_compiled_page_is_the_device_size(tmp_path: Path):
    """The geometry has to survive all the way into the PDF, not just into the source. 7.07in
    at 72pt/in = 508.9pt."""
    fitz = pytest.importorskip("fitz")
    out = render_latex_to_pdf("Body.", tmp_path / "geo.pdf", title="Geometry")
    with fitz.open(out) as doc:
        rect = doc[0].rect
    assert rect.width == pytest.approx(7.07 * 72, abs=2)
    assert rect.height == pytest.approx(9.43 * 72, abs=2)


@requires_latex
def test_a_broken_document_raises_with_the_latex_error(tmp_path: Path):
    """The failure has to be FIXABLE: the caller wrote the source, so the engine's own error
    line is the whole value of the exception. A bare 'compile failed' would leave the model
    guessing, and a silently-empty PDF on the tablet is the §3 failure class."""
    with pytest.raises(RuntimeError) as exc:
        render_latex_to_pdf(
            "\\begin{align} x = 1 \\end{alynn}", tmp_path / "bad.pdf", title="Broken"
        )
    message = str(exc.value)
    assert "LaTeX compile failed" in message
    assert len(message.splitlines()) <= 15      # the error lines, not the whole log


@requires_latex
def test_full_document_geometry_wins(tmp_path: Path):
    """Proof the escape hatch reaches the PDF: an author-specified page size must not be
    overridden by the device default."""
    fitz = pytest.importorskip("fitz")
    out = render_latex_to_pdf(
        "\\documentclass{article}\n"
        "\\usepackage[paperwidth=5in,paperheight=7in,margin=0.4in]{geometry}\n"
        "\\begin{document}Mine.\\end{document}",
        tmp_path / "own.pdf",
        title="ignored",
    )
    with fitz.open(out) as doc:
        rect = doc[0].rect
    assert rect.width == pytest.approx(5 * 72, abs=2)
    assert rect.height == pytest.approx(7 * 72, abs=2)


# ---------- the send path ----------

def test_send_latex_rejects_empty_source():
    from locus.reading.send import send_latex

    with pytest.raises(ValueError, match="empty"):
        send_latex("   \n", title="Nothing", cfg=_send_cfg(), runner=FakeRmapi({}))


@requires_latex
def test_send_latex_lands_in_the_send_folder(tmp_path: Path):
    """Same folder rules as every other send: never /Daily (the ink inbox), never a Reading
    subfolder loop_b would auto-ingest, never /Notes (invariant 5)."""
    from locus.reading.send import send_latex

    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")})
    sent = send_latex(
        "A derivation: $e^{i\\pi} + 1 = 0$.", title="Euler", cfg=_send_cfg(), runner=fake
    )

    put = [c for c in fake.calls if c[0] == "put"][0]
    assert put[-1] == "Inbox"
    assert sent.filename.endswith("Euler.pdf")
    assert sent.filename[:4].isdigit()          # date-prefixed by safe_filename
    assert sent.pages == 1
    assert sent.device_path == f"/Inbox/{sent.filename}"


@requires_latex
def test_send_latex_does_not_push_when_the_compile_fails():
    """A failed render must not reach the device at all. Pushing whatever was on disk would put
    a truncated or stale document on the tablet under a name that says it is current."""
    from locus.reading.send import send_latex

    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")})
    with pytest.raises(RuntimeError, match="LaTeX compile failed"):
        send_latex("\\undefinedmacro{x}", title="Broken", cfg=_send_cfg(), runner=fake)

    assert not [c for c in fake.calls if c[0] == "put"]


def test_a_missing_engine_names_the_install_command(monkeypatch):
    """The one place this path is less reproducible than the markdown one (which installs from
    the lockfile). The miss has to say what to install rather than surfacing a bare
    FileNotFoundError from inside a subprocess."""
    from locus.reading import tex2pdf

    monkeypatch.setattr(tex2pdf.shutil, "which", lambda _name: None)
    with pytest.raises(RuntimeError, match="tectonic"):
        tex2pdf.available_engine()


def test_a_configured_engine_that_is_absent_falls_through(monkeypatch):
    """A stale `[reading].latex_engine` should not disable the feature — it should fall through
    to whatever IS installed. The alternative is a config value silently breaking every send."""
    from locus.reading import tex2pdf

    monkeypatch.setattr(tex2pdf.shutil, "which", lambda name: name == "pdflatex")
    assert tex2pdf.available_engine("tectonic") == "pdflatex"


# ---------- engine selection: the wiring, not just the helper ----------

def test_the_configured_engine_is_only_a_preference(monkeypatch, tmp_path: Path):
    r"""REGRESSION (2026-09-08). The fall-through was documented, unit-tested and unreachable.

    `render_latex_to_pdf` used to do `engine = engine or available_engine(engine)`.
    `[reading].latex_engine` is a non-empty string with a default, and `send_latex` passes it on
    every call, so the `or` short-circuited and `available_engine` never ran: no PATH check, no
    fall-through, straight to `subprocess` and `FileNotFoundError: 'tectonic'`.

    It bit only in production because the MCP server is launched `uv run`, whose PATH drops
    ~/.local/bin where tectonic lives, while every interactive shell has it. `/usr/bin/pdflatex`
    was installed and available the whole time.

    `test_a_configured_engine_that_is_absent_falls_through` below covers the same intent and
    passed throughout, because it calls `available_engine` DIRECTLY. That is the gap this closes:
    assert on the path the product actually takes.
    """
    from locus.reading import tex2pdf

    monkeypatch.setattr(tex2pdf.shutil, "which", lambda name: f"/usr/bin/{name}"
                        if name == "pdflatex" else None)

    used: list[str] = []

    def fake_run(argv, **kwargs):
        used.append(argv[0])
        (tmp_path / "out").mkdir(exist_ok=True)
        Path(kwargs["cwd"], "doc.pdf").write_bytes(b"%PDF-1.5\n")
        return types.SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(tex2pdf.subprocess, "run", fake_run)

    tex2pdf.render_latex_to_pdf("Prose.", tmp_path / "o.pdf", title="T", engine="tectonic")
    assert used and set(used) == {"pdflatex"}, f"asked for tectonic, should have used pdflatex: {used}"


# ---------- house style ----------

@pytest.mark.parametrize("dash", ["---", "—"])
def test_an_em_dash_is_refused(dash):
    """Enforced, not merely documented (his instruction, 2026-09-08). Only ever applies to prose
    the agent wrote: his own text reaches the device through `send_markdown`, not through here."""
    with pytest.raises(ValueError, match="em dash"):
        build_document(f"A sentence {dash} with a dash.", title="T")


def test_an_em_dash_in_the_title_is_refused():
    with pytest.raises(ValueError, match="em dash in the title"):
        build_document("Clean prose.", title=f"Notes — draft")


def test_the_em_dash_error_quotes_the_offending_text():
    """'There is an em dash somewhere in your 400 lines' is not a fixable report."""
    with pytest.raises(ValueError, match="tunnel coupling"):
        build_document("The tunnel coupling --- the hard one --- is last.", title="T")


def test_an_en_dash_and_a_maths_minus_are_left_alone():
    """`--` is a range, not an em dash, and `$a-b$` is arithmetic. Over-matching would make the
    rule unusable and it would get switched off."""
    out = build_document("Pages 3--5 and $a-b$ and a hyphen-joined word.", title="T")
    assert "3--5" in out and "$a-b$" in out and "hyphen-joined" in out


def test_an_em_dash_is_refused_in_a_full_document_too():
    """A `\\documentclass` document is still something the agent wrote; the style rule is about
    the prose, not about who supplied the preamble."""
    with pytest.raises(ValueError, match="em dash"):
        build_document("\\documentclass{article}\\begin{document}A --- b\\end{document}")


# ---------- layout ----------

def test_the_latex_layout_is_two_column_and_its_own_size():
    """Two columns at 9pt, where the markdown path is one column at 11pt. Sharing `font_pt`
    would mean resizing a document Claude wrote also resized his own relayed notes."""
    from locus.reading.send import latex_geometry, reading_geometry

    cfg = _send_cfg()
    assert latex_geometry(cfg).font_pt == 9.0
    assert reading_geometry(cfg).font_pt == 11.0
    assert "twocolumn" in latex_preamble(latex_geometry(cfg))


def test_a_font_size_the_document_class_cannot_set_is_refused():
    r"""MEASURED: `\documentclass[9.5pt]{article}` compiles clean and renders at 10pt, saying so
    only as "Unused global option" in the log. Refusing beats typesetting at a size nobody chose.
    """
    with pytest.raises(ValueError, match="not a size the document class implements"):
        latex_preamble(PageGeometry(width_in=7.07, height_in=9.43, margin_in=0.5, font_pt=9.5))


def test_the_title_spans_both_columns():
    """A `\\section*` title in a two-column document sits in the left column and reads as the
    first section's heading rather than as the document's name."""
    out = build_document("Opening prose.", title="Kalman notes")
    assert "\\twocolumn[" in out and "Kalman notes" in out


@requires_latex
def test_a_two_column_document_really_has_two_columns(tmp_path: Path):
    """Source-level assertions cannot see a class option that silently did nothing. Read the
    text back out of the PDF and check it arrives in two horizontal bands."""
    fitz = pytest.importorskip("fitz")

    out = render_latex_to_pdf(
        "word " * 900, tmp_path / "cols.pdf", title="Columns",
        geometry=PageGeometry(width_in=7.07, height_in=9.43, margin_in=0.5, font_pt=9.0),
    )
    with fitz.open(out) as doc:
        lefts = {round(b[0]) for b in doc[0].get_text("blocks") if b[4].strip()}
    assert len(lefts) >= 2, f"expected two column origins, got {sorted(lefts)}"
    assert max(lefts) - min(lefts) > 100, f"columns too close to be real: {sorted(lefts)}"


# ---------- diagrams and images ----------

def test_the_diagram_packages_are_always_loaded():
    """The tool docstring tells the authoring model it can draw with TikZ/pgfplots without
    declaring anything. Drop these and every document that believed it fails to compile."""
    pre = latex_preamble(PageGeometry())
    for package in ("tikz", "pgfplots"):
        assert package in pre
    assert "cycle list name=eink" in pre, "greyscale-safe plot styling is the point on e-ink"


@requires_latex
def test_a_tikz_diagram_and_a_pgfplots_axis_compile(tmp_path: Path):
    """The claim the docstring makes, tested end to end rather than by grepping the preamble."""
    fitz = pytest.importorskip("fitz")

    out = render_latex_to_pdf(
        "\\begin{tikzpicture}\\node[draw] (a) {x}; \\node[draw,right=1cm of a] (b) {y};"
        "\\draw[->] (a) -- (b);\\end{tikzpicture}\n\n"
        "\\begin{tikzpicture}\\begin{axis}[width=5cm,height=3cm]"
        "\\addplot coordinates {(0,1) (1,2)};\\addplot coordinates {(0,2) (1,1)};"
        "\\end{axis}\\end{tikzpicture}",
        tmp_path / "tikz.pdf",
        title="Drawn",
    )
    with fitz.open(out) as doc:
        assert doc.page_count >= 1


@requires_latex
def test_an_included_image_is_staged_into_the_compile(tmp_path: Path):
    r"""REGRESSION (2026-09-08). `resource_dir` was documented as making relative
    `\includegraphics` resolve, via TEXINPUTS. It does not under tectonic, which resolves images
    relative to the input file rather than through the TeX search path: the compile failed with
    "Unable to load picture or PDF file" while the file sat exactly where TEXINPUTS pointed.
    No test had ever compiled a document containing an image, so the claim went a year unchecked.
    """
    fitz = pytest.importorskip("fitz")

    assets = tmp_path / "assets"
    assets.mkdir()
    # A PNG, because that is what the corpus stores: `figures.raw_path` points at
    # `<hash>_figN.png` under the raw store, which is the case this exists to serve.
    fitz.Pixmap(fitz.csRGB, fitz.IRect(0, 0, 120, 90)).save(assets / "diagram.png")

    out = render_latex_to_pdf(
        "\\includegraphics[width=3cm]{diagram.png}",
        tmp_path / "withimage.pdf",
        title="Figure",
        resource_dir=assets,
    )
    with fitz.open(out) as doc:
        assert doc[0].get_images(), "the image was staged but never made it into the PDF"


def test_an_image_that_is_not_in_the_resource_dir_names_itself(tmp_path: Path):
    """Better than the engine's own "Unable to load picture" 40 lines into a TeX log."""
    (tmp_path / "assets").mkdir()
    with pytest.raises(RuntimeError, match="no such image"):
        render_latex_to_pdf(
            "\\includegraphics{missing.png}",
            tmp_path / "o.pdf",
            title="T",
            resource_dir=tmp_path / "assets",
        )


def test_send_latex_defaults_the_resource_dir_to_the_raw_store(tmp_path: Path, monkeypatch):
    """So a corpus figure is includable by the filename `figures.raw_path` already holds, with
    nothing to arrange at the call site."""
    from locus.reading import send as send_mod

    cfg = _send_cfg()
    cfg.paths = types.SimpleNamespace(raw_store=tmp_path / "raw")
    seen: dict[str, object] = {}

    def fake_render(tex, out_pdf, **kwargs):
        seen.update(kwargs)
        Path(out_pdf).write_bytes(b"%PDF-1.5\n")
        return Path(out_pdf)

    monkeypatch.setattr(send_mod, "render_latex_to_pdf", fake_render)
    monkeypatch.setattr(send_mod, "_page_count", lambda _p: 1)
    monkeypatch.setattr(send_mod, "deliver_pdf", lambda *a, **k: None)

    send_mod.send_latex("Prose.", title="T", cfg=cfg,
                        runner=FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")}))
    assert seen["resource_dir"] == tmp_path / "raw"


def test_a_document_opening_with_its_first_section_still_gets_a_title():
    r"""REGRESSION (2026-09-08). The duplicate-title guard tested for the PRESENCE of an opening
    heading, and a normal document opens with `\section{...}` — so the title was suppressed on
    almost every real document and the page came out with nothing naming it. Invisible in the
    source and invisible in a page-count check; you only see it by looking at page 1.

    It matters more under the two-column layout, where `_title_block` is the only material set
    across the full measure, and the tablet shows no filename while a document is open.
    """
    out = build_document("\\section{The project in one sentence}\n\nBody.", title="Ares brief")
    assert "\\twocolumn[" in out and "\\section*{Ares brief}" in out
    assert "The project in one sentence" in out


def test_a_title_the_opening_heading_repeats_is_still_not_printed_twice():
    """The case the guard was for, kept: same name, printed once."""
    for heading in ("\\section{Regime detection}", "\\section*{regime  DETECTION}"):
        out = build_document(f"{heading}\n\nBody.", title="Regime detection")
        assert "\\twocolumn[" not in out, f"title should be suppressed for {heading!r}"


@requires_latex
def test_plots_default_to_greyscale(tmp_path: Path):
    """REGRESSION (2026-09-08). The screen is greyscale, so a plot separating its series by
    colour separates them by nothing.

    `cycle list name=eink` was originally folded into `every axis/.append style`, where pgfplots
    overwrites it with its own default list: every plot rendered in stock blue and red. A test
    grepping the preamble for the key passed the whole time, because the key WAS there. This one
    renders a two-series plot and looks at the pixels, which is the only thing that could tell
    the difference.
    """
    fitz = pytest.importorskip("fitz")

    out = render_latex_to_pdf(
        # BOTH kinds. `ybar` installs its own `bar cycle list` over the line one, so fixing the
        # line cycle alone left every bar chart blue and red, and the first version of this test
        # plotted only lines and passed while the document that prompted it was still coloured.
        "\\begin{tikzpicture}\\begin{axis}[width=6cm,height=4cm]"
        "\\addplot coordinates {(0,1) (1,3) (2,2)};"
        "\\addplot coordinates {(0,3) (1,1) (2,3)};"
        "\\end{axis}\\end{tikzpicture}\n\n"
        "\\begin{tikzpicture}\\begin{axis}[width=6cm,height=4cm,ybar]"
        "\\addplot coordinates {(0,1) (1,3) (2,2)};"
        "\\addplot coordinates {(0,3) (1,1) (2,3)};"
        "\\end{axis}\\end{tikzpicture}",
        tmp_path / "grey.pdf",
        title="Greyscale",
    )
    with fitz.open(out) as doc:
        pix = doc[0].get_pixmap(dpi=110)
        pixels = [pix.pixel(x, y) for y in range(0, pix.height, 3) for x in range(0, pix.width, 3)]

    coloured = [p for p in pixels if max(p[:3]) - min(p[:3]) > 24]
    assert not coloured, f"{len(coloured)} coloured pixels, e.g. {coloured[:4]} — plot is not greyscale"


# ---------- the compile timeout (2026-09-08 regression) ----------


def test_a_compile_timeout_raises_runtime_error_not_timeout_expired(monkeypatch, tmp_path: Path):
    """THE BUG THIS FILE EXISTS TO KEEP FIXED. `subprocess.TimeoutExpired` is a
    `SubprocessError`, not a `RuntimeError`, so it matched no handler in any caller — including
    the MCP tool's `except RuntimeError` — and crossed the boundary as a bare transport failure
    with no message. The model on the far side read "the call failed" and reported the server
    was down; the server was fine and tectonic was still fetching packages.

    Asserting `pytest.raises(RuntimeError)` is the whole point: `TimeoutExpired` would satisfy a
    bare `except Exception` test and still be the bug."""
    import subprocess as _sp

    from locus.reading import tex2pdf

    def _timeout(*_args, **kwargs):
        raise _sp.TimeoutExpired(cmd="tectonic", timeout=kwargs.get("timeout", 900))

    monkeypatch.setattr(tex2pdf.subprocess, "run", _timeout)
    with pytest.raises(RuntimeError) as exc:
        render_latex_to_pdf(r"\section{X}Body.", tmp_path / "out.pdf", title="T", engine="tectonic")

    message = str(exc.value)
    assert "timed out" in message
    # It must point at the cold-cache fetch, because that is what it almost always is, and a
    # message that only says "timed out" sends the reader to look at their LaTeX.
    assert "cold cache" in message
    assert "latex_timeout_s" in message


def test_the_configured_timeout_reaches_the_engine_call(monkeypatch, tmp_path: Path):
    """A ceiling nothing passes down is a constant with extra steps — the shape of the
    `available_engine` bug this module already carries a comment about."""
    import subprocess as _sp

    from locus.reading import tex2pdf

    seen: dict[str, float | None] = {}

    def _capture(*_args, **kwargs):
        seen["timeout"] = kwargs.get("timeout")
        raise _sp.TimeoutExpired(cmd="tectonic", timeout=kwargs.get("timeout") or 0)

    monkeypatch.setattr(tex2pdf.subprocess, "run", _capture)
    with pytest.raises(RuntimeError):
        render_latex_to_pdf(
            r"\section{X}B.", tmp_path / "o.pdf", title="T", engine="tectonic", timeout_s=42.0
        )
    assert seen["timeout"] == 42.0


def test_the_default_timeout_is_sized_for_a_cold_package_fetch():
    """MEASURED 2026-09-08: a one-line fragment on a cold tectonic cache hits `TimeoutExpired`
    at the old hardcoded 180s, while the same fragment warm compiles in 1.2s. The ceiling is
    sized for the one-off fetch, so a value near the warm time is the regression."""
    from locus.reading.tex2pdf import DEFAULT_TIMEOUT_S

    assert DEFAULT_TIMEOUT_S >= 600


def test_send_latex_does_not_push_when_the_compile_times_out(monkeypatch):
    """A timeout must be as un-pushable as a compile error. Before the fix it did not merely
    push the wrong thing — it escaped `send_latex` entirely."""
    import subprocess as _sp

    from locus.reading import tex2pdf
    from locus.reading.send import send_latex

    monkeypatch.setattr(
        tex2pdf.subprocess,
        "run",
        lambda *a, **k: (_ for _ in ()).throw(_sp.TimeoutExpired(cmd="tectonic", timeout=900)),
    )
    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")})
    with pytest.raises(RuntimeError, match="timed out"):
        send_latex(r"\section{X}B.", title="Slow", cfg=_send_cfg(), runner=fake)

    assert not [c for c in fake.calls if c[0] == "put"]
