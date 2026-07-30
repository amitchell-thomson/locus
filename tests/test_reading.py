"""`locus read` (agent-layer §8.5, Phase 0.5) — two independently-testable pieces.

The render path (md2pdf) runs the real pandoc+typst toolchain (fast, deterministic, no model),
skipped cleanly when the `[reading]` extra is absent. The delivery path (deliver_remarkable)
never touches a device: `rmapi` is behind an injectable runner, so we assert the exact argv.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.reading.deliver_remarkable import DeliveryResult, deliver_pdf
from locus.reading.md2pdf import PageGeometry, markdown_to_typst


def _toolchain_available() -> bool:
    try:
        import pypandoc  # noqa: F401
        import typst  # noqa: F401
    except ImportError:
        return False
    return True


requires_toolchain = pytest.mark.skipif(
    not _toolchain_available(), reason="[reading] extra (pypandoc-binary + typst) not installed"
)


# ---------- render path ----------

def test_geometry_preamble_carries_page_size():
    pre = PageGeometry(width_in=7.07, height_in=9.43, margin_in=0.5, font_pt=11).typst_preamble()
    assert "width: 7.07in" in pre
    assert "height: 9.43in" in pre
    assert "margin: 0.5in" in pre
    assert "size: 11pt" in pre


@requires_toolchain
def test_markdown_math_converts_to_native_typst_math():
    """The whole point of pandoc's typst writer: LaTeX math becomes NATIVE typst math, so the
    compile step needs no LaTeX packages. Guards against a silent switch to a lossy converter."""
    out = markdown_to_typst(r"An integral $\int_0^1 x\,dx$ and a root $\sqrt{2}$.")
    assert "integral" in out          # \int -> integral (native typst), not a raw LaTeX string
    assert "sqrt(2)" in out           # \sqrt{2} -> sqrt(2)
    assert "\\int" not in out         # no leftover LaTeX control sequences


@requires_toolchain
def test_render_produces_a_valid_pdf(tmp_path: Path):
    from locus.reading.md2pdf import render_markdown_to_pdf

    out = render_markdown_to_pdf(
        "# Title\n\nBody with math $e^{i\\pi}+1=0$.\n",
        tmp_path / "doc.pdf",
        geometry=PageGeometry(),
    )
    data = out.read_bytes()
    assert data[:5] == b"%PDF-"
    assert len(data) > 1000


@requires_toolchain
def test_render_file_titles_from_stem_when_no_h1(tmp_path: Path):
    from locus.reading.md2pdf import render_markdown_file

    md = tmp_path / "regime_notes.md"
    md.write_text("Just a paragraph, no heading.\n", encoding="utf-8")
    out = render_markdown_file(md, tmp_path / "regime_notes.pdf")
    assert out.read_bytes()[:5] == b"%PDF-"


# ---------- delivery path (device-free) ----------

class FakeRmapi:
    """Records argv and returns scripted (code, out, err) per subcommand."""

    def __init__(self, responses: dict[str, tuple[int, str, str]]):
        self.responses = responses
        self.calls: list[list[str]] = []

    def __call__(self, args: list[str]) -> tuple[int, str, str]:
        self.calls.append(args)
        return self.responses.get(args[0], (0, "", ""))


def test_deliver_creates_folder_then_puts(tmp_path: Path):
    pdf = tmp_path / "doc.pdf"
    pdf.write_bytes(b"%PDF-1.7\n...")
    fake = FakeRmapi({"mkdir": (0, "created", ""), "put": (0, "uploaded", "")})

    result = deliver_pdf(pdf, remote_folder="Locus", runner=fake)

    assert result == DeliveryResult(remote_folder="Locus", filename="doc.pdf", created_folder=True)
    assert fake.calls[0] == ["mkdir", "Locus"]
    assert fake.calls[1] == ["put", str(pdf), "Locus"]


def test_deliver_tolerates_existing_folder(tmp_path: Path):
    pdf = tmp_path / "doc.pdf"
    pdf.write_bytes(b"%PDF-1.7\n...")
    # rmapi mkdir errors when the folder exists; that must not fail the delivery.
    fake = FakeRmapi({"mkdir": (1, "", "entry already exists"), "put": (0, "", "")})

    result = deliver_pdf(pdf, remote_folder="Locus", runner=fake)

    assert result.created_folder is False
    assert fake.calls[-1][0] == "put"


def test_deliver_raises_on_put_failure(tmp_path: Path):
    pdf = tmp_path / "doc.pdf"
    pdf.write_bytes(b"%PDF-1.7\n...")
    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (1, "", "network unreachable")})

    with pytest.raises(RuntimeError, match="rmapi put"):
        deliver_pdf(pdf, remote_folder="Locus", runner=fake)


def test_deliver_missing_file_raises(tmp_path: Path):
    with pytest.raises(FileNotFoundError):
        deliver_pdf(tmp_path / "nope.pdf", runner=FakeRmapi({}))


class SequencedRmapi(FakeRmapi):
    """Like FakeRmapi, but `put` returns a different scripted result on each call."""

    def __init__(self, responses, put_sequence):
        super().__init__(responses)
        self.put_sequence = list(put_sequence)

    def __call__(self, args):
        self.calls.append(args)
        if args[0] == "put" and self.put_sequence:
            return self.put_sequence.pop(0)
        return self.responses.get(args[0], (0, "", ""))


def test_deliver_replaces_an_existing_entry_when_asked(tmp_path: Path):
    """The deploy-day bug: rmapi REFUSES a same-named re-upload, it does not duplicate.

    A recurring delivery therefore works exactly once and fails every run afterwards. Caught
    on 2026-07-30 by running the daily systemd unit rather than only the command by hand.
    """
    pdf = tmp_path / "daily-2026-07-30.pdf"
    pdf.write_bytes(b"%PDF-1.7\n...")
    fake = SequencedRmapi(
        {"mkdir": (1, "", "entry already exists")},
        put_sequence=[(1, "", "entry already exists (use --force ...)"), (0, "replaced", "")],
    )

    result = deliver_pdf(pdf, remote_folder="Locus", replace=True, runner=fake)

    assert result.filename == "daily-2026-07-30.pdf"
    puts = [c for c in fake.calls if c[0] == "put"]
    assert puts[1] == ["put", "--content-only", str(pdf), "Locus"]


def test_deliver_does_not_replace_unless_asked(tmp_path: Path):
    """Default stays strict: silently overwriting a page the owner annotated would lose it."""
    pdf = tmp_path / "doc.pdf"
    pdf.write_bytes(b"%PDF-1.7\n...")
    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (1, "", "entry already exists")})

    with pytest.raises(RuntimeError, match="already exists"):
        deliver_pdf(pdf, remote_folder="Locus", runner=fake)
    assert not any("--content-only" in c for c in fake.calls)
