"""Every model call on the subscription (Slice 0.13): the lean stream-json runner and the
Messages-API adapter the former SDK call sites use. No subprocess is spawned: `subprocess.run`
is replaced, and what the runner WOULD send is asserted."""

from __future__ import annotations

import json
import subprocess

import pytest

from locus.agent import claude


class _Proc:
    def __init__(self, stdout, returncode=0, stderr=""):
        self.stdout, self.returncode, self.stderr = stdout, returncode, stderr


def _result_line(text="ok", is_error=False, cost=0.0042):
    return json.dumps({"type": "result", "result": text, "is_error": is_error,
                       "total_cost_usd": cost,
                       "usage": {"input_tokens": 3, "cache_creation_input_tokens": 700,
                                 "output_tokens": 9}})


@pytest.fixture()
def spawned(monkeypatch):
    calls: list = []

    def fake_run(cmd, **kw):
        calls.append((cmd, kw))
        return _Proc(json.dumps({"type": "system", "subtype": "init"}) + "\n"
                     + calls[-1][1].get("_reply", _result_line()) + "\n")

    monkeypatch.setattr(subprocess, "run", fake_run)
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-metered")
    return calls


def test_every_call_is_lean_and_off_the_metered_key(spawned):
    """No tools and no MCP servers: otherwise ~23,500 tokens of definitions ride on each call."""
    r = claude.content_runner([{"type": "text", "text": "hi"}], "haiku", "be terse")
    cmd, kw = spawned[0]
    assert cmd[:2] == ["claude", "-p"]
    for flag in ("--strict-mcp-config", "--max-turns"):
        assert flag in cmd
    assert cmd[cmd.index("--tools") + 1] == ""
    assert cmd[cmd.index("--input-format") + 1] == "stream-json"
    assert cmd[cmd.index("--system-prompt") + 1] == "be terse"
    assert "ANTHROPIC_API_KEY" not in kw["env"]
    assert r.text == "ok" and r.cost_usd == pytest.approx(0.0042)


def test_an_image_travels_inside_the_message_not_as_a_file(spawned):
    """Phase 0 handed `claude -p` a file and paid for eight tool turns to open it."""
    claude.ask("Transcribe this.", images=[b"\x89PNG fake"], model="sonnet",
               runner=claude.content_runner)
    sent = json.loads(spawned[0][1]["input"])
    blocks = sent["message"]["content"]
    assert [b["type"] for b in blocks] == ["image", "text"]          # image first
    assert blocks[0]["source"]["media_type"] == "image/png"
    assert sent["type"] == "user"


def test_control_characters_never_reach_the_model(spawned):
    claude.content_runner([{"type": "text", "text": "jx \x00 aj < \x11"}], None, None)
    text = json.loads(spawned[0][1]["input"])["message"]["content"][0]["text"]
    assert "\x00" not in text and "\x11" not in text


def test_a_reply_marked_as_an_error_raises(monkeypatch):
    monkeypatch.setattr(subprocess, "run",
                        lambda cmd, **kw: _Proc(_result_line("credit", is_error=True)))
    with pytest.raises(claude.ClaudeError):
        claude.content_runner([{"type": "text", "text": "x"}], None, None)
    monkeypatch.setattr(subprocess, "run", lambda cmd, **kw: _Proc("not json", 1, "boom"))
    with pytest.raises(claude.ClaudeError, match="no result"):
        claude.content_runner([{"type": "text", "text": "x"}], None, None)


def _recording_runner(reply="ok"):
    seen: list = []

    def run(blocks, model, system):
        seen.append((blocks, model, system))
        return claude.ClaudeResult(text=reply, cost_usd=0.01,
                                   usage={"input_tokens": 2, "cache_read_input_tokens": 5,
                                          "output_tokens": 4})
    return run, seen


