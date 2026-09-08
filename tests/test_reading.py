"""`locus read` (agent-layer §8.5, Phase 0.5) — two independently-testable pieces.

The render path (md2pdf) runs the real pandoc+typst toolchain (fast, deterministic, no model),
skipped cleanly when the `[reading]` extra is absent. The delivery path (deliver_remarkable)
never touches a device: `rmapi` is behind an injectable runner, so we assert the exact argv.
"""

from __future__ import annotations

import argparse
import types
from pathlib import Path

import pytest

from locus import cli
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


# ---------- replace: keeping the device's page records honest ----------

def _stale_pdf(tmp_path: Path, pages: int = 2) -> Path:
    """A real multi-page PDF, so `_pdf_page_count` returns a number rather than None."""
    fitz = pytest.importorskip("fitz")
    path = tmp_path / "rebuilt.pdf"
    with fitz.open() as doc:
        for _ in range(pages):
            doc.new_page(width=200, height=300)
        doc.save(path)
    return path


def _replacing_runner():
    from tests.test_reading import SequencedRmapi

    return SequencedRmapi(
        {"mkdir": (1, "", "entry already exists"), "rm": (0, "", "")},
        put_sequence=[(1, "", "entry already exists"), (0, "replaced", "")],
    )


def _remote(*, ok: bool, page_count, has_ink: bool):
    from locus.reading.deliver_remarkable import RemoteDoc

    return lambda _p: RemoteDoc(ok=ok, page_count=page_count, has_ink=has_ink)


def test_a_page_count_change_with_no_ink_is_deleted_and_reput(tmp_path: Path):
    """REGRESSION (2026-09-08). `--content-only` swaps the PDF and KEEPS the device's per-page
    records, so a rebuild with a different length leaves them describing a document that no
    longer exists. Observed live: a 9-page brief rebuilt to 6 left `.content` reading
    `pageCount: 9`, and `rmapi geta` then refused the document with "page count too short".
    Nothing reports this; you find it by trying to read the document back."""
    pdf = _stale_pdf(tmp_path, pages=2)
    fake = _replacing_runner()

    result = deliver_pdf(pdf, remote_folder="Inbox", replace=True, runner=fake,
                         inspect=_remote(ok=True, page_count=9, has_ink=False))

    assert result.replaced == "delete+put"
    assert ["rm", "Inbox/rebuilt.pdf"] in fake.calls
    assert not any("--content-only" in c for c in fake.calls)


def test_ink_is_never_deleted_even_when_the_records_are_stale(tmp_path: Path):
    """The one rule that outranks correct metadata. Stale page records are recoverable; his
    handwriting is not, and `rm` does not come back."""
    pdf = _stale_pdf(tmp_path, pages=2)
    fake = _replacing_runner()

    result = deliver_pdf(pdf, remote_folder="Inbox", replace=True, runner=fake,
                         inspect=_remote(ok=True, page_count=9, has_ink=True))

    assert result.replaced == "content-only"
    assert not any(c[0] == "rm" for c in fake.calls)


def test_a_remote_that_cannot_be_inspected_is_not_deleted(tmp_path: Path):
    """`ok=False` is why `RemoteDoc` carries it separately from `has_ink`: a fetch that failed
    must not read as "no ink", because deletion is the only thing that flag gates. Under the
    reMarkable cloud's 429 rate limiting this is a live case, not a hypothetical."""
    pdf = _stale_pdf(tmp_path, pages=2)
    fake = _replacing_runner()

    result = deliver_pdf(pdf, remote_folder="Inbox", replace=True, runner=fake,
                         inspect=_remote(ok=False, page_count=None, has_ink=False))

    assert result.replaced == "content-only"
    assert not any(c[0] == "rm" for c in fake.calls)


def test_a_matching_page_count_keeps_the_cheap_path(tmp_path: Path):
    """Same length means the records still describe the document, so there is nothing to repair
    and no reason to spend a delete plus a fresh upload."""
    pdf = _stale_pdf(tmp_path, pages=2)
    fake = _replacing_runner()

    result = deliver_pdf(pdf, remote_folder="Inbox", replace=True, runner=fake,
                         inspect=_remote(ok=True, page_count=2, has_ink=False))

    assert result.replaced == "content-only"
    assert not any(c[0] == "rm" for c in fake.calls)


