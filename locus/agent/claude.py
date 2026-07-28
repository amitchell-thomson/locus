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

Bulk work (a full re-ingest) belongs on the API Batch endpoint, not this per-call CLI path — that
runner is added with the hybrid-routing swap (plan §7), which the Phase-1 loops do not need.
"""

from __future__ import annotations

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
_CLI_TIMEOUT_S = 180
# Env vars that reroute `claude -p` off the subscription onto metered billing — scrubbed (req 1).
_METERED_KEYS = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")

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
# Optional per-result sink (the budget ledger records cost; the caller may log). Never raises.
ResultSink = Callable[[ClaudeResult], None]


def _scrubbed_env() -> dict[str, str]:
    env = dict(os.environ)
    for key in _METERED_KEYS:
        env.pop(key, None)
    return env


def cli_runner(prompt: str, model: str | None) -> ClaudeResult:
    """Run one headless `claude -p` with a scrubbed env and return the parsed envelope.

    `--output-format json` wraps the reply in a stable envelope; a neutral cwd keeps the call
    from picking up this repo's CLAUDE.md / project MCP servers. Raises `ClaudeError` on any
    failure (missing CLI, timeout, nonzero exit, non-JSON output, model-reported error)."""
    cmd = ["claude", "-p", prompt, "--output-format", "json"]
    if model:
        cmd += ["--model", model]
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=_CLI_TIMEOUT_S,
            cwd=tempfile.gettempdir(), env=_scrubbed_env(),
        )
    except FileNotFoundError as e:
        raise ClaudeError("`claude` CLI not found on PATH — the agent layer uses headless Claude Code") from e
    except subprocess.TimeoutExpired as e:
        raise ClaudeError(f"`claude -p` timed out after {_CLI_TIMEOUT_S}s") from e
    if proc.returncode != 0:
        raise ClaudeError(f"`claude -p` failed (exit {proc.returncode}): {proc.stderr[:500]}")
    try:
        envelope = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise ClaudeError(f"`claude -p` did not return JSON: {proc.stdout[:300]!r}") from e
    if envelope.get("is_error"):
        raise ClaudeError(f"`claude -p` reported an error: {envelope.get('result')!r}")
    return ClaudeResult(
        text=envelope.get("result", ""),
        cost_usd=float(envelope.get("total_cost_usd") or 0.0),
        usage=envelope.get("usage") or {},
        service_tier=envelope.get("service_tier"),
    )


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
