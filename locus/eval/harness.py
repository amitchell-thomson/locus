"""Eval orchestration: sample sections, (optionally re-run a model), judge, aggregate.

Two modes:
  - score the outputs already in the DB (what qwen produced at ingest), or
  - regenerate a section's outputs with a named model and score those, to compare models
    head-to-head on a fixed sample (decision C: qwen2.5:7b vs llama3.1:8b).
"""

from __future__ import annotations

import random
from dataclasses import dataclass

from locus.eval.judge import SectionScores, judge_section
from locus.ingest import entities as entities_pass
from locus.ingest import propositions as propositions_pass
from locus.ingest import summarize as summarize_pass


@dataclass
class SectionEvalInput:
    section_id: int
    doc_id: int
    title: str | None
    source: str
    summary: str
    propositions: list[str]
    entities: list[tuple[str, str]]


def sample_section_ids(conn, n: int, seed: int = 0, doc_id: int | None = None) -> list[int]:
    """Deterministically sample up to n section ids (optionally within one doc)."""
    sql = "SELECT id FROM sections"
    params: tuple = ()
    if doc_id is not None:
        sql += " WHERE doc_id=?"
        params = (doc_id,)
    sql += " ORDER BY id"
    ids = [r["id"] for r in conn.execute(sql, params)]
    if n >= len(ids):
        return ids
    return sorted(random.Random(seed).sample(ids, n))


def _section_source(conn, section_id: int) -> str:
    rows = conn.execute(
        "SELECT raw_text FROM chunks WHERE section_id=? ORDER BY position", (section_id,)
    ).fetchall()
    return "\n".join(r["raw_text"] for r in rows)


def load_existing(conn, section_id: int) -> SectionEvalInput:
    """Build an eval input from the outputs already stored for this section."""
    s = conn.execute(
        "SELECT id, doc_id, title, summary FROM sections WHERE id=?", (section_id,)
    ).fetchone()
    props = [r["text"] for r in conn.execute(
        "SELECT text FROM propositions WHERE section_id=? ORDER BY position", (section_id,)
    )]
    ents = [(r["name"], r["type"]) for r in conn.execute(
        "SELECT name, type FROM entities WHERE section_id=?", (section_id,)
    )]
    return SectionEvalInput(
        section_id=section_id, doc_id=s["doc_id"], title=s["title"],
        source=_section_source(conn, section_id), summary=s["summary"] or "",
        propositions=props, entities=ents,
    )


def regenerate(conn, section_id: int, model: str) -> SectionEvalInput:
    """Re-run the section passes with `model` (does not touch the DB)."""
    s = conn.execute("SELECT id, doc_id, title FROM sections WHERE id=?", (section_id,)).fetchone()
    source = _section_source(conn, section_id)
    title = s["title"]
    summary = summarize_pass.summarize_section(title, source, model=model).summary
    props = propositions_pass.extract_propositions(title, source, model=model)
    ents = entities_pass.extract_entities(title, source, model=model)
    return SectionEvalInput(
        section_id=section_id, doc_id=s["doc_id"], title=title, source=source,
        summary=summary, propositions=props, entities=[(e.name, str(e.type)) for e in ents],
    )


@dataclass
class JudgedSection:
    section_id: int
    doc_id: int
    scores: SectionScores


def _aggregate(judged: list[JudgedSection]) -> dict[str, float]:
    if not judged:
        return {}
    fields = list(SectionScores.model_fields)
    fields.remove("notes")
    agg = {f: sum(getattr(j.scores, f) for j in judged) / len(judged) for f in fields}
    agg["overall_mean"] = sum(j.scores.mean() for j in judged) / len(judged)
    return agg


def evaluate(
    conn,
    *,
    sample: int = 8,
    seed: int = 0,
    doc_id: int | None = None,
    model: str | None = None,
    judge_model: str | None = None,
) -> tuple[list[JudgedSection], dict[str, float]]:
    """Judge a fixed sample. If `model` is given, regenerate with it first; else score the DB."""
    ids = sample_section_ids(conn, sample, seed, doc_id)
    judged: list[JudgedSection] = []
    for sid in ids:
        ev = regenerate(conn, sid, model) if model else load_existing(conn, sid)
        scores = judge_section(
            ev.source, ev.summary, ev.propositions, ev.entities, model=judge_model
        )
        judged.append(JudgedSection(section_id=sid, doc_id=ev.doc_id, scores=scores))
    return judged, _aggregate(judged)