def test_an_injected_runner_does_not_reach_the_network_to_inspect(tmp_path: Path):
    """A caller that injected a transport injected the transport it wants used. Defaulting to the
    real inspector here would make every replace test shell out to `rmapi get` and hit the
    device — which it did, until the default was made conditional."""
    pdf = _stale_pdf(tmp_path, pages=2)
    fake = _replacing_runner()

    result = deliver_pdf(pdf, remote_folder="Inbox", replace=True, runner=fake)

    assert result.replaced == "content-only"
    assert not any(c[0] == "rm" for c in fake.calls)


# ---------- CLI wiring: `locus read` dispatches on the file suffix ----------
#
# WHY THIS SECTION EXISTS. `cmd_read` is a chain of suffix branches, and the markdown one — the
# one the command is named for — called `render_markdown_file` while importing only
# `render_markdown_to_pdf`, so every `locus read <x>.md` died with a NameError. The .pdf and
# .tex branches return before reaching that line, so they worked, and nothing tested the
# command itself. Same shape as the `cmd_discover` bug (§13): a dispatching CLI entry point
# has to be tested on the branches its callers actually take, not on the modules beneath them.
#
# The renderers and the device are stubbed, so what is under test is the control flow — which
# renderer each suffix reaches, with which arguments — not the toolchain. One toolchain-gated
# test at the end runs the markdown branch for real, because a stub cannot catch a call that
# hands a Path to a function expecting markdown text.


class _FakeReadingCfg:
    """Pinned config. NEVER `config.load()`: `config.toml` is gitignored, so a test that
    inherits it passes or fails per machine (§13)."""

    page_width_in = 7.07
    page_height_in = 9.43
    margin_in = 0.5
    font_pt = 11.0
    target_folder = "Locus/Inbox"
    rmapi_binary = "rmapi"
    latex_engine = "tectonic"


class _FakeCfg:
    reading = _FakeReadingCfg()


def _read_args(path: Path, **kw) -> argparse.Namespace:
    """Every flag the `read` subparser defines, defaulted as argparse would build them."""
    base = dict(path=str(path), title=None, to=None, out=None, no_push=False)
    base.update(kw)
    return argparse.Namespace(**base)


@pytest.fixture()
def read_calls(monkeypatch):
    """Record which renderer each suffix reaches. No toolchain, no network, no device."""
    calls: dict[str, list] = {"markdown": [], "latex": [], "sent_pdf": [], "delivered": []}

    monkeypatch.setattr(cli, "load", lambda: _FakeCfg())

    def fake_render_markdown_file(md_path, out_pdf, *, geometry=None):
        calls["markdown"].append((Path(md_path), Path(out_pdf), geometry))
        Path(out_pdf).write_bytes(b"%PDF-1.4 stub")
        return Path(out_pdf)

    def fake_render_latex_file(tex_path, out_pdf, *, geometry=None, engine=None, timeout_s=None):
        calls["latex"].append((Path(tex_path), Path(out_pdf), geometry, engine))
        Path(out_pdf).write_bytes(b"%PDF-1.4 stub")
        return Path(out_pdf)

    def fake_send_pdf(pdf, *, title=None, folder=None, cfg=None, runner=None):
        calls["sent_pdf"].append((Path(pdf), title, folder))
        return types.SimpleNamespace(filename=Path(pdf).name, device_path=f"/{folder}")

    def fake_deliver_pdf(pdf, *, remote_folder="", rmapi_binary="rmapi", **kw):
        calls["delivered"].append((Path(pdf), remote_folder))
        return DeliveryResult(remote_folder=remote_folder, filename=Path(pdf).name,
                              created_folder=False)

    # Patched on the modules, not on `cli`: cmd_read imports these inside the function body,
    # so the lookup happens at call time and picks these up.
    monkeypatch.setattr("locus.reading.md2pdf.render_markdown_file", fake_render_markdown_file)
    monkeypatch.setattr("locus.reading.tex2pdf.render_latex_file", fake_render_latex_file)
    monkeypatch.setattr("locus.reading.send.send_pdf", fake_send_pdf)
    monkeypatch.setattr("locus.reading.deliver_remarkable.deliver_pdf", fake_deliver_pdf)
    # The real one calls `config.load()` when given no cfg, which would read the live file.
    monkeypatch.setattr(
        "locus.reading.send.latex_geometry",
        lambda cfg=None: PageGeometry(width_in=7.07, height_in=9.43, margin_in=0.5, font_pt=9.0),
    )
    return calls


