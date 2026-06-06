"""Stage 7: query — prompt assembly, mode selection, response parsing (no live Claude/Ollama)."""

from types import SimpleNamespace

import pytest

from locus import query as query_mod
from locus.query import QUERY_MODES, QueryResult, _system_prompt, answer
from locus.retrieve import RetrievalResult


class _FakeMessages:
    def __init__(self):
        self.kwargs = None

    def create(self, **kwargs):
        self.kwargs = kwargs
        return SimpleNamespace(
            content=[
                SimpleNamespace(type="thinking", thinking=""),  # ignored
                SimpleNamespace(type="text", text="the grounded answer"),
            ]
        )


class _FakeClient:
    def __init__(self):
        self.messages = _FakeMessages()


def test_system_prompt_carries_mode_and_grounding():
    sp = _system_prompt("gap")
    assert "Locus" in sp
    assert QUERY_MODES["gap"] in sp
    assert "ONLY the retrieved context" in sp


def test_unknown_mode_raises_before_any_call():
    with pytest.raises(ValueError):
        answer("q", mode="bogus")


def test_answer_assembles_prompt_and_parses_text(monkeypatch):
    monkeypatch.setattr(
        query_mod,
        "retrieve",
        lambda q, conn=None, facets=None: RetrievalResult(query=q, context="CTX-BLOCK", citations=["DocA, §S1, pp 1-2"]),
    )
    client = _FakeClient()
    res = answer("What determines stability?", mode="standard", client=client, model="claude-test")

    assert isinstance(res, QueryResult)
    assert res.answer == "the grounded answer"  # only text blocks, thinking dropped
    assert res.citations == ["DocA, §S1, pp 1-2"]
    assert res.model == "claude-test"

    kw = client.messages.kwargs
    assert kw["model"] == "claude-test"
    assert kw["system"][0]["cache_control"] == {"type": "ephemeral"}
    assert kw["thinking"] == {"type": "adaptive"}
    user = kw["messages"][0]["content"]
    assert "What determines stability?" in user
    assert "CTX-BLOCK" in user


def test_empty_context_still_answers(monkeypatch):
    monkeypatch.setattr(
        query_mod, "retrieve",
        lambda q, conn=None, facets=None: RetrievalResult(query=q, context="", citations=[]),
    )
    client = _FakeClient()
    res = answer("anything", client=client, model="m")
    assert res.answer == "the grounded answer"
    assert "no relevant material" in client.messages.kwargs["messages"][0]["content"]


def test_figures_attach_as_image_blocks(monkeypatch, tmp_path):
    """Retrieved figures become base64 image blocks in the user turn (tier 3)."""
    import pymupdf

    from locus.config import load as cfg_load
    from locus.retrieve.pipeline import RetrievedFigure

    # a real small PNG in an isolated raw store
    monkeypatch.setattr(cfg_load().paths, "raw_store", tmp_path)
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 200, 100))
    pix.clear_with(80)
    (tmp_path / "h_fig0.png").write_bytes(pix.tobytes("png"))

    figs = [
        RetrievedFigure(raw_path="h_fig0.png", page=3, kind="vector",
                        caption="Figure 1", doc_title="Doc T", rerank_score=5.0),
        RetrievedFigure(raw_path="missing.png", page=9, kind="raster",
                        caption=None, doc_title="Doc T", rerank_score=4.0),  # degrades
    ]
    monkeypatch.setattr(
        query_mod, "retrieve",
        lambda q, conn=None, facets=None: RetrievalResult(
            query=q, context="CTX", citations=[], figures=figs,
        ),
    )
    client = _FakeClient()
    res = answer("show the diagram", client=client, model="m")

    assert res.figures_attached == 1  # missing file silently degraded
    content = client.messages.kwargs["messages"][0]["content"]
    assert isinstance(content, list)
    types = [b["type"] for b in content]
    assert types == ["text", "text", "image"]  # prompt, label, image
    assert 'figure on p.3 of "Doc T"' in content[1]["text"]
    img = content[2]
    assert img["source"]["type"] == "base64" and img["source"]["media_type"] == "image/png"
    import base64

    assert base64.standard_b64decode(img["source"]["data"])[:8] == b"\x89PNG\r\n\x1a\n"


def test_no_figures_keeps_plain_string_content(monkeypatch):
    monkeypatch.setattr(
        query_mod, "retrieve",
        lambda q, conn=None, facets=None: RetrievalResult(query=q, context="CTX", citations=[]),
    )
    client = _FakeClient()
    res = answer("q", client=client, model="m")
    assert res.figures_attached == 0
    assert isinstance(client.messages.kwargs["messages"][0]["content"], str)


def test_figure_image_downscaled_to_max_edge(tmp_path, monkeypatch):
    import pymupdf
    from io import BytesIO

    from PIL import Image

    from locus.config import load as cfg_load
    from locus.retrieve.figure_images import MAX_EDGE_PX, load_figure_png

    monkeypatch.setattr(cfg_load().paths, "raw_store", tmp_path)
    pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 2400, 1200))  # over the edge cap
    pix.clear_with(60)
    (tmp_path / "big.png").write_bytes(pix.tobytes("png"))

    out = load_figure_png("big.png")
    assert out is not None
    with Image.open(BytesIO(out)) as im:
        assert max(im.size) == MAX_EDGE_PX
    assert load_figure_png("nope.png") is None
