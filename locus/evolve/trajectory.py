"""Understanding-evolution: the dated position chain + tension detection (plan §3.4, §6.3).

This is the headline capability — "first I thought X, then after reading Z I revised to Y, now I
argue W" — and the reason `belief_positions.dated_at` is the SOURCE note's date rather than the
extraction time: the trajectory has to read as when the owner actually held each view, however
long capture took to catch up.

Two halves:

  `render_trajectory` — a deterministic join. Positions oldest-first, each with the document it
  came from. No model, so it cannot invent a stance the owner never took; if there are no stored
  positions there is no trajectory, and it says so rather than composing one.

  `find_tensions` — the advisory half. Semantic similarity ALONE cannot do this: a stance and its
  own contradiction sit close together in embedding space precisely because they are about the
  same thing, so a cosine threshold surfaces agreements and disagreements alike. So the shape is
  retrieve-then-judge, the same split the alias layer uses: embeddings (local, free) pick the near
  neighbours, and ONE `claude -p` call decides which of those pairs are genuinely in tension. The
  model is asked to prefer "no tension" — a false tension is worse than a missed one, because the
  callout's whole value is that it is rare enough to be worth reading.

Everything here is ADVISORY (invariant 2): a tension is a `> [!ai] Tension` callout for the owner
to judge, never an edit to a position, never an auto-archived belief.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field

from pydantic import BaseModel, Field

from locus.agent import state
from locus.agent.claude import ClaudeError, run_structured
from locus.agent.state import parse_entity_key
from locus.config import load

log = logging.getLogger(__name__)

# How many near-neighbour claims to put in front of the judge. Small on purpose: the call is per
# stance, and a long candidate list invites the model to find something rather than nothing.
_MAX_NEIGHBOURS = 8
# Stored propositions this far (cosine distance) from the stance are not about the same thing;
# nomic distances on genuinely related claims sit well inside this.
_MAX_DISTANCE = 0.55


@dataclass
class TrajectoryEntry:
    dated_at: str
    stance: str
    source_doc_id: int | None
    source_title: str | None = None


@dataclass
class Tension:
    """One advisory conflict between a stance and an earlier claim of the owner's."""

    stance: str
    conflicts_with: str
    reason: str
    source: str = ""  # where the conflicting claim came from (document title, or a dated position)


@dataclass
class Trajectory:
    subject_kind: str
    subject_key: str
    label: str
    entries: list[TrajectoryEntry] = field(default_factory=list)
    tensions: list[Tension] = field(default_factory=list)

    def __bool__(self) -> bool:
        return bool(self.entries)


# --- labels ------------------------------------------------------------------------------------


def subject_label(conn, subject_kind: str, subject_key: str) -> str:
    """Human-readable name for a trajectory subject (a concept's canonical name, a project's
    object title). Falls back to the raw key so an orphaned subject still renders."""
    if subject_kind == "concept":
        return parse_entity_key(subject_key)[0]
    row = conn.execute("SELECT title FROM objects WHERE id=?", (subject_key,)).fetchone()
    return row["title"] if row else subject_key


# --- the chain (deterministic) -------------------------------------------------------------------


def build_trajectory(
    conn, subject_kind: str, subject_key: str, *, with_tensions: bool = False, runner=None
) -> Trajectory:
    """The owner's dated positions on one subject, oldest first (§6.3).

    `with_tensions=True` additionally runs the judged tension pass over the LATEST stance — the
    one a new capture just added is the one worth warning about."""
    positions = state.positions_for(conn, subject_kind, subject_key)
    traj = Trajectory(
        subject_kind=subject_kind, subject_key=subject_key,
        label=subject_label(conn, subject_kind, subject_key),
    )
    titles = _doc_titles(conn, [p.source_doc_id for p in positions if p.source_doc_id])
    traj.entries = [
        TrajectoryEntry(
            dated_at=p.dated_at, stance=p.stance, source_doc_id=p.source_doc_id,
            source_title=titles.get(p.source_doc_id),
        )
        for p in positions
    ]
    if with_tensions and positions:
        traj.tensions = find_tensions(
            conn, positions[-1].stance, subject_kind=subject_kind, subject_key=subject_key,
            runner=runner,
        )
    return traj


