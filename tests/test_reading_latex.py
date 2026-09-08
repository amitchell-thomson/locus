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
    assert "\\documentclass[11pt]{article}" in pre


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
