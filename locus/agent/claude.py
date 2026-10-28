"""The one `claude -p` task runner for the agent layer (agent-layer plan §10).

Every agent language task — transcription, fill-in, enrich phrasing, critique/synthesis, object
proposal — shells out through this single runner, exactly as `link/adjudicate.py` pioneered for
alias adjudication. This module generalises that pattern and adds the two hard-won Phase-0
requirements that adjudication predates:

  1. **Env scrubbing (non-negotiable).** `locus.config` injects the project `.env`'s
     ANTHROPIC_API_KEY into `os.environ` at import; a `claude -p` subprocess that inherits it
     prefers that METERED key over the subscription OAuth login — silently rerouting agent-layer
     spend to pay-as-you-go billing (or failing outright). Every call here runs with a cleaned
     env. (The Phase-0 spike failed on every call until this was fixed.)
  2. **Bounded retry-then-degrade.** `claude -p` errors are transient (an incidental connector
     warning exits nonzero, then the retry succeeds). `call()` retries a bounded number of times
     with backoff, then raises `ClaudeError` — the CALLER catches it to DEGRADE (keep the raw
     input, flag a gap), so a model failure never blocks capture or aborts a batch (ingest §7).

The runner is injectable (tests pass a fake `CliRunner`, no subprocess), and every call surfaces
the envelope's cost/usage so the budget ledger (`budget.py`) can meter spend from a direct read
(Phase-0 finding d) rather than scraping rate-limit errors.

Grounding is NOT done here: the orchestrator retrieves candidates in-process and passes them in
the prompt. This module only turns a (prompt, model) into validated output.

**Every model call in the codebase comes through here** (Slice 0.13, 2026-10-27). Transcription,
the ingest passes, `locus query`, retitle and the evals used the metered SDK until the API credit
ran out mid-capture on 27 Oct; Alec chose to move them rather than top it up. The reason they had
left was measured and is gone: Phase 0 sent a page to `claude -p` as a FILE, and the model spent
eight tool turns opening it ($0.23 a page). `ask` sends the image INSIDE the message
(`--input-format stream-json`), and it is read in one turn, on the subscription, in under 3 s.

**Every call runs lean**: `--tools '' --strict-mcp-config --max-turns 1`. A headless call
otherwise loads every tool and every user MCP server it can find and sends their definitions
with the prompt: measured 27 Oct, 23,581 tokens of definitions on a call that needed none,
against 741 lean. That was being paid on every fill-in, structure and adjudication call.

**Thinking is OFF unless a call asks for it.** Headless Claude Code thinks by default. Measured
27 Oct on an ingest pass (propositions from 9,000 characters of his notes, Haiku): 54.9 s and
4,834 thinking tokens by default, 19.5 s and none with `MAX_THINKING_TOKENS=0`, the same task, a
third of the output quota. Extraction and transcription gain nothing from it; `locus query`,
which asked the SDK for adaptive thinking, keeps it.
"""

from __future__ import annotations

import base64
import json
import os
import subprocess
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import TypeVar

from pydantic import BaseModel, ValidationError

from locus.config import load

# A wedged `claude -p` should never take this long; bounds a stuck subprocess.
_CLI_TIMEOUT_S = 600
# Env vars that reroute `claude -p` off the subscription onto metered billing — scrubbed (req 1).
_METERED_KEYS = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")

# C0 control characters that must not reach the prompt. The prompt is an ARGV ELEMENT, and argv
# cannot carry a NUL — `subprocess` raises `ValueError: embedded null byte` before the process is
# even spawned. That is not a `ClaudeError`, so it escapes every `except ClaudeError` degrade path
# in the codebase and surfaces as whatever generic handler happens to be above it.
# NOT HYPOTHETICAL. Maths PDFs extract symbol-font glyphs as control codes ("jx \x00 aj < \x11" is
# |x - a| < e), so three calculus documents carried NULs in their chunk text and failed the
# structure pass on EVERY run — 2026-08-06 was at least the second night, three documents each
# time, and each failure marked the whole run degraded and put an alarm on the daily page.
# Kept: \t, \n, \r, which are legitimate prompt content.
_CONTROL_CHARS = {c: None for c in range(0x20) if c not in (0x09, 0x0A, 0x0D)}
_CONTROL_CHARS[0x7F] = None

T = TypeVar("T", bound=BaseModel)


class ClaudeError(RuntimeError):
    """A `claude -p` call failed after exhausting retries. Callers catch this to degrade."""


@dataclass(frozen=True)
class ClaudeResult:
    """One `claude -p` reply plus the envelope's cost/usage (for the budget ledger, §10)."""

    text: str
    cost_usd: float = 0.0
    usage: dict = field(default_factory=dict)
    service_tier: str | None = None


