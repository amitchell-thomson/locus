"""Model calls on the Max API credit first, the subscription when it refuses (3 Nov).

No network: the API client is a fake and `subprocess.run` (the headless route) is faked; what is
asserted is which route each call took, that a refusal falls back and pauses the API, and that
every attempt is counted with its route."""

from __future__ import annotations

import json
import subprocess
from types import SimpleNamespace

import anthropic
import httpx
import pytest

from locus import config
from locus.agent import claude, usage


@pytest.fixture()
def api_route(monkeypatch, tmp_path):
    cfg = config.load()
    monkeypatch.setattr(cfg.agent, "backend", "api", raising=False)
    monkeypatch.setattr(claude, "_state_file", lambda: tmp_path / "state" / "api-credit.json")
    rows: list = []
    monkeypatch.setattr(usage, "SINK", rows.append)
    return rows


def _resp(text="api says hi"):
    return SimpleNamespace(
        content=[SimpleNamespace(type="text", text=text)],
        usage=SimpleNamespace(input_tokens=1000, output_tokens=100,
                              cache_creation_input_tokens=0, cache_read_input_tokens=0),
    )


class _Client:
    def __init__(self, behaviour):
        self.behaviour = behaviour
        self.calls: list = []
        self.messages = self

    def create(self, **kw):
        self.calls.append(kw)
        return self.behaviour(kw)


def _bad_request(msg):
    req = httpx.Request("POST", "https://api.anthropic.com/v1/messages")
    return anthropic.BadRequestError(msg, response=httpx.Response(400, request=req), body=None)


def _headless(monkeypatch):
    spawned: list = []
    line = json.dumps({"type": "result", "result": "cli says hi", "is_error": False,
                       "total_cost_usd": 0.0, "usage": {"input_tokens": 3, "output_tokens": 2}})

    def run(cmd, **kw):
        spawned.append(cmd)
        return SimpleNamespace(stdout=line + "\n", returncode=0, stderr="")

    monkeypatch.setattr(subprocess, "run", run)
    return spawned


def test_calls_go_to_the_api_credit_first(monkeypatch, api_route):
    client = _Client(lambda kw: _resp())
    monkeypatch.setattr(claude, "_API_CLIENT", client)
    spawned = _headless(monkeypatch)
    r = claude.content_runner([{"type": "text", "text": "hello"}], "haiku", "be brief")
    assert r.text == "api says hi" and spawned == []
    call = client.calls[0]
    assert call["model"] == "claude-haiku-4-5-20251001"       # the alias pinned to an API id
    assert call["system"] == "be brief" and "thinking" not in call
    (rec,) = api_route
    assert rec.backend == "api" and rec.ok
    assert rec.cost_usd == pytest.approx((1000 * 1.0 + 100 * 5.0) / 1e6)


def test_a_spent_credit_falls_back_to_the_subscription_and_pauses_the_api(monkeypatch, api_route,
                                                                         tmp_path):
    def refuse(kw):
        raise _bad_request("Your credit balance is too low to access the Anthropic API.")

    client = _Client(refuse)
    monkeypatch.setattr(claude, "_API_CLIENT", client)
    spawned = _headless(monkeypatch)
    r = claude.content_runner([{"type": "text", "text": "hello"}], "haiku", None)
    assert r.text == "cli says hi" and len(spawned) == 1
    assert [(x.backend, x.ok) for x in api_route] == [("api", False), ("subscription", True)]
    assert (tmp_path / "state" / "api-credit.json").exists()
    # the next call does not knock on the API again until the pause is over
    claude.content_runner([{"type": "text", "text": "again"}], "haiku", None)
    assert len(client.calls) == 1 and len(spawned) == 2


def test_the_api_is_tried_again_after_the_pause(monkeypatch, api_route, tmp_path):
    state = tmp_path / "state" / "api-credit.json"
    state.parent.mkdir(parents=True)
    state.write_text(json.dumps({"refused_at": "2026-10-01T00:00:00+00:00", "reason": "x"}))
    client = _Client(lambda kw: _resp())
    monkeypatch.setattr(claude, "_API_CLIENT", client)
    _headless(monkeypatch)
    assert claude.content_runner([{"type": "text", "text": "x"}], None, None).text == "api says hi"


def test_an_ordinary_api_error_is_an_error_not_a_fallback(monkeypatch, api_route):
    """Only a refusal for credit (or a dead key) switches routes; a malformed request is a bug
    to see, and silently re-running it on the subscription would hide it."""
    client = _Client(lambda kw: (_ for _ in ()).throw(_bad_request("max_tokens: too large")))
    monkeypatch.setattr(claude, "_API_CLIENT", client)
    spawned = _headless(monkeypatch)
    with pytest.raises(claude.ClaudeError):
        claude.content_runner([{"type": "text", "text": "x"}], None, None)
    assert spawned == []


def test_query_thinking_and_images_reach_the_api(monkeypatch, api_route):
    client = _Client(lambda kw: _resp())
    monkeypatch.setattr(claude, "_API_CLIENT", client)
    claude.SubscriptionClient().messages.create(
        model="sonnet", max_tokens=16000, thinking={"type": "adaptive"},
        messages=[{"role": "user", "content": [claude.image_block(b"png"),
                                               {"type": "text", "text": "q"}]}],
    )
    call = client.calls[0]
    assert call["thinking"] == {"type": "adaptive"} and call["max_tokens"] == 16000
    assert call["model"] == "claude-sonnet-5"
    assert call["messages"][0]["content"][0]["type"] == "image"
    assert api_route[0].images == 1


def test_backend_subscription_never_touches_the_api(monkeypatch, api_route):
    monkeypatch.setattr(config.load().agent, "backend", "subscription", raising=False)
    monkeypatch.setattr(claude, "_API_CLIENT", _Client(lambda kw: pytest.fail("API used")))
    spawned = _headless(monkeypatch)
    claude.content_runner([{"type": "text", "text": "x"}], None, None)
    assert len(spawned) == 1


def test_the_cycle_starts_on_its_day():
    from datetime import datetime, timezone

    now = datetime(2026, 11, 3, 12, tzinfo=timezone.utc)
    assert usage.cycle_start(1, now) == datetime(2026, 11, 1, tzinfo=timezone.utc)
    assert usage.cycle_start(15, now) == datetime(2026, 10, 15, tzinfo=timezone.utc)
