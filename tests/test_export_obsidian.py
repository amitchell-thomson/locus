"""Obsidian projection (§13) — model-free: pure rendering + a seeded-DB export.

Asserts the load-bearing invariants from docs/obsidian-projection-plan.md: stable/collision-safe
slugs, canonical-only entity notes, deterministic (byte-identical) re-export, stale-note pruning,
and — the safety property — that the exporter never touches `.obsidian/` or anything outside its
owned subtrees.
"""

from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.export.obsidian import (
    WikiLink,
    doc_note_markdown,
    entity_note_markdown,
    export_vault,
    slug,
)


# --------------------------------------------------------------------------- pure rendering


def test_slug_basic_and_stability():
    assert slug("Kalman Filtering for Tracking!") == "kalman-filtering-for-tracking"
    assert slug("  --weird__name-- ") == "weird-name"
    assert slug("") == "untitled"
    # The disambiguator is an immutable id, so the slug is stable across re-exports.
    assert slug("Same Title", disambiguator=12) == "same-title-12"
    assert slug("Same Title", disambiguator=12) == slug("Same Title", disambiguator=12)


def test_doc_note_markdown_frontmatter_and_sections():
    doc = {
        "title": "State: Estimation",  # colon must survive in quoted frontmatter
        "category": "paper",
        "source_type": "pdf",
        "source_date": "2024-01-02",
        "source_uri": "arxiv:1234",
        "thesis": 'A "quoted" thesis',
        "method": "m",
        "result": "r",
        "limitations": None,  # None fields are omitted
    }
    sections = [{"position": 1, "title": "Intro", "summary": "the intro summary"}]
    related = [WikiLink("docs/note/other-2", "Other Doc")]
    entities = [WikiLink("entities/concept/kalman-filter", "Kalman filter")]
    md = doc_note_markdown(doc, sections, related, entities)

    assert md.startswith("---\n")
    assert 'title: "State: Estimation"' in md
    assert 'thesis: "A \\"quoted\\" thesis"' in md
    assert "limitations:" not in md  # None omitted
    assert "## 1. Intro" in md and "the intro summary" in md
    assert "[[docs/note/other-2|Other Doc]]" in md
    assert "[[entities/concept/kalman-filter|Kalman filter]]" in md


def test_entity_note_uses_canonical_not_raw():
    md = entity_note_markdown(
        "Kalman filter",
        "concept",
        variants=["Kalman filter", "kalman filter", "Kalman Filter"],
        mentioning_docs=[WikiLink("docs/paper/a-1", "Doc A")],
    )
    assert 'canonical_name: "Kalman filter"' in md
    assert "## Variant surfaces" in md
    # The canonical surface itself is not re-listed as a variant.
    assert "- kalman filter" in md and "- Kalman filter\n" not in md
    assert "[[docs/paper/a-1|Doc A]]" in md


# --------------------------------------------------------------------------- seeded export


def _seed(db: Path) -> None:
    migrate(db)
    conn = get_connection(db)
    with conn:
        docs = [
            ("h1", "pdf", "arxiv:1", "Kalman Filtering for Tracking", "paper"),
            ("h2", "markdown", "note:1", "State Estimation Notes", "note"),
            ("h3", "pdf", "course:thermo", "Thermodynamics Basics", "coursework"),
        ]
        for h, st, uri, title, cat in docs:
            conn.execute(
                "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
                "ingested_at, ingest_model, category, source_date, thesis) "
                "VALUES (?,?,?,?,?,?,?,?,?,?)",
                (h, st, uri, f"{h}.raw", title, "2026-01-01 00:00:00", "test", cat, "2024-01-01", f"thesis of {h}"),
            )
        # one section each
        for doc_id in (1, 2, 3):
            conn.execute(
                "INSERT INTO sections (doc_id, position, title, summary) VALUES (?,?,?,?)",
                (doc_id, 1, f"Section of doc {doc_id}", f"summary {doc_id}"),
            )
        # entities: docs 1 & 2 share the canonical "Kalman filter"; doc 1 also has a singleton
        # "EKF"; doc 3 stands alone — so exactly one canonical spans >= 2 docs.
        ents = [
            (1, 1, "Kalman filter", "concept"),
            (1, 1, "EKF", "method"),
            (2, 2, "kalman filter", "concept"),
            (3, 3, "Thermodynamics", "concept"),
        ]
        for doc_id, sec_id, name, typ in ents:
            conn.execute(
                "INSERT INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
                (doc_id, sec_id, name, typ),
            )
        # alias substrate (total mapping; singletons map to self).
        aliases = [
            ("Kalman filter", "concept", "Kalman filter", "concept", 1, "identity"),
            ("kalman filter", "concept", "Kalman filter", "concept", 1, "casefold"),
            ("EKF", "method", "EKF", "method", 2, "identity"),
            ("Thermodynamics", "concept", "Thermodynamics", "concept", 3, "identity"),
        ]
        for vn, vt, cn, ct, cid, tier in aliases:
            conn.execute(
                "INSERT INTO entity_aliases (variant_name, variant_type, canonical_name, "
                "canonical_type, cluster_id, tier) VALUES (?,?,?,?,?,?)",
                (vn, vt, cn, ct, cid, tier),
            )
    conn.close()


