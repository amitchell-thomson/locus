"""Assemble the evidence a critique/synthesis is allowed to use (plan §8.4).

Everything here is deterministic and free: local retrieval, plus joins over the agent-state
tables. No model call happens in this module — it produces the EVIDENCE SET, and the surfaces
that call it are then held to it (a claim citing something not in this set is dropped).

Keeping assembly separate is what makes the surfaces testable without a model at all: a test can
build an evidence set by hand and assert exactly what survives.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from locus.agent import state
from locus.agent.state import AgentObject, parse_entity_key
from locus.evolve.trajectory import Trajectory, build_trajectory
from locus.learn.gaps import Gap, gaps_for_concept_key, gaps_for_object


@dataclass
class Evidence:
    """One citable unit put in front of the model. `key` is what a reply must cite back."""

    key: str  # short stable handle, e.g. 'S1'
    text: str
    source: str  # document title / provenance line


@dataclass
class GroundingSet:
    topic: str
    evidence: list[Evidence] = field(default_factory=list)
    objects: list[AgentObject] = field(default_factory=list)
    trajectories: list[Trajectory] = field(default_factory=list)
    gaps: list[Gap] = field(default_factory=list)
    low_confidence: bool = False

    def by_key(self) -> dict[str, Evidence]:
        return {e.key: e for e in self.evidence}

    def render(self) -> str:
        """The evidence block for a prompt. Keys are what the model must cite."""
        lines = []
        if self.evidence:
            lines.append("CORPUS EVIDENCE (cite these keys):")
            lines += [f"[{e.key}] ({e.source}) {e.text}" for e in self.evidence]
        if self.objects:
            lines.append("\nYOUR STRUCTURED OBJECTS:")
            for o in self.objects:
                lines.append(f"- {o.type} '{o.title}' ({o.status})")
                for field_name in ("approach", "mastery", "state"):
                    if o.body.get(field_name):
                        lines.append(f"    {field_name}: {o.body[field_name]}")
                for thread in o.body.get("open_threads", []):
                    lines.append(f"    open thread: {thread}")
                for learning in o.body.get("learnings", []):
                    lines.append(f"    learning: {learning}")
        for traj in self.trajectories:
            if traj.entries:
                lines.append(f"\nYOUR POSITIONS ON {traj.label.upper()} (oldest first):")
                lines += [f"- {e.dated_at}: {e.stance}" for e in traj.entries]
        if self.gaps:
            lines.append("\nDETECTED GAPS:")
            lines += [f"- {g.render()}" for g in self.gaps]
        return "\n".join(lines) or "(no corpus material found)"


def _retrieval_evidence(result, *, floor: float | None, titles: dict[int, str]) -> list[Evidence]:
    out: list[Evidence] = []
    for i, c in enumerate(getattr(result, "citation_details", []) or [], start=1):
        if floor is not None and (c.rerank_score is None or c.rerank_score < floor):
            continue
        out.append(Evidence(key=f"S{i}", text=c.text, source=titles.get(c.doc_id, "corpus")))
    return out


def _titles(conn, doc_ids) -> dict[int, str]:
    ids = sorted({d for d in doc_ids if d is not None})
    if not ids:
        return {}
    placeholders = ",".join("?" * len(ids))
    return {
        r["id"]: r["title"]
        for r in conn.execute(f"SELECT id, title FROM documents WHERE id IN ({placeholders})", ids)
    }


def ground_topic(
    conn,
    topic: str,
    *,
    retrieve_fn=None,
    include_trajectories: bool = True,
    include_gaps: bool = True,
    max_evidence: int = 12,
) -> GroundingSet:
    """Everything the corpus knows about a topic: retrieved units + objects + trajectories + gaps.

    Evidence is floor-filtered (sub-floor citations are noise the surfaces must not reason from),
    but a low-confidence retrieval is REPORTED rather than emptied — "I have little on this" is
    a true and useful answer, and suppressing it would make the surface silently vague."""
    from locus.config import load

    if retrieve_fn is None:
        from locus.retrieve import retrieve as _retrieve

        retrieve_fn = lambda q: _retrieve(q, conn=conn)  # noqa: E731

    out = GroundingSet(topic=topic)
    result = retrieve_fn(topic)
    out.low_confidence = bool(getattr(result, "low_confidence", False))
    details = getattr(result, "citation_details", []) or []
    titles = _titles(conn, [c.doc_id for c in details])
    out.evidence = _retrieval_evidence(
        result, floor=load().retrieve.min_rerank_score, titles=titles
    )[:max_evidence]

    out.objects = _objects_matching(conn, topic)
    if include_trajectories:
        out.trajectories = _trajectories_for(conn, out.objects, topic)
    if include_gaps:
        out.gaps = _gaps_for(conn, out.objects, out.trajectories)
    return out


def _objects_matching(conn, topic: str, *, limit: int = 5) -> list[AgentObject]:
    """Objects whose title appears in the topic, or vice versa — a deliberately literal match.

    Fuzzy object matching would put the WRONG project's open threads in front of the model, and a
    confidently-answered question about the wrong project is worse than a miss."""
    rows = conn.execute(
        "SELECT * FROM objects WHERE status != 'archived' ORDER BY "
        "CASE status WHEN 'active' THEN 0 ELSE 1 END, updated_at DESC"
    ).fetchall()
    topic_cf = topic.casefold()
    out = []
    for r in rows:
        title = r["title"].casefold()
        if title and (title in topic_cf or topic_cf in title):
            out.append(state.get_object(conn, r["id"]))
        if len(out) >= limit:
            break
    return [o for o in out if o is not None]


def _trajectories_for(conn, objects: list[AgentObject], topic: str) -> list[Trajectory]:
    """Trajectories for the matched objects, plus any concept subject named in the topic."""
    out: list[Trajectory] = []
    seen: set[tuple[str, str]] = set()
    for obj in objects:
        if obj.type == "project":
            key = ("project", str(obj.id))
        else:
            entity = next((link for link in obj.links if link.target_kind == "entity"), None)
            if entity is None:
                continue
            key = ("concept", entity.target_key)
        if key not in seen:
            seen.add(key)
            out.append(build_trajectory(conn, *key))

    topic_cf = topic.casefold()
    for kind, key, _n in state.subjects_with_positions(conn, limit=200):
        if kind != "concept" or (kind, key) in seen:
            continue
        if parse_entity_key(key)[0].casefold() in topic_cf:
            seen.add((kind, key))
            out.append(build_trajectory(conn, kind, key))
    return [t for t in out if t.entries]


def _gaps_for(conn, objects: list[AgentObject], trajectories: list[Trajectory]) -> list[Gap]:
    out: list[Gap] = []
    for obj in objects:
        out += gaps_for_object(conn, obj.id, limit=5)
    for traj in trajectories:
        if traj.subject_kind == "concept":
            out += gaps_for_concept_key(conn, traj.subject_key, limit=2)
    # Dedupe on (kind, subject) — a concept reached through both a project and its own trajectory
    # is one gap, not two.
    seen: set[tuple[str, str]] = set()
    unique = []
    for gap in out:
        key = (gap.kind, gap.subject)
        if key not in seen:
            seen.add(key)
            unique.append(gap)
    return unique


def ground_object(conn, object_id: int, *, retrieve_fn=None, max_evidence: int = 12) -> GroundingSet:
    """Grounding centred on one object — its own docs, threads, trajectory and gaps."""
    obj = state.get_object(conn, object_id)
    if obj is None:
        return GroundingSet(topic=f"object {object_id}")
    out = ground_topic(
        conn, obj.title, retrieve_fn=retrieve_fn, max_evidence=max_evidence,
    )
    if all(o.id != obj.id for o in out.objects):
        out.objects.insert(0, obj)
        out.gaps = _gaps_for(conn, out.objects, out.trajectories)
    return out
