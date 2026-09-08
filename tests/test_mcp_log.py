"""The MCP call log and the boundary guard around every tool.

These test the thing that was missing on 2026-09-08: a bare "Tool execution failed" carried no
information, so three sessions concluded the server was down while it was measurably serving.
What is asserted here is not that tools work — it is that when one does not, the client is told
which tool and which exception, and the server keeps a record that outlives the client.
"""

from __future__ import annotations

import json

import pytest

from locus import mcp_server
from locus.observe import mcp_log


@pytest.fixture(autouse=True)
def log_to_tmp(tmp_path, monkeypatch):
    """Never touch the real vault/logs, and never read the gitignored live config."""
    monkeypatch.setattr(mcp_log, "_resolved", tmp_path / "mcp.jsonl")
    yield tmp_path / "mcp.jsonl"


def _events(path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines()]


def test_successful_call_is_logged_start_and_end(log_to_tmp):
    wrapped = mcp_server._instrument(lambda greeting="hi": f"{greeting} there")

    assert wrapped(greeting="hello") == "hello there"

    kinds = [(e["event"], e.get("tool"), e.get("ok")) for e in _events(log_to_tmp)]
    assert kinds == [("start", "<lambda>", None), ("end", "<lambda>", True)]


def test_exception_becomes_text_naming_the_class(log_to_tmp):
    def to_remarkable(title: str = "x"):
        raise TimeoutError("tectonic went to lunch")

    result = mcp_server._instrument(to_remarkable)(title="brief")

    # The whole point: the model on the far side gets a message, not an empty transport error,
    # and the message says the call ARRIVED so the diagnosis does not wander to the network.
    assert "to_remarkable raised TimeoutError" in result
    assert "tectonic went to lunch" in result
    assert "REACHED the server" in result


def test_exception_is_recorded_with_its_traceback(log_to_tmp):
    def boom():
        raise RuntimeError("compile failed on line 12")

    mcp_server._instrument(boom)()

    end = _events(log_to_tmp)[-1]
    assert end["ok"] is False
    assert end["error"] == "RuntimeError"
    assert "compile failed on line 12" in end["message"]
    assert "raise RuntimeError" in end["traceback"]


def test_long_arguments_are_recorded_as_a_size_not_a_body(log_to_tmp):
    mcp_server._instrument(lambda latex="": "sent")(latex="x" * 5000)

    start = _events(log_to_tmp)[0]
    assert start["args"]["latex"] == "<5000 chars>"


def test_a_call_that_never_finished_is_identifiable(log_to_tmp):
    """The client-gave-up fingerprint: an arrival with no completion beside it.

    This is the one failure a server-side error message can never reach him for, because by
    the time the tool would answer nobody is listening. It has to be readable afterwards.
    """
    mcp_log.record("start", call=1, tool="to_remarkable")
    mcp_log.record("start", call=2, tool="list_documents")
    mcp_log.record("end", call=2, tool="list_documents", ok=True, ms=3)

    pending = mcp_log.unanswered()
    assert [e["tool"] for e in pending] == ["to_remarkable"]


def test_logging_failure_never_breaks_the_tool(log_to_tmp, monkeypatch):
    """An observability path that can break the thing it observes is worse than none."""
    monkeypatch.setattr(mcp_log, "_resolved", log_to_tmp / "not-a-dir" / "x" / "\0bad")

    assert mcp_server._instrument(lambda: "delivered")() == "delivered"


def test_stale_server_warns_in_the_result(monkeypatch):
    """A long-lived server running old code says so where he can see it — the tool result."""
    monkeypatch.setattr(mcp_server, "_SERVER_BUILD", "aaaaaaa (2026-09-01)")
    monkeypatch.setattr(mcp_server, "_stale_cache", (0.0, ""))
    monkeypatch.setattr(mcp_server, "_build_stamp", lambda: "bbbbbbb (2026-09-08)")

    result = mcp_server._instrument(lambda: "12 documents")()

    assert "STALE" in result
    assert "aaaaaaa" in result and "bbbbbbb" in result
    assert result.endswith("12 documents")


def test_current_server_adds_no_banner(monkeypatch):
    monkeypatch.setattr(mcp_server, "_SERVER_BUILD", "bbbbbbb (2026-09-08)")
    monkeypatch.setattr(mcp_server, "_stale_cache", (0.0, ""))
    monkeypatch.setattr(mcp_server, "_build_stamp", lambda: "bbbbbbb (2026-09-08)")

    assert mcp_server._instrument(lambda: "12 documents")() == "12 documents"


def test_unknown_build_never_cries_wolf(monkeypatch):
    """A tarball deploy with no git cannot compare builds; silence beats a false alarm."""
    monkeypatch.setattr(mcp_server, "_SERVER_BUILD", "unstarted")
    monkeypatch.setattr(mcp_server, "_stale_cache", (0.0, ""))

    assert mcp_server._stale_note() == ""


def test_wrapper_preserves_the_advertised_signature():
    """FastMCP builds the tool schema from the function; the guard must be invisible to it."""
    import inspect

    def to_remarkable(latex: str | None = None, title: str | None = None) -> str:
        """Send a document to the tablet."""
        return "sent"

    wrapped = mcp_server._instrument(to_remarkable)
    assert inspect.signature(wrapped) == inspect.signature(to_remarkable)
    assert wrapped.__doc__ == to_remarkable.__doc__
    assert wrapped.__name__ == "to_remarkable"


def test_a_dirty_working_tree_alone_is_not_stale(monkeypatch):
    """The banner is a commit-level alarm. Editing the checkout must not set it off all day."""
    monkeypatch.setattr(mcp_server, "_SERVER_BUILD", "983d2af (2026-09-08)")
    monkeypatch.setattr(mcp_server, "_stale_cache", (0.0, ""))
    monkeypatch.setattr(mcp_server, "_build_stamp", lambda: "983d2af+dirty (2026-09-08)")

    assert mcp_server._stale_note() == ""


def test_the_note_is_cached_between_calls(monkeypatch):
    """One `git` subprocess per 30s, not one per tool call."""
    calls = []
    monkeypatch.setattr(mcp_server, "_SERVER_BUILD", "aaaaaaa (2026-09-01)")
    monkeypatch.setattr(mcp_server, "_stale_cache", (0.0, ""))
    monkeypatch.setattr(mcp_server, "_build_stamp",
                        lambda: (calls.append(1), "bbbbbbb (2026-09-08)")[1])

    first = mcp_server._stale_note()
    second = mcp_server._stale_note()

    assert first == second and "STALE" in first
    assert len(calls) == 1
