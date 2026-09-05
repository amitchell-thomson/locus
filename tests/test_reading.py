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


# ---------- ad-hoc markdown sends (reading/send.py) ----------

def _send_cfg(tmp_folder: str = "Inbox"):
    """A config stub for the send path. Pinned, NOT read from config.toml — that file is
    gitignored, so a test inheriting it passes or fails per machine (CLAUDE.md §13)."""
    import types

    return types.SimpleNamespace(
        reading=types.SimpleNamespace(
            rmapi_binary="rmapi",
            target_folder="Daily",
            send_folder=tmp_folder,
            page_width_in=7.07,
            page_height_in=9.43,
            margin_in=0.5,
            font_pt=11.0,
        )
    )


def test_send_rejects_empty_markdown():
    from locus.reading.send import send_markdown

    with pytest.raises(ValueError, match="empty"):
        send_markdown("   \n", title="Nothing", cfg=_send_cfg(), runner=FakeRmapi({}))


def test_send_ensures_every_folder_level():
    """`rmapi mkdir` does not create intermediate directories, so a nested send_folder needs
    one call per level — the bug `deliver.ensure_reading_folders` already fixed for Reading/."""
    from locus.reading.send import _ensure_folder_path

    fake = FakeRmapi({"mkdir": (0, "", "")})
    _ensure_folder_path(fake, "Inbox/From Claude")
    assert fake.calls == [["mkdir", "Inbox"], ["mkdir", "Inbox/From Claude"]]


@requires_toolchain
def test_send_renders_and_puts_into_the_send_folder(tmp_path: Path):
    """The send lands in send_folder — never /Daily (the ink inbox) and never a Reading
    subfolder that loop_b would auto-ingest."""
    from locus.reading.send import send_markdown

    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")})
    sent = send_markdown(
        "Some body text.\n", title="Kalman notes", cfg=_send_cfg(), runner=fake
    )

    put = [c for c in fake.calls if c[0] == "put"][0]
    assert put[-1] == "Inbox"          # its own folder: not /Daily, not /Notes, not Reading/
    assert put[-2].endswith(f"{sent.filename}")
    assert sent.filename.endswith("Kalman notes.pdf")   # date-prefixed by safe_filename
    assert sent.filename[:4].isdigit()
    assert sent.pages == 1
    assert sent.device_path == f"/Inbox/{sent.filename}"


@requires_toolchain
def test_send_replaces_on_a_same_day_resend(tmp_path: Path):
    """safe_filename dates the file, so a resend the same day collides — and `rmapi put`
    REFUSES a same-name upload rather than duplicating it. Without replace=True, sending the
    same title twice in one day would work once and fail every time after."""
    from locus.reading.send import send_markdown

    fake = SequencedRmapi(
        {"mkdir": (1, "", "entry already exists")},
        put_sequence=[(1, "", "entry already exists"), (0, "replaced", "")],
    )
    send_markdown("Body.\n", title="Same day", cfg=_send_cfg(), runner=fake)

    puts = [c for c in fake.calls if c[0] == "put"]
    assert puts[1][1] == "--content-only"


# ---------- existing-PDF sends (reading/send.send_pdf) ----------

def _a_pdf(path: Path, pages: int = 1) -> Path:
    """A byte-valid PDF. Content does not matter — `resolve_pdf` checks the header, and the
    page count is read best-effort, so a minimal file exercises every branch under test."""
    path.write_bytes(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<<>>\nendobj\ntrailer\n%%EOF\n")
    return path


def test_resolve_pdf_rejects_a_file_that_is_not_a_pdf(tmp_path: Path):
    """`deliver_pdf` uploads whatever bytes it is given under a .pdf name, so a renamed .docx
    or a saved HTML error page becomes a device document that opens to nothing — which reads
    as a device fault rather than a send fault."""
    from locus.reading.send import resolve_pdf

    impostor = tmp_path / "notreally.pdf"
    impostor.write_bytes(b"<html><body>rate limited</body></html>")
    with pytest.raises(ValueError, match="not a PDF"):
        resolve_pdf(impostor)


def test_resolve_pdf_rejects_an_empty_file(tmp_path: Path):
    from locus.reading.send import resolve_pdf

    empty = tmp_path / "empty.pdf"
    empty.write_bytes(b"")
    with pytest.raises(ValueError, match="empty"):
        resolve_pdf(empty)


def test_missing_pdf_names_what_it_tried_and_why(tmp_path: Path):
    """The error is the guard against the one failure this mode can have: being called from a
    machine that does not share a filesystem with the Locus server. A bare FileNotFoundError
    cannot distinguish that from a typo, and the two have completely different fixes."""
    from locus.reading.send import resolve_pdf

    with pytest.raises(FileNotFoundError) as exc:
        resolve_pdf("does/not/exist.pdf")
    msg = str(exc.value)
    assert "Tried:" in msg
    assert "does/not/exist.pdf" in msg
    assert "another machine" in msg


def test_resolve_pdf_falls_back_to_the_checkout_root(tmp_path: Path, monkeypatch):
    """'a file in the project' is the second thing this is for, and `docs/plan.pdf` is how a
    person says it — so a relative path is tried against the repo root, not just the cwd."""
    from locus.reading import send as S

    monkeypatch.setattr(S, "_repo_root", lambda: tmp_path)
    (tmp_path / "docs").mkdir()
    _a_pdf(tmp_path / "docs" / "plan.pdf")

    assert S.resolve_pdf("docs/plan.pdf") == tmp_path / "docs" / "plan.pdf"


def test_send_pdf_pushes_unchanged_under_the_given_title(tmp_path: Path):
    """The bytes must arrive as they are — nothing re-renders an existing PDF — but the DEVICE
    name comes from the filename `rmapi put` is handed, so the title is applied by staging a
    copy rather than by touching the caller's file."""
    from locus.reading.send import send_pdf

    src = _a_pdf(tmp_path / "generated-report.pdf")
    original = src.read_bytes()
    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")})

    sent = send_pdf(src, title="Q3 Report", cfg=_send_cfg(), runner=fake)

    put = [c for c in fake.calls if c[0] == "put"][0]
    assert put[-1] == "Inbox"
    staged = Path(put[-2])
    assert staged.name == sent.filename
    assert sent.filename.endswith("Q3 Report.pdf")
    assert sent.filename[:4].isdigit()          # date-prefixed by safe_filename
    assert src.read_bytes() == original         # the caller's file is never mutated
    assert sent.device_path == f"/Inbox/{sent.filename}"


def test_send_pdf_titles_from_the_filename_by_default(tmp_path: Path):
    from locus.reading.send import send_pdf

    src = _a_pdf(tmp_path / "Kalman derivation.pdf")
    fake = FakeRmapi({"mkdir": (0, "", ""), "put": (0, "", "")})

    sent = send_pdf(src, cfg=_send_cfg(), runner=fake)

    assert sent.filename.endswith("Kalman derivation.pdf")


def test_send_pdf_refuses_something_absurdly_large(tmp_path: Path, monkeypatch):
    """A typo guard, not a policy on document length: an rmapi upload that runs for minutes and
    then fails is the worst way to find out you pointed at the wrong file."""
    from locus.reading import send as S

    monkeypatch.setattr(S, "_MAX_PDF_BYTES", 32)
    src = _a_pdf(tmp_path / "big.pdf")
    with pytest.raises(ValueError, match="send guard"):
        S.send_pdf(src, cfg=_send_cfg(), runner=FakeRmapi({}))
