"""Conservative fill-in (agent-layer §8.1, Phase 1 ④). The claude -p runner is injected; fills are
applied deterministically in Python, so we assert non-gap text is preserved byte-for-byte."""

from __future__ import annotations

from locus.agent.claude import ClaudeResult
from locus.capture import fillin
from locus.capture.fillin import fill_gaps


def _runner(fills_json: str):
    return lambda prompt, model: ClaudeResult(text=fills_json)


def test_no_markers_is_unchanged():
    text = "A clean transcription with no gaps at all."
    r = fill_gaps(text)  # no runner needed — short-circuits
    assert r.markdown == text
    assert (r.filled, r.total_gaps) == (0, 0)


def test_high_confidence_fill_marked_and_null_left():
    text = "The swap [illegible] resets on SOFR[?] each period."
    runner = _runner('{"fills": [{"n": 1, "resolution": "leg"}, {"n": 2, "resolution": null}]}')
    r = fill_gaps(text, runner=runner)
    # gap 1 ([illegible]) filled + AI-marked; gap 2 (SOFR[?]) left untouched (null)
    assert r.markdown == "The swap ⟦leg⟧ resets on SOFR[?] each period."
    assert (r.filled, r.total_gaps) == (1, 2)


def test_non_gap_text_is_byte_preserved():
    text = "# Heading\n\nExact prose — keep me.  [illegible]  More exact prose.\n"
    runner = _runner('{"fills": [{"n": 1, "resolution": "word"}]}')
    r = fill_gaps(text, runner=runner)
    assert r.markdown == "# Heading\n\nExact prose — keep me.  ⟦word⟧  More exact prose.\n"


def test_degrades_to_raw_on_runner_failure(monkeypatch):
    def boom(*a, **k):
        raise fillin.claude.ClaudeError("subprocess failed")

    monkeypatch.setattr(fillin.claude, "run_structured", boom)
    text = "word [illegible] here"
    r = fill_gaps(text)
    assert r.markdown == text  # unchanged — degrade, never block
    assert (r.filled, r.total_gaps) == (0, 1)