def _doc_titles(conn, doc_ids: list[int]) -> dict[int, str]:
    ids = sorted({d for d in doc_ids if d is not None})
    if not ids:
        return {}
    placeholders = ",".join("?" * len(ids))
    return {
        r["id"]: r["title"]
        for r in conn.execute(f"SELECT id, title FROM documents WHERE id IN ({placeholders})", ids)
    }


# --- tension detection (retrieve deterministically, judge once) -----------------------------------


class _TensionVerdict(BaseModel):
    conflicts_with: str = ""
    reason: str = ""


class _TensionVerdicts(BaseModel):
    tensions: list[_TensionVerdict] = Field(default_factory=list)


@dataclass
class _Neighbour:
    text: str
    source: str


def _neighbour_positions(conn, stance: str, subject_kind: str, subject_key: str) -> list[_Neighbour]:
    """The subject's other stored stances — always relevant, no embedding needed."""
    out = []
    for p in state.positions_for(conn, subject_kind, subject_key):
        if p.stance.strip() == stance.strip():
            continue
        out.append(_Neighbour(text=p.stance, source=f"your position of {p.dated_at}"))
    return out


def _neighbour_propositions(conn, stance: str, *, embed_fn=None) -> list[_Neighbour]:
    """Stored propositions nearest the stance (dense KNN over proposition_vectors).

    Reuses the propositions layer as the plan specifies — the corpus's atomic claims are the
    substrate an inconsistency shows up against. Degrades to [] if embedding is unavailable:
    tension detection is advisory, and no callout is the right failure."""
    if embed_fn is None:
        from locus.ingest.embed import embed_text

        embed_fn = embed_text
    # The whole lookup degrades as one: embedding (Ollama may be down) and the KNN (the vector
    # table may be empty or dimension-mismatched) are both non-fatal here — this pass is advisory,
    # and offering no neighbour just means no callout.
    try:
        import struct

        values = embed_fn(stance)
        vec = struct.pack(f"{len(values)}f", *values)
        rows = conn.execute(
            "SELECT p.text, d.title, k.distance FROM "
            "(SELECT proposition_id, distance FROM proposition_vectors "
            " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
            "JOIN propositions p ON p.id = k.proposition_id "
            "JOIN documents d ON d.id = p.doc_id",
            (vec, _MAX_NEIGHBOURS),
        ).fetchall()
    except Exception as exc:
        log.warning("trajectory: proposition neighbours unavailable for tension check: %s", exc)
        return []
    return [
        _Neighbour(text=r["text"], source=r["title"])
        for r in rows
        if r["distance"] is not None and r["distance"] <= _MAX_DISTANCE
    ]


_TENSION_PROMPT = """\
Below is a position the owner of a knowledge vault holds, and claims already stored in that vault.

Identify ONLY genuine CONTRADICTIONS — a stored claim that cannot be true at the same time as the \
position, or that the position directly reverses. Do NOT report claims that merely discuss the \
same topic, add nuance, qualify, or elaborate. Most of the time the correct answer is an empty \
list; return one. A false tension is far worse than a missed one.

Return ONLY JSON: {{"tensions": [{{"conflicts_with": "<the stored claim, copied exactly>", \
"reason": "<one line: what makes them incompatible>"}}]}}

POSITION
{stance}

STORED CLAIMS
{claims}
"""