def _export(db: Path, out_dir: Path, **kw):
    conn = get_connection(db)
    try:
        return export_vault(conn, out_dir, db_path=db, **kw)
    finally:
        conn.close()


def test_export_creates_expected_tree(tmp_path):
    db = tmp_path / "locus.db"
    _seed(db)
    out = tmp_path / "obsidian"
    report = _export(db, out)

    assert report.doc_notes == 3
    assert report.entity_notes == 1  # only "Kalman filter" spans >= 2 docs
    assert report.aliases_built is True
    assert (out / "docs" / "paper" / "kalman-filtering-for-tracking.md").exists()
    assert (out / "docs" / "note" / "state-estimation-notes.md").exists()
    assert (out / "entities" / "concept" / "kalman-filter.md").exists()
    # singleton entities are NOT emitted as notes
    assert not (out / "entities" / "method" / "ekf.md").exists()
    assert not (out / "entities" / "concept" / "thermodynamics.md").exists()
    assert (out / "_index.md").exists()

    # doc1 <-> doc2 are related (shared canonical); doc1 links the entity note.
    doc1 = (out / "docs" / "paper" / "kalman-filtering-for-tracking.md").read_text()
    assert "## Related documents" in doc1
    assert "[[docs/note/state-estimation-notes|State Estimation Notes]]" in doc1
    assert "[[entities/concept/kalman-filter|Kalman filter]]" in doc1
    assert report.related_edges >= 2


def test_reexport_is_byte_identical(tmp_path):
    db = tmp_path / "locus.db"
    _seed(db)
    out = tmp_path / "obsidian"
    _export(db, out)
    before = {p: p.read_bytes() for p in sorted(out.rglob("*.md"))}
    _export(db, out)
    after = {p: p.read_bytes() for p in sorted(out.rglob("*.md"))}
    assert before == after


def test_prune_removes_deleted_doc_but_spares_obsidian_dir(tmp_path):
    db = tmp_path / "locus.db"
    _seed(db)
    out = tmp_path / "obsidian"
    _export(db, out)

    # The user's Obsidian config + a stray root file must survive a re-export untouched.
    (out / ".obsidian").mkdir()
    sentinel = out / ".obsidian" / "workspace.json"
    sentinel.write_text("user layout")
    root_note = out / "_index.md"
    assert root_note.exists()

    thermo = out / "docs" / "coursework" / "thermodynamics-basics.md"
    assert thermo.exists()

    # Delete doc 3 and re-export.
    conn = get_connection(db)
    with conn:
        conn.execute("DELETE FROM documents WHERE id=3")
    conn.close()
    report = _export(db, out)

    assert report.doc_notes == 2
    assert not thermo.exists()          # stale note pruned
    assert sentinel.read_text() == "user layout"  # .obsidian/ untouched
    assert report.pruned == 1


def test_docs_only_graph_skips_entity_notes(tmp_path):
    db = tmp_path / "locus.db"
    _seed(db)
    out = tmp_path / "obsidian"
    report = _export(db, out, emit_entity_notes=False)
    assert report.entity_notes == 0
    assert not (out / "entities").exists()
    # related edges (doc<->doc) still present — they don't depend on entity notes
    assert report.related_edges >= 2


def test_guard_refuses_out_dir_containing_db(tmp_path):
    out = tmp_path / "obsidian"
    db_inside = out / "sub" / "locus.db"
    db_inside.parent.mkdir(parents=True)
    db_inside.write_text("")  # not a real DB; the guard fires before any query
    conn = get_connection(tmp_path / "real.db")
    try:
        with pytest.raises(ValueError, match="parent of the DB"):
            export_vault(conn, out, db_path=db_inside)
    finally:
        conn.close()