def test_read_renders_a_markdown_file(tmp_path: Path, read_calls, capsys):
    """THE REGRESSION. The markdown branch must reach the renderer that takes a PATH.

    It called `render_markdown_file` while importing `render_markdown_to_pdf`, so this branch
    raised NameError for every caller. The two are not interchangeable — one takes a path and
    titles from the stem, the other takes markdown text — so this asserts the file itself is
    what gets handed over, not just that something was called."""
    md = tmp_path / "probe.md"
    md.write_text("# Test\n\nBody.\n", encoding="utf-8")

    cli.cmd_read(_read_args(md, no_push=True))

    assert [c[0] for c in read_calls["markdown"]] == [md]
    assert read_calls["markdown"][0][1] == tmp_path / "probe.pdf"
    assert read_calls["delivered"] == []          # --no-push means render only
    assert "rendered probe.md" in capsys.readouterr().out


def test_read_pushes_a_rendered_markdown_file_when_not_told_otherwise(tmp_path: Path, read_calls):
    """The default path — render, then deliver. `--no-push` is the exception, not the shape."""
    md = tmp_path / "note.md"
    md.write_text("# Note\n\nBody.\n", encoding="utf-8")

    cli.cmd_read(_read_args(md))

    assert read_calls["delivered"] == [(tmp_path / "note.pdf", "Locus/Inbox")]


def test_read_renders_every_markdown_file_in_a_directory(tmp_path: Path, read_calls):
    """A directory is the other markdown entry point, and it reaches the same line."""
    (tmp_path / "a.md").write_text("# A\n", encoding="utf-8")
    (tmp_path / "b.md").write_text("# B\n", encoding="utf-8")
    (tmp_path / "c.txt").write_text("not markdown\n", encoding="utf-8")

    cli.cmd_read(_read_args(tmp_path, no_push=True))

    assert [c[0].name for c in read_calls["markdown"]] == ["a.md", "b.md"]


def test_read_writes_to_the_out_dir_when_given_one(tmp_path: Path, read_calls):
    md = tmp_path / "probe.md"
    md.write_text("# Test\n", encoding="utf-8")
    out = tmp_path / "pdfs"
    out.mkdir()

    cli.cmd_read(_read_args(md, no_push=True, out=str(out)))

    assert read_calls["markdown"][0][1] == out / "probe.pdf"


def test_read_compiles_a_tex_file_rather_than_typesetting_its_macros(tmp_path: Path, read_calls):
    """A .tex file goes to the LaTeX engine at the LaTeX body size, never to the markdown
    renderer — which would not fail, it would push a document of backslashes."""
    tex = tmp_path / "brief.tex"
    tex.write_text(r"\section{Brief}Body.", encoding="utf-8")

    cli.cmd_read(_read_args(tex, no_push=True))

    assert read_calls["markdown"] == []
    (src, out_pdf, geometry, engine) = read_calls["latex"][0]
    assert (src, out_pdf) == (tex, tmp_path / "brief.pdf")
    assert geometry.font_pt == 9.0        # the two-column LaTeX size, not [reading].font_pt
    assert engine == "tectonic"


def test_read_pushes_a_pdf_unchanged(tmp_path: Path, read_calls):
    """A PDF is already the artifact: it is pushed as-is, never re-rendered."""
    pdf = tmp_path / "paper.pdf"
    pdf.write_bytes(b"%PDF-1.4 stub")

    cli.cmd_read(_read_args(pdf))

    assert read_calls["sent_pdf"] == [(pdf, None, "Locus/Inbox")]
    assert read_calls["markdown"] == [] and read_calls["latex"] == []


def test_read_no_push_on_a_pdf_does_nothing_at_all(tmp_path: Path, read_calls, capsys):
    """`--no-push` means "render only", and for a PDF there is nothing to render. Checking the
    flag AFTER the send would have pushed the document the flag said not to push."""
    pdf = tmp_path / "paper.pdf"
    pdf.write_bytes(b"%PDF-1.4 stub")

    cli.cmd_read(_read_args(pdf, no_push=True))

    assert read_calls["sent_pdf"] == []
    assert "nothing to render" in capsys.readouterr().out


@requires_toolchain
def test_read_markdown_end_to_end_produces_a_real_pdf(tmp_path: Path, monkeypatch, capsys):
    """The stubbed tests above pin which function is called; this one pins that the call is
    ACTUALLY VALID. `render_markdown_to_pdf` has the same arity and would accept a Path as its
    markdown text, so only a real render distinguishes the two."""
    monkeypatch.setattr(cli, "load", lambda: _FakeCfg())
    md = tmp_path / "probe.md"
    md.write_text("# Test\n\nBody.\n", encoding="utf-8")

    cli.cmd_read(_read_args(md, no_push=True))

    out_pdf = tmp_path / "probe.pdf"
    assert out_pdf.read_bytes()[:5] == b"%PDF-"
    assert "rendered probe.md" in capsys.readouterr().out
