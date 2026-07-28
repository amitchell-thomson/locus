"""Loop A transcription (agent-layer §8.1, Phase 1 ④): handwriting PDF -> Markdown via a Claude
vision call. The Anthropic client is faked, so this is offline; rasterisation is real (pymupdf)."""

from __future__ import annotations

import types
from pathlib import Path

import pymupdf

from locus.capture.transcribe import PageTranscript, Transcript, render_pdf_pages, transcribe_pdf


def _make_pdf(path: Path, pages: int) -> Path:
    doc = pymupdf.open()
    for i in range(pages):
        page = doc.new_page()
        page.insert_text((72, 72), f"page {i + 1} content", fontsize=12)
    doc.save(str(path))
    doc.close()
    return path


class _FakeResp:
    def __init__(self, text, in_tok, out_tok):
        self.content = [types.SimpleNamespace(type="text", text=text)]
        self.usage = types.SimpleNamespace(input_tokens=in_tok, output_tokens=out_tok)


class _FakeClient:
    """Returns a scripted reply per call; records the image blocks it was sent."""

    def __init__(self, replies):
        self.replies = replies
        self.calls = 0
        self.sent_images = 0
        self.messages = self

    def create(self, **kw):
        for block in kw["messages"][0]["content"]:
            if block.get("type") == "image":
                self.sent_images += 1
        text, in_tok, out_tok = self.replies[self.calls]
        self.calls += 1
        return _FakeResp(text, in_tok, out_tok)


def test_render_pdf_pages_returns_png_per_page(tmp_path: Path):
    pdf = _make_pdf(tmp_path / "n.pdf", pages=3)
    pngs = render_pdf_pages(pdf, dpi=100)
    assert len(pngs) == 3
    assert all(p[:8] == b"\x89PNG\r\n\x1a\n" for p in pngs)  # PNG magic


def test_transcribe_pdf_pages_illegible_and_usage(tmp_path: Path):
    pdf = _make_pdf(tmp_path / "n.pdf", pages=2)
    client = _FakeClient([
        ("# Rates\n- swap curve\n- [illegible] basis", 1000, 40),
        ("More notes with [illegible] and [illegible].", 900, 30),
    ])
    t = transcribe_pdf(pdf, client=client, model="fake", dpi=100)

    assert client.sent_images == 2                     # one image per page
    assert [p.page for p in t.pages] == [1, 2]
    assert t.pages[0].illegible == 1
    assert t.illegible_total == 3
    assert (t.input_tokens, t.output_tokens) == (1900, 70)
    # cost estimate: 1900/1e6*1.0 + 70/1e6*5.0
    assert t.cost_usd == (1900 / 1_000_000 * 1.0 + 70 / 1_000_000 * 5.0)


def test_uncertainty_and_blank_markers_counted(tmp_path: Path):
    pdf = _make_pdf(tmp_path / "n.pdf", pages=2)
    client = _FakeClient([
        ("swap curve [?] and basis [?] risk [illegible]", 100, 10),
        ("[blank page]", 100, 5),
    ])
    t = transcribe_pdf(pdf, client=client, model="fake", dpi=100)
    assert t.pages[0].uncertain == 2 and t.pages[0].illegible == 1
    assert t.uncertain_total == 2
    assert t.pages[1].blank is True and t.pages[0].blank is False


def test_transcript_markdown_has_page_markers(tmp_path: Path):
    pdf = _make_pdf(tmp_path / "n.pdf", pages=2)
    client = _FakeClient([("first page text", 10, 5), ("second page text", 10, 5)])
    t = transcribe_pdf(pdf, client=client, model="fake", dpi=100)
    md = t.markdown
    assert "<!-- page 1 -->" in md and "<!-- page 2 -->" in md
    assert "first page text" in md and "second page text" in md


def test_empty_transcript_costs_zero():
    assert Transcript().cost_usd == 0.0
    assert Transcript().illegible_total == 0