# (prompt, model) -> ClaudeResult. The default spawns `claude -p`; tests inject a fake.
CliRunner = Callable[[str, "str | None"], ClaudeResult]
# (content blocks, model, system) -> ClaudeResult: the general form, which carries images.
ContentRunner = Callable[[list, "str | None", "str | None"], ClaudeResult]
# Optional per-result sink (the budget ledger records cost; the caller may log). Never raises.
ResultSink = Callable[[ClaudeResult], None]

# No tools, no MCP servers, one turn: see the module docstring for what this saves.
_LEAN = ("--max-turns", "1", "--strict-mcp-config", "--tools", "")
# Used when a caller gives no system prompt. Without one a call inherits Claude Code's own, which
# is written for an interactive coding agent and weighs ~3,500 tokens: a one-word probe measured
# 4,279 input tokens on 28 Oct. Every fill-in, structure and adjudication call was paying it.
_PLAIN_SYSTEM = (
    "You complete one task from the user's message and reply with only what it asks for. "
    "Follow its instructions and output format exactly."
)


def _scrubbed_env(*, thinking: bool = False) -> dict[str, str]:
    env = dict(os.environ)
    for key in _METERED_KEYS:
        env.pop(key, None)
    if not thinking:
        env["MAX_THINKING_TOKENS"] = "0"     # see the module docstring
    else:
        env.pop("MAX_THINKING_TOKENS", None)
    return env


def scrub_prompt(prompt: str) -> str:
    """Drop control characters a prompt can neither carry nor mean (see `_CONTROL_CHARS`)."""
    return (prompt or "").translate(_CONTROL_CHARS)


def image_block(data: bytes, media_type: str = "image/png") -> dict:
    """An image as a message content block, for `ask(images=...)` or a hand-built content list."""
    return {
        "type": "image",
        "source": {"type": "base64", "media_type": media_type,
                   "data": base64.standard_b64encode(data).decode("ascii")},
    }


def _scrub_blocks(content: list) -> list:
    out = []
    for block in content:
        if isinstance(block, dict) and block.get("type") == "text":
            block = {**block, "text": scrub_prompt(block.get("text", ""))}
            block.pop("cache_control", None)
        elif isinstance(block, dict):
            block = {k: v for k, v in block.items() if k != "cache_control"}
        out.append(block)
    return out


def _content_runner(
    content: list, model: str | None, system: str | None, *, timeout: float = _CLI_TIMEOUT_S,
    thinking: bool = False,
) -> ClaudeResult:
    """Run one headless `claude -p` over message content blocks (text and images).

    The message goes in on stdin as one stream-json user turn, so an image travels inside it and
    nothing has to be opened with a tool, and the prompt is no longer an argv element (a NUL
    could not go there at all; control characters are still scrubbed, they carry no meaning).
    The env is scrubbed of the metered key, the cwd is neutral, and the call is lean. Raises
    `ClaudeError` on any failure, including a reply the model marked as an error.
    """
    cmd = ["claude", "-p", "--input-format", "stream-json", "--output-format", "stream-json",
           "--verbose", *_LEAN]
    if model:
        cmd += ["--model", model]
    cmd += ["--system-prompt", scrub_prompt(system or _PLAIN_SYSTEM)]
    message = {"type": "user",
               "message": {"role": "user", "content": _scrub_blocks(content)}}
    try:
        proc = subprocess.run(
            cmd, input=json.dumps(message) + "\n", capture_output=True, text=True,
            timeout=timeout, cwd=tempfile.gettempdir(), env=_scrubbed_env(thinking=thinking),
        )
    except FileNotFoundError as e:
        raise ClaudeError("`claude` CLI not found on PATH — the agent layer uses headless Claude Code") from e
    except subprocess.TimeoutExpired as e:
        raise ClaudeError(f"`claude -p` timed out after {timeout}s") from e
    envelope = None
    for line in proc.stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("type") == "result":
            envelope = event
    if envelope is None:
        detail = (proc.stderr or proc.stdout)[:500]
        raise ClaudeError(f"`claude -p` returned no result (exit {proc.returncode}): {detail}")
    if envelope.get("is_error") or proc.returncode != 0:
        raise ClaudeError(f"`claude -p` reported an error: {envelope.get('result')!r}")
    return ClaudeResult(
        text=envelope.get("result", "") or "",
        cost_usd=float(envelope.get("total_cost_usd") or 0.0),
        usage=envelope.get("usage") or {},
        service_tier=envelope.get("service_tier"),
    )