def find_tensions(
    conn,
    stance: str,
    *,
    subject_kind: str,
    subject_key: str,
    runner=None,
    embed_fn=None,
    model: str | None = None,
    on_result=None,
) -> list[Tension]:
    """Near-neighbour claims genuinely incompatible with `stance` (advisory, may be empty).

    Grounded-or-silent (invariant 3): a returned tension's `conflicts_with` must match a claim
    actually put in front of the judge — a model that paraphrases or invents the conflicting claim
    is dropped, not repaired."""
    neighbours = (
        _neighbour_positions(conn, stance, subject_kind, subject_key)
        + _neighbour_propositions(conn, stance, embed_fn=embed_fn)
    )[:_MAX_NEIGHBOURS]
    if not neighbours:
        return []

    prompt = _TENSION_PROMPT.format(
        stance=stance,
        claims="\n".join(f"- {n.text}" for n in neighbours),
    )
    try:
        verdicts = run_structured(
            prompt, schema=_TensionVerdicts, model=model or load().agent.model,
            runner=runner, on_result=on_result,
        )
    except ClaudeError as exc:  # advisory: no callout is the correct degradation
        log.warning("trajectory: tension judging failed: %s", exc)
        return []

    by_text = {n.text.strip(): n for n in neighbours}
    out: list[Tension] = []
    for v in verdicts.tensions:
        match = by_text.get(v.conflicts_with.strip())
        if match is None:  # not one of the claims offered -> ungrounded, dropped
            log.debug("trajectory: dropped ungrounded tension %r", v.conflicts_with[:80])
            continue
        out.append(Tension(stance=stance, conflicts_with=match.text, reason=v.reason,
                           source=match.source))
    return out


# --- rendering -----------------------------------------------------------------------------------


def render_trajectory(traj: Trajectory, *, heading: bool = True) -> str:
    """The trajectory as markdown: dated chain + any tension callouts.

    Says "no recorded positions" rather than composing a narrative when the chain is empty — the
    absence of data is information, and inventing continuity here would be the exact failure the
    grounded-or-silent invariant exists to prevent."""
    lines: list[str] = []
    if heading:
        lines.append(f"### {traj.label}")
    if not traj.entries:
        lines.append("_No recorded positions yet._")
        return "\n".join(lines)

    for e in traj.entries:
        source = f" — {e.source_title}" if e.source_title else ""
        lines.append(f"- **{e.dated_at}**{source}: {e.stance}")
    for t in traj.tensions:
        lines.append("")
        lines.append("> [!ai] Tension")
        lines.append(f"> Your latest position: {t.stance}")
        lines.append(f"> Conflicts with ({t.source}): {t.conflicts_with}")
        if t.reason:
            lines.append(f"> {t.reason}")
    return "\n".join(lines)


def write_trajectory_note(
    traj: Trajectory, *, run_id: str, out_dir=None
) -> "object":
    """Render one subject's trajectory into an agent-owned `_generated/` note (invariant 5).

    `_generated/` is corpus-excluded, so this rendering can never be re-ingested and fed back to
    the agent as if it were the owner's own material."""
    from pathlib import Path

    from locus.vault.writer import write_generated_note

    base = Path(out_dir) if out_dir is not None else (load().paths.notes / "_generated" / "trajectories")
    slug = "".join(c if c.isalnum() else "-" for c in traj.label.lower()).strip("-") or "subject"
    return write_generated_note(
        base / f"{slug}.md",
        render_trajectory(traj),
        run_id=run_id,
        extra={"title": f"Trajectory — {traj.label}", "subject_kind": traj.subject_kind},
    )


def resolve_subject(conn, subject: str) -> tuple[str, str | None]:
    """Resolve a subject NAME to the (kind, key) the position tables use.

    A project object title wins over a concept of the same name — the object is the more specific
    thing the owner named. Returns key=None when nothing matches, so callers report "no
    trajectory" rather than rendering an empty one for a subject that does not exist.

    Shared by the CLI and the MCP surface deliberately: two copies of this would drift, and a
    subject that resolves in one surface but not the other is a confusing bug to chase."""
    row = conn.execute(
        "SELECT id FROM objects WHERE type='project' AND title=? COLLATE NOCASE", (subject,)
    ).fetchone()
    if row:
        return "project", str(row["id"])
    for kind, key, _n in state.subjects_with_positions(conn, limit=500):
        if kind == "concept" and parse_entity_key(key)[0].casefold() == subject.casefold():
            return "concept", key
    return "concept", None


def all_trajectories(conn, *, limit: int = 50) -> list[Trajectory]:
    """Every subject that has a recorded chain — what `locus evolution` and the MCP read list."""
    return [
        build_trajectory(conn, kind, key)
        for kind, key, _ in state.subjects_with_positions(conn, limit=limit)
    ]
