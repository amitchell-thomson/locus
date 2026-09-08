"""Every MCP tool call, recorded on the SERVER, where it survives the client giving up.

WHY THIS EXISTS
---------------
An MCP client reports a failed tool call as a bare string with no message. Three completely
different situations produce that same string, and from the client side they are
indistinguishable:

1. the tool raised, and the exception never made it into a response;
2. the server process is gone, or was never reached;
3. the server is fine and still working, and the CLIENT ran out of patience first.

On 2026-09-08 that ambiguity cost most of a day. `to_remarkable` was reported as broken from
three separate sessions while the delivery path was measurably working (CLI 2.9s, MCP tool
2.1s, PDF on the device), and the conclusion drawn from the bare error was "Locus is down" —
so the debugging went to Tailscale and the server, neither of which was broken. Nothing
anywhere recorded whether the call had even ARRIVED, so there was no way to tell case 3 from
case 2 except by argument.

This is that record. One JSON line per event, written the moment a call arrives and again when
it finishes, carrying the build the process started with and its pid. It answers the only
question that matters when the client says "Tool execution failed": did the call reach this
process, and what happened to it? A call logged `start` with no matching `end` is case 3 (or a
hang). No `start` at all is case 2. A `start` and an `end` carrying an error is case 1.

This is CLAUDE.md §3 in its usual shape — a path that looks wired and isn't, failing silently,
with tests passing either side. `observe/gates.py` does the same job for thresholds: record
what was rejected, because the rejection is invisible otherwise.

DERIVED AND REGENERABLE. Delete the file and nothing is lost but history. It is written
best-effort and never raises: an observability path that can break the thing it observes is
worse than no observability at all.
"""

from __future__ import annotations

import json
import os
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# Rotate at this size so an always-on log cannot fill the disk. One generation is kept: the
# question this log answers is always about the last few minutes, never about last month.
_MAX_BYTES = 8 * 1024 * 1024

# Long argument values (a LaTeX document, a captured conversation) are recorded as a size, not
# a body. The log is for "what was called and what happened", not for keeping a second copy of
# his content — and a multi-KB line per call would make the tail unreadable.
_MAX_ARG_CHARS = 120


# Resolved once per process. `record` runs on the hot path of every tool call, and re-reading
# and re-validating config.toml for each one would make the observability cost the thing it
# observes. It also keeps the tests off the live, gitignored config.toml (CLAUDE.md §13) —
# they call `use_path` instead.
_resolved: Path | None = None


def log_path(cfg: Any = None) -> Path:
    """Where the call log lives: `vault/logs/mcp.jsonl`, beside the DB it serves."""
    global _resolved
    if cfg is not None:
        return cfg.paths.db.parent / "logs" / "mcp.jsonl"
    if _resolved is None:
        from locus.config import load

        _resolved = load().paths.db.parent / "logs" / "mcp.jsonl"
    return _resolved


def use_path(path: Path | str) -> None:
    """Point the log at `path` for the rest of the process (tests; explicit overrides)."""
    global _resolved
    _resolved = Path(path)


def arg_digest(kwargs: dict[str, Any]) -> dict[str, Any]:
    """Summarise call arguments: short values verbatim, long ones as a character count."""
    out: dict[str, Any] = {}
    for key, value in kwargs.items():
        if value is None:
            continue
        if isinstance(value, str) and len(value) > _MAX_ARG_CHARS:
            out[key] = f"<{len(value)} chars>"
        else:
            out[key] = value
    return out


def record(event: str, **fields: Any) -> None:
    """Append one event. Best effort: any failure here is swallowed, deliberately.

    Called on the hot path of every tool, including before the tool runs, so it must be cheap
    and it must never raise. A logging error that broke a working tool call would be the exact
    inversion of the point.
    """
    try:
        path = log_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            if path.stat().st_size > _MAX_BYTES:
                path.replace(path.with_suffix(".jsonl.1"))
        except FileNotFoundError:
            pass
        line = json.dumps(
            {
                "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                "event": event,
                "pid": os.getpid(),
                **fields,
            },
            default=str,
        )
        with path.open("a", encoding="utf-8") as fh:
            fh.write(line + "\n")
    except Exception:  # noqa: BLE001 — see docstring: observability must not break the tool
        pass


def read_recent(limit: int = 40, *, errors_only: bool = False, cfg: Any = None) -> list[dict]:
    """The most recent events, oldest first. Unparseable lines are skipped, not raised on."""
    path = log_path(cfg)
    if not path.exists():
        return []
    events: list[dict] = []
    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if errors_only and event.get("ok", True):
            continue
        events.append(event)
    return events[-limit:]


def unanswered(cfg: Any = None) -> list[dict]:
    """Calls that arrived and never finished — the shape a client-side timeout leaves behind.

    A `start` with no matching `end` for the same (pid, call) is the fingerprint of case 3 in
    the module docstring: the server took the call and was still working when the client
    stopped listening. That is the one case a server-side error message cannot reach him, so it
    has to be readable here.
    """
    started: dict[tuple[int, int], dict] = {}
    for event in read_recent(limit=10_000, cfg=cfg):
        key = (event.get("pid", 0), event.get("call", 0))
        if event.get("event") == "start":
            started[key] = event
        elif event.get("event") == "end":
            started.pop(key, None)
    return list(started.values())


def now() -> float:
    """Monotonic clock for call durations, in one place so the unit is never in doubt."""
    return time.monotonic()


def live_servers(cfg: Any = None) -> list[dict]:
    """The `locus mcp` processes that started and are still alive, newest last.

    "Which server am I actually talking to, and is it running current code" is the first
    question in every one of these investigations, and it was previously unanswerable: the
    build stamp goes to stderr, which belongs to the client and is gone with it. Cross the
    logged `server_start` events against what is still running and the answer is a table.

    Liveness is checked with signal 0, which tests existence without touching the process. A
    recycled pid could in principle report a dead server as live; the start time in the row is
    what disambiguates that, and it is cheap enough to be worth the imprecision.
    """
    seen: dict[int, dict] = {}
    for event in read_recent(limit=10_000, cfg=cfg):
        if event.get("event") == "server_start":
            seen[event.get("pid", 0)] = event
        elif event.get("event") == "server_stop":
            seen.pop(event.get("pid", 0), None)
    live = []
    for pid, event in seen.items():
        try:
            os.kill(pid, 0)
        except (OSError, TypeError):
            continue
        live.append(event)
    return sorted(live, key=lambda e: e.get("at", ""))