def test_the_adapter_serves_the_messages_shape():
    run, seen = _recording_runner("transcribed text")
    client = claude.SubscriptionClient(runner=run)
    resp = client.messages.create(
        model="claude-sonnet-5", max_tokens=4096, thinking={"type": "adaptive"},
        system=[{"type": "text", "text": "You transcribe.", "cache_control": {"type": "ephemeral"}}],
        messages=[{"role": "user", "content": [claude.image_block(b"png"),
                                               {"type": "text", "text": "Go."}]}],
    )
    assert [b.type for b in resp.content] == ["text"]
    assert resp.content[0].text == "transcribed text"
    assert resp.usage.input_tokens == 7 and resp.usage.output_tokens == 4
    blocks, model, system = seen[0]
    assert model == "claude-sonnet-5" and system == "You transcribe."
    assert blocks[0]["type"] == "image"


def test_forced_tool_use_comes_back_as_a_tool_use_block():
    """retitle and the judge force a tool for structured output; on the CLI that is a JSON
    reply against the tool's schema, returned in the shape they already parse."""
    run, seen = _recording_runner('Here: {"keep_existing": false, "topic": "Swaps"}')
    tool = {"name": "title", "input_schema": {"type": "object"}}
    resp = claude.SubscriptionClient(runner=run).messages.create(
        model="m", messages=[{"role": "user", "content": "Title this."}],
        tools=[tool], tool_choice={"type": "tool", "name": "title"},
    )
    assert resp.content[0].type == "tool_use"
    assert resp.content[0].input == {"keep_existing": False, "topic": "Swaps"}
    assert "JSON Schema" in seen[0][0][-1]["text"]


def test_a_repair_conversation_is_folded_into_one_turn():
    run, seen = _recording_runner()
    claude.SubscriptionClient(runner=run).messages.create(
        model="m", messages=[
            {"role": "user", "content": [claude.image_block(b"png"), {"type": "text", "text": "Judge."}]},
            {"role": "assistant", "content": "not json"},
            {"role": "user", "content": "That was invalid. JSON only."},
        ],
    )
    blocks = seen[0][0]
    assert blocks[0]["type"] == "image"
    assert blocks[-2]["text"].startswith("[Your earlier reply]")
    assert blocks[-1]["text"].startswith("[Follow-up]")


def test_no_metered_client_is_left_in_the_codebase():
    """The point of 0.13: nothing constructs a metered client any more."""
    from pathlib import Path

    root = Path(claude.__file__).resolve().parents[1]
    offenders = [
        str(p.relative_to(root)) for p in root.rglob("*.py")
        if "anthropic.Anthropic(" in p.read_text(encoding="utf-8")
    ]
    assert offenders == []


def test_thinking_is_off_unless_asked_for(spawned):
    """Default headless thinking tripled an ingest pass's time and output quota (55 s vs 19.5 s)."""
    claude.content_runner([{"type": "text", "text": "x"}], None, None)
    assert spawned[-1][1]["env"]["MAX_THINKING_TOKENS"] == "0"
    claude.content_runner([{"type": "text", "text": "x"}], None, None, thinking=True)
    assert "MAX_THINKING_TOKENS" not in spawned[-1][1]["env"]


def test_query_keeps_its_thinking_and_extraction_does_not(spawned):
    client = claude.SubscriptionClient()
    client.messages.create(model="m", messages=[{"role": "user", "content": "q"}],
                           thinking={"type": "adaptive"})
    assert "MAX_THINKING_TOKENS" not in spawned[-1][1]["env"]
    client.messages.create(model="m", messages=[{"role": "user", "content": "extract"}])
    assert spawned[-1][1]["env"]["MAX_THINKING_TOKENS"] == "0"


def test_a_call_without_a_system_prompt_does_not_inherit_claude_codes(spawned):
    """Claude Code's own system prompt is ~3,500 tokens, written for an interactive agent."""
    claude.cli_runner("x", None)
    cmd = spawned[-1][0]
    assert cmd[cmd.index("--system-prompt") + 1] == claude._PLAIN_SYSTEM
