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
        lambda q, conn=None: RetrievalResult(query=q, context="CTX-BLOCK", citations=["DocA, §S1, pp 1-2"]),
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
        lambda q, conn=None: RetrievalResult(query=q, context="", citations=[]),
    )
    client = _FakeClient()
    res = answer("anything", client=client, model="m")
    assert res.answer == "the grounded answer"
    assert "no relevant material" in client.messages.kwargs["messages"][0]["content"]
