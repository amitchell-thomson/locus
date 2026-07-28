"""Loop C — conversation capture (agent-layer §8.3, Phase 1 ⑤). Pure file/parsing, no model/DB:
the note is written to notes/conversations/ with rough frontmatter, and a Claude Code .jsonl
transcript is parsed into a formatted note."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from locus.capture.conversations import (
    capture_conversation, conversations_dir, import_jsonl_transcript,
)


def test_capture_writes_rough_note_with_provenance(tmp_path: Path):
    cap = capture_conversation(
        "We decided to route transcription to Sonnet because Haiku garbled jargon.",
        title="Transcription model choice", project="agent-layer",
        notes_dir=tmp_path, now="2026-07-28T00:00:00+00:00",
    )
    assert cap.path.parent == conversations_dir(tmp_path)  # filed under conversations/
    text = cap.path.read_text()
    assert "category: note" in text and "maturity: rough" in text
    assert "source: conversation" in text
    assert "title: Transcription model choice" in text
    assert "project: agent-layer" in text
    assert "route transcription to Sonnet" in text


def test_slug_has_hash_and_is_idempotent(tmp_path: Path):
    a = capture_conversation("same body", title="Note", notes_dir=tmp_path)
    b = capture_conversation("same body", title="Note", notes_dir=tmp_path)
    c = capture_conversation("different body", title="Note", notes_dir=tmp_path)
    assert a.slug == b.slug           # identical content -> same file (idempotent)
    assert a.slug != c.slug           # different content -> distinct file (no clobber)
    assert list(conversations_dir(tmp_path).glob("*.md")).__len__() == 2


def _write_jsonl(path: Path, entries: list[dict]) -> Path:
    path.write_text("\n".join(json.dumps(e) for e in entries), encoding="utf-8")
    return path


def test_import_jsonl_transcript_parses_turns(tmp_path: Path):
    jsonl = _write_jsonl(tmp_path / "t.jsonl", [
        {"type": "user", "message": {"role": "user", "content": "How should I model regimes?"}},
        {"type": "assistant", "message": {"role": "assistant",
                                          "content": [{"type": "text", "text": "Use an HMM."}]}},
        {"type": "system", "message": {"role": "system", "content": "ignored meta"}},
        {"message": {"role": "assistant", "content": [
            {"type": "tool_use", "name": "x"},  # tool block -> no text -> skipped
        ]}},
    ])
    cap = import_jsonl_transcript(jsonl, notes_dir=tmp_path, project="regime-ml")
    text = cap.path.read_text()
    assert cap.title.startswith("How should I model regimes")  # title from first user line
    assert "**You:**" in text and "How should I model regimes?" in text
    assert "**Claude:**" in text and "Use an HMM." in text
    assert "ignored meta" not in text  # system/tool lines dropped


def test_import_empty_transcript_raises(tmp_path: Path):
    jsonl = _write_jsonl(tmp_path / "e.jsonl", [{"type": "system", "message": {"role": "system", "content": "x"}}])
    with pytest.raises(ValueError, match="no user/assistant turns"):
        import_jsonl_transcript(jsonl, notes_dir=tmp_path)