def content_runner(
    content: list, model: str | None, system: str | None, *, timeout: float = _CLI_TIMEOUT_S,
    thinking: bool = False,
) -> ClaudeResult:
    """`_content_runner`, counted: every call, success or failure, is one `model_calls` row
    (agent/usage.py, Slice 0.7), so the subscription share is measured rather than guessed."""
    from locus.agent import usage

    images = sum(1 for b in content if isinstance(b, dict) and b.get("type") == "image")
    started = time.monotonic()
    try:
        result = _content_runner(content, model, system, timeout=timeout, thinking=thinking)
    except ClaudeError as exc:
        usage.record(usage.record_from_usage(
            model=model, ok=False, duration_ms=int((time.monotonic() - started) * 1000),
            usage=None, images=images, error=str(exc),
        ))
        raise
    usage.record(usage.record_from_usage(
        model=model, ok=True, duration_ms=int((time.monotonic() - started) * 1000),
        usage=result.usage, images=images, cost_usd=result.cost_usd,
    ))
    return result


def cli_runner(prompt: str, model: str | None) -> ClaudeResult:
    """Run one headless `claude -p` over a text prompt (the original runner's interface)."""
    return content_runner([{"type": "text", "text": prompt}], model, None)


def ask(
    content: "str | list",
    *,
    system: str | None = None,
    images: "list[bytes] | tuple[bytes, ...]" = (),
    model: str | None = None,
    runner: ContentRunner | None = None,
    retries: int = 2,
    backoff_s: float = 0.5,
    on_result: ResultSink | None = None,
    thinking: bool = False,
) -> ClaudeResult:
    """One call with a system prompt, text and images, with bounded retry-then-raise.

    `content` is a prompt string or ready-made content blocks (`locus query` builds its own, with
    figures). `images` are PNG bytes placed before the text, which is where a vision model reads
    them best. The model defaults to `[agent].model`.
    """
    if runner is None:
        def runner(content, model, system):
            return content_runner(content, model, system, thinking=thinking)
    model = model or load().agent.model
    blocks = (
        [image_block(i) for i in images] + [{"type": "text", "text": content}]
        if isinstance(content, str) else list(content)
    )
    last: ClaudeError | None = None
    for attempt in range(retries + 1):
        try:
            result = runner(blocks, model, system)
        except ClaudeError as e:
            last = e
            if attempt < retries and backoff_s:
                time.sleep(backoff_s * (attempt + 1))
            continue
        if on_result is not None:
            on_result(result)
        return result
    raise last or ClaudeError("`claude -p` call failed with no captured error")


def call(
    prompt: str,
    *,
    model: str | None = None,
    runner: CliRunner | None = None,
    retries: int = 2,
    backoff_s: float = 0.5,
    on_result: ResultSink | None = None,
) -> ClaudeResult:
    """One `claude -p` call with bounded retry-then-raise (req 2).

    Retries transient `ClaudeError`s up to `retries` times (linear backoff), then re-raises the
    last error for the caller to degrade on. `on_result` (e.g. the budget ledger) sees every
    successful result. `model` defaults to `[agent].model` (Haiku — the Phase-0 routing choice)."""
    runner = runner or cli_runner
    model = model or load().agent.model
    last: ClaudeError | None = None
    for attempt in range(retries + 1):
        try:
            result = runner(prompt, model)
        except ClaudeError as e:
            last = e
            if attempt < retries and backoff_s:
                time.sleep(backoff_s * (attempt + 1))
            continue
        if on_result is not None:
            on_result(result)
        return result
    raise last or ClaudeError("`claude -p` call failed with no captured error")


def run_text(
    prompt: str, *, model: str | None = None, runner: CliRunner | None = None,
    retries: int = 2, backoff_s: float = 0.5, on_result: ResultSink | None = None,
) -> str:
    """Run a language task whose output is free text (transcription, fill-in, phrasing)."""
    return call(
        prompt, model=model, runner=runner, retries=retries, backoff_s=backoff_s,
        on_result=on_result,
    ).text


def _extract_json(text: str) -> str:
    """Slice the first '{'..last '}' — tolerant of prose/code-fences around the JSON."""
    start, end = text.find("{"), text.rfind("}")
    if start == -1 or end <= start:
        raise ClaudeError(f"no JSON object in reply: {text[:200]!r}")
    return text[start : end + 1]


def run_structured(
    prompt: str,
    *,
    schema: type[T],
    model: str | None = None,
    runner: CliRunner | None = None,
    retries: int = 2,
    backoff_s: float = 0.5,
    on_result: ResultSink | None = None,
) -> T:
    """Run a language task that must return JSON validating against `schema`.

    Bounded repair: a reply that is unparseable or fails validation is retried (same `retries`
    budget as transient errors), because a re-ask usually fixes a one-off malformed reply. On
    exhaustion raises `ClaudeError` for the caller to degrade on. Mirrors `adjudicate._parse_verdict`
    but for any pydantic model."""
    runner = runner or cli_runner
    model = model or load().agent.model
    last: ClaudeError | None = None
    for attempt in range(retries + 1):
        try:
            result = runner(prompt, model)
            obj = json.loads(_extract_json(result.text))
            validated = schema.model_validate(obj)
        except (ClaudeError, json.JSONDecodeError, ValidationError) as e:
            last = e if isinstance(e, ClaudeError) else ClaudeError(f"invalid structured reply: {e}")
            if attempt < retries and backoff_s:
                time.sleep(backoff_s * (attempt + 1))
            continue
        if on_result is not None:
            on_result(result)
        return validated
    raise last or ClaudeError("`claude -p` structured call failed with no captured error")


