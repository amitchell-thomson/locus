"""Owned-block writer (agent-layer plan §10, Phase 1 ③) — the correctness linchpin.

Pure file manipulation, no model/DB. The properties that make agent writes safe: content outside
the marked block is never touched, regeneration replaces (never accumulates), writes are atomic
(no temp litter), and a conflict diverts to a sidecar instead of clobbering the owner's prose.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.vault import markers, sidecar
from locus.vault.writer import (
    WriteResult, content_hash, upsert_block, write_generated_note,
)


# ---------- markers ----------

def test_validate_kind_rejects_unsafe():
    markers.validate_kind("related")
    markers.validate_kind("tension-2")
    for bad in ("Related", "a b", "x:y", "", "-x"):
        with pytest.raises(ValueError):
            markers.validate_kind(bad)


def test_render_block_strips_body_and_wraps_markers():
    b = markers.render_block("related", "\n\n> body\n\n", "run7")
    assert b == "<!-- locus:ai:related:start run=run7 -->\n> body\n<!-- locus:ai:related:end -->"


def test_upsert_appends_then_replaces_without_accumulating():
    text = "# Note\n\nProse.\n"
    once = markers.upsert(text, "related", "A", "1")
    assert once.startswith("# Note\n\nProse.\n")  # prose preserved byte-for-byte
    assert once.count("locus:ai:related:start") == 1
    twice = markers.upsert(once, "related", "B", "2")  # new run, new body
    assert twice.count("locus:ai:related:start") == 1  # replaced, not appended
    assert "B" in twice and "run=2" in twice and "run=1" not in twice
    assert twice.startswith("# Note\n\nProse.\n")


def test_invalid_run_id_rejected():
    with pytest.raises(ValueError):
        markers.render_block("related", "b", "bad>id")


# ---------- writer: create / append / replace ----------

def test_upsert_creates_note_when_absent(tmp_path: Path):
    note = tmp_path / "n.md"
    r = upsert_block(note, "related", "> [!ai] Related\n> - x", run_id="7")
    assert r == WriteResult(path=note, to_sidecar=False, created=True)
    assert "locus:ai:related:start" in note.read_text()


def test_upsert_preserves_prose_and_replaces_block(tmp_path: Path):
    note = tmp_path / "n.md"
    note.write_text("# Title\n\nOwner's words.\n", encoding="utf-8")
    upsert_block(note, "related", "first", run_id="1")
    upsert_block(note, "related", "second", run_id="2")
    out = note.read_text()
    assert out.startswith("# Title\n\nOwner's words.\n")  # untouched
    assert out.count("locus:ai:related:start") == 1       # no accumulation
    assert "second" in out and "first" not in out


def test_two_kinds_coexist(tmp_path: Path):
    note = tmp_path / "n.md"
    note.write_text("body\n", encoding="utf-8")
    upsert_block(note, "related", "R", run_id="1")
    upsert_block(note, "tension", "T", run_id="1")
    out = note.read_text()
    assert "locus:ai:related:start" in out and "locus:ai:tension:start" in out


def test_atomic_write_leaves_no_temp_files(tmp_path: Path):
    note = tmp_path / "n.md"
    upsert_block(note, "related", "x", run_id="1")
    assert [p.name for p in tmp_path.iterdir()] == ["n.md"]  # no .locus-*.tmp litter


# ---------- writer: conflict -> sidecar ----------

def test_expected_hash_mismatch_diverts_to_sidecar(tmp_path: Path):
    note = tmp_path / "n.md"
    note.write_text("original\n", encoding="utf-8")
    stale = content_hash("original\n")
    note.write_text("owner edited this\n", encoding="utf-8")  # changed since we read it
    r = upsert_block(note, "related", "enrichment", run_id="9", expected_hash=stale)
    assert r.to_sidecar is True
    assert r.path == sidecar.sidecar_path(note)
    assert note.read_text() == "owner edited this\n"          # prose NOT clobbered
    assert "enrichment" in r.path.read_text()


def test_sync_conflict_sibling_diverts_to_sidecar(tmp_path: Path):
    note = tmp_path / "n.md"
    note.write_text("body\n", encoding="utf-8")
    (tmp_path / "n.sync-conflict-20260101-120000-ABCDEF.md").write_text("dup\n", encoding="utf-8")
    r = upsert_block(note, "related", "x", run_id="1")
    assert r.to_sidecar is True
    assert "locus:ai:related" not in note.read_text()  # note left alone


def test_matching_expected_hash_writes_in_place(tmp_path: Path):
    note = tmp_path / "n.md"
    note.write_text("stable\n", encoding="utf-8")
    r = upsert_block(note, "related", "x", run_id="1", expected_hash=content_hash("stable\n"))
    assert r.to_sidecar is False


# ---------- writer: generated notes ----------

def test_generated_note_carries_provenance_frontmatter(tmp_path: Path):
    gen = tmp_path / "_generated" / "surfaced.md"
    r = write_generated_note(
        gen, "Body here.", run_id="42", generated_at=lambda: "2026-07-28T00:00:00+00:00",
        extra={"title": "Surfaced connections"},
    )
    assert r.created is True
    text = gen.read_text()
    assert "author: agent" in text
    assert "generated: true" in text
    assert "source_run: 42" in text
    assert "generated_at: 2026-07-28T00:00:00+00:00" in text
    assert "title: Surfaced connections" in text
    assert text.rstrip().endswith("Body here.")
