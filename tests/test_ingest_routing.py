"""Per-pass ingest LLM routing (agent-layer §7, Phase 1). Model-free: the SDK client is faked and
the config is monkeypatched, so no Ollama and no API. Verifies a pass routes to Claude when
configured, resolves aliases, and falls back to local by default."""

from __future__ import annotations

import types

import pytest

from locus.config import load
from locus.ingest import llm
from locus.ingest.llm import IngestExtractionError, generate_structured, route_for
from locus.ingest.summarize import SectionSummary


def test_route_for_resolves_aliases_and_defaults_local(monkeypatch):
    cfg = load()
    monkeypatch.setattr(cfg.ingest, "pass_routing",
                        {"summarize": "haiku", "propositions": "sonnet", "gaps": "local"})
    assert route_for("summarize") == "claude-haiku-4-5-20251001"
    assert route_for("propositions") == "claude-sonnet-5"
    assert route_for("gaps") == "local"
    assert route_for("entities") == "local"  # not in the map -> local
    assert route_for(None) == "local"         # no pass name -> local


class _FakeSDK:
    """Records the model and returns scripted replies (one per call)."""

    def __init__(self, *replies: str):
        self.replies = list(replies)
        self.model = None
        self.calls = 0
        self.messages = self

    def create(self, **kw):
        self.model = kw["model"]
        self.calls += 1
        text = self.replies[min(self.calls - 1, len(self.replies) - 1)]
        return types.SimpleNamespace(content=[types.SimpleNamespace(type="text", text=text)])


def test_generate_structured_routes_to_claude_sdk(monkeypatch):
    monkeypatch.setattr(llm, "route_for",
                        lambda p: "claude-haiku-4-5-20251001" if p == "summarize" else "local")
    fake = _FakeSDK('Here you go: {"summary": "a faithful summary", "title": "Kalman"}')
    out = generate_structured(SectionSummary, "text", pass_name="summarize", sdk_client=fake)
    assert out.summary == "a faithful summary" and out.title == "Kalman"
    assert fake.model == "claude-haiku-4-5-20251001"  # routed to the resolved Claude model
    assert fake.calls == 1


def test_claude_path_repairs_then_succeeds(monkeypatch):
    monkeypatch.setattr(llm, "route_for", lambda p: "claude-haiku-4-5-20251001")
    fake = _FakeSDK("not json at all", '{"summary": "ok", "title": null}')
    out = generate_structured(SectionSummary, "t", pass_name="summarize", sdk_client=fake)
    assert out.summary == "ok" and fake.calls == 2  # retried on the invalid first reply


def test_claude_path_raises_after_exhausting_retries(monkeypatch):
    monkeypatch.setattr(llm, "route_for", lambda p: "claude-haiku-4-5-20251001")
    fake = _FakeSDK("nope", "still nope", "nope again")
    with pytest.raises(IngestExtractionError):
        generate_structured(SectionSummary, "t", pass_name="summarize", sdk_client=fake, retries=2)


# ---------- the routed SDK client: one per process, and it must fail fast ----------


def test_the_sdk_client_is_built_once_not_per_call(monkeypatch):
    """Live failure 2026-07-31: a client per call leaked 324 fds (218 API sockets) climbing
    ~15/min on a 1024 limit, and the 211-page book ingest wedged."""
    from locus.ingest import llm

    monkeypatch.setattr(llm, "_SDK_CLIENT", None)
    built = []

    class _Fake:
        def __init__(self, **kw):
            built.append(kw)

    monkeypatch.setattr("anthropic.Anthropic", _Fake)
    monkeypatch.setattr("locus.config.Config.anthropic_api_key", staticmethod(lambda: "k"))

    a, b = llm._client(), llm._client()
    assert a is b, "the client must be reused across passes"
    assert len(built) == 1


def test_the_client_fails_fast_rather_than_blocking_for_ten_minutes(monkeypatch):
    """The SDK default is 600s x 2 retries = up to 30 min of silent blocking on one hung call."""
    from locus.ingest import llm

    monkeypatch.setattr(llm, "_SDK_CLIENT", None)
    built = {}

    class _Fake:
        def __init__(self, **kw):
            built.update(kw)

    monkeypatch.setattr("anthropic.Anthropic", _Fake)
    monkeypatch.setattr("locus.config.Config.anthropic_api_key", staticmethod(lambda: "k"))

    llm._client()
    assert built["timeout"] <= 120, "an ingest pass returns <=4096 tokens; minutes is too long"
    assert built["max_retries"] >= 1