# --- the Messages-API shape, served by the subscription -------------------------------------------

@dataclass
class _Block:
    type: str
    text: str = ""
    input: dict | None = None
    name: str = ""


@dataclass
class _Usage:
    input_tokens: int = 0
    output_tokens: int = 0


@dataclass
class _Response:
    content: list
    usage: _Usage
    cost_usd: float = 0.0


def _system_text(system) -> str | None:
    if system is None:
        return None
    if isinstance(system, str):
        return system
    return "\n\n".join(b.get("text", "") for b in system if isinstance(b, dict))


def _as_blocks(content) -> list:
    return [{"type": "text", "text": content}] if isinstance(content, str) else list(content)


def _fold_turns(messages: list) -> list:
    """A conversation as ONE user turn, since a headless call is one turn.

    The callers that send more than one message are repair loops ("your reply was invalid, try
    again"): the first user turn carries the images and the task, the rest is the exchange. They
    are kept in order and labelled, so the model sees what it said and what was wrong with it.
    """
    if len(messages) == 1:
        return _as_blocks(messages[0]["content"])
    blocks = _as_blocks(messages[0]["content"])
    for m in messages[1:]:
        label = "Your earlier reply" if m.get("role") == "assistant" else "Follow-up"
        text = " ".join(
            b.get("text", "") for b in _as_blocks(m["content"]) if b.get("type") == "text"
        )
        blocks.append({"type": "text", "text": f"[{label}]\n{text}"})
    return blocks


class SubscriptionClient:
    """`client.messages.create(...)` served by headless Claude Code on the subscription.

    For the call sites written against the Anthropic SDK (transcription, ingest passes, query,
    retitle, evals), so moving them off the metered key (Slice 0.13) changed one constructor each
    and none of their tests, which inject SDK-shaped fakes. Supports what those sites use: a
    system prompt, text and image blocks, a repair conversation (folded, see `_fold_turns`), and
    FORCED tool use, which is asked for as JSON against the tool's schema and returned as a
    `tool_use` block, and `thinking`, which turns it on for that call. Anything else they pass
    (`max_tokens`, `cache_control`) is the CLI's to decide and is ignored. Never reaches the metered key: `content_runner` scrubs it.
    """

    def __init__(self, runner: ContentRunner | None = None, *, timeout: float | None = None):
        self._runner = runner
        self._timeout = timeout
        self.messages = self

    def create(self, *, model=None, messages, system=None, tools=None, tool_choice=None,
               thinking=None, **_):
        blocks = _fold_turns(messages)
        forced = None
        if tool_choice and tool_choice.get("type") == "tool" and tools:
            forced = next((t for t in tools if t.get("name") == tool_choice.get("name")), None)
        if forced is not None:
            blocks.append({"type": "text", "text": (
                "Respond with ONLY a JSON object conforming to this JSON Schema "
                f"(no prose, no code fences):\n{json.dumps(forced.get('input_schema', {}))}"
            )})
        # A call that asked the SDK for thinking (`locus query`) keeps it; the rest run without.
        think = bool(thinking) and (thinking or {}).get("type") != "disabled"
        runner = self._runner
        if runner is None:
            timeout = self._timeout or _CLI_TIMEOUT_S

            def runner(content, model, system):
                return content_runner(content, model, system, timeout=timeout, thinking=think)
        result = ask(blocks, system=_system_text(system), model=model, runner=runner)
        usage = result.usage or {}
        tokens_in = sum(int(usage.get(k) or 0) for k in (
            "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
        out_usage = _Usage(input_tokens=tokens_in, output_tokens=int(usage.get("output_tokens") or 0))
        if forced is not None:
            try:
                obj = json.loads(_extract_json(result.text))
                return _Response([_Block("tool_use", input=obj, name=forced["name"])], out_usage,
                                 result.cost_usd)
            except (ClaudeError, json.JSONDecodeError):
                pass                    # the caller's "no tool_use block" path reports it
        return _Response([_Block("text", text=result.text)], out_usage, result.cost_usd)


def subscription_client(*, timeout: float | None = None) -> SubscriptionClient:
    return SubscriptionClient(timeout=timeout)
