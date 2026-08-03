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
import sqlite3
from dataclasses import dataclass, field

from pydantic import BaseModel, Field

from locus.agent import state
from locus.agent.claude import ClaudeError, run_structured
from locus.agent.state import parse_entity_key
from locus.config import load

log = logging.getLogger(__name__)


def _utcnow() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).isoformat()

# How many near-neighbour claims to put in front of the judge. Small on purpose: the call is per
# stance, and a long candidate list invites the model to find something rather than nothing.
_MAX_NEIGHBOURS = 14
# Stored propositions this far (cosine distance) from the stance are not about the same thing;
# nomic distances on genuinely related claims sit well inside this.
# THE FILTER THAT MADE THIS INERT. At 0.55 the only claim close enough to survive was a
# PARAPHRASE of the stance itself — measured 2026-08-03 on "Market has over-done pricing in the
# easing cycle": one neighbour kept at d=0.354 (his own note, restated), and everything from 0.79
# to 0.84 dropped, which is where a genuine counter-claim sits. So the pass could only ever
# compare a view to its own echo, and 16 recorded positions produced zero tensions.
#
# It also contradicted this module's own premise: "semantic similarity ALONE cannot do this — a
# stance and its negation are near-identical embeddings". If that is true, distance cannot be the
# filter, and the JUDGE has to be. The window is now wide enough to contain a disagreement, and
# the prompt does the discriminating.
_MAX_DISTANCE = 0.92

# THE JUDGE'S IDENTITY, and the cache key that goes with it. Widening `_MAX_DISTANCE` fixed
# retrieval and the section stayed blank anyway: measured 2026-08-03, all 16 positions retrieved
# 14 neighbours each and the judge returned zero, because the prompt was told "most of the time
# the correct answer is an empty list; a false tension is far worse than a missed one". Position
# 5's neighbours included, verbatim, "Realised cash flow is the actual cash that exchanges hands
# when a coupon is physically paid" against his "once it fixes ... it becomes a realised cashflow,
# not a risk" — a real disagreement, correctly retrieved, declined.
#
# Rebalanced to CONFLICT rather than contradiction, the same 16 positions yield 2 tensions and 14
# still yield nothing, which is the number that matters: it did not become a topic matcher.
#
# BUMP THIS whenever the prompt or the neighbour rule changes. `store_tensions` re-judges any
# position whose cached verdict carries a different version, because a cached "no" that outlives
# the judge that said it is how a fixed pass ships and changes nothing.
_JUDGE_VERSION = "2026-08-03-conflict"


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
    # 'object' addresses a thread (an idea or question) by id; 'project' addresses a project the
    # same way, so one lookup serves both.
    row = conn.execute("SELECT title FROM objects WHERE id=?", (subject_key,)).fetchone()
    return row["title"] if row else subject_key


# --- the chain (deterministic) -------------------------------------------------------------------


def build_trajectory(
    conn, subject_kind: str, subject_key: str, *, with_tensions: bool = False, runner=None
) -> Trajectory:
    """The owner's dated positions on one subject, oldest first (§6.3).

    `with_tensions=True` additionally runs the judged tension pass over the LATEST stance — the
    one a new capture just added is the one worth warning about."""
    # ONE chain, two provenances: stances the structurer EXTRACTED from his notes, and passes he
    # AUTHORED on the daily page. Reading only the first meant the half he typed himself never
    # appeared in his own trajectory (see `state.development_positions`).
    positions = sorted(
        state.positions_for(conn, subject_kind, subject_key)
        + state.development_positions(conn, subject_kind, subject_key),
        key=lambda p: (p.dated_at or "", p.id),
    )
    traj = Trajectory(
        subject_kind=subject_kind, subject_key=subject_key,
        label=subject_label(conn, subject_kind, subject_key),
    )
    titles = _doc_titles(conn, [p.source_doc_id for p in positions if p.source_doc_id])
    by_uri = _titles_by_uri(conn, [p.source_uri for p in positions if p.source_uri])
    traj.entries = [
        TrajectoryEntry(
            dated_at=p.dated_at, stance=p.stance, source_doc_id=p.source_doc_id,
            # source_uri FIRST: notes_sync replaces a changed note with a new document id, so the
            # id goes stale on any edit while the path does not (migration 0012).
            source_title=by_uri.get(p.source_uri) or titles.get(p.source_doc_id),
        )
        for p in positions
    ]
    if with_tensions and positions:
        traj.tensions = find_tensions(
            conn, positions[-1].stance, subject_kind=subject_kind, subject_key=subject_key,
            runner=runner,
        )
    return traj


def _titles_by_uri(conn, uris: list[str]) -> dict[str, str]:
    """source_uri -> current document title. The provenance lookup that survives a re-ingest."""
    keys = sorted({u for u in uris if u})
    if not keys:
        return {}
    placeholders = ",".join("?" * len(keys))
    return {
        r["source_uri"]: r["title"]
        for r in conn.execute(
            f"SELECT source_uri, title FROM documents WHERE source_uri IN ({placeholders})", keys
        )
    }


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
    # LOGGED because this is the gate that was dead. At 0.55 it admitted only paraphrases of the
    # owner's own stance, so the pass could never find a contradiction, and nothing said so — the
    # rejects were the only evidence and no one kept them (`observe/gates.py`).
    from locus.observe import gates

    out: list[_Neighbour] = []
    for r in rows:
        ok = r["distance"] is not None and r["distance"] <= _MAX_DISTANCE
        gates.record(
            conn, "trajectory.max_distance", rejected=not ok,
            value=None if ok else f"d={r['distance']:.3f} {str(r['text'])[:70]}",
        )
        if ok:
            out.append(_Neighbour(text=r["text"], source=r["title"]))
    return out


# WHAT COUNTS AS A TENSION, and why the first bar was wrong. The original prompt asked for
# logical CONTRADICTIONS only, and told the judge that "most of the time the correct answer is an
# empty list". Measured 2026-08-03 over every stored position: 0 tensions from 16, while position
# 5's own neighbour list contained the claim "Realised cash flow is the actual cash that exchanges
# hands when a coupon is physically paid" against his stance "once it fixes ... it becomes a
# realised cashflow, not a risk". Those two cannot both be right about when a cashflow is
# realised — but it is not a strict logical contradiction, so the judge correctly returned
# nothing and the section stayed blank.
#
# The owner asked for something that "tells you when you're wrong", and using a term differently
# from the material he is learning it from IS that. So the ask is now conflict rather than
# contradiction, with the three shapes named explicitly, and the thumb is off the scale. What is
# NOT relaxed: the claim must be copied verbatim (the caller drops any that was not offered), and
# agreement/elaboration is still excluded — which is what keeps this from becoming a topic matcher.
_TENSION_PROMPT = """\
Below is a position the owner of a knowledge vault holds, and claims already stored in that vault.

Identify stored claims that genuinely CONFLICT with the position. A claim conflicts when:
  - it cannot be true at the same time as the position, or the position reverses it; or
  - it defines or uses a key term in a way incompatible with how the position uses it; or
  - it states a condition or counter-example under which the position does not hold.

Do NOT report claims that agree with the position, merely discuss the same topic, or add nuance \
and elaboration without disagreeing. Judge the substance, not the wording: two claims that say \
the same thing in different words do not conflict. If nothing genuinely conflicts, return an \
empty list — but do not strain to avoid reporting a real disagreement.

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


# --- storing tensions, so the daily page can offer one -------------------------------------------


def store_tensions(conn, *, limit: int = 4, runner=None, model: str | None = None) -> int:
    """Judge recent positions for contradictions and store what survives. Returns how many.

    Overnight work. `compose_daily` may not call a model (§18), so the page reads `belief_tensions`
    and prints; the thinking happens here. Positions already judged are skipped, so a re-run costs
    nothing and the pass is safe on a timer.
    """
    from locus.agent import state

    written = 0
    rows = conn.execute(
        "SELECT id, subject_kind, subject_key, stance FROM belief_positions "
        "ORDER BY dated_at DESC, id DESC LIMIT ?",
        (limit * 6,),
    ).fetchall()
    for row in rows:
        if written >= limit:
            break
        # RE-JUDGE WHEN THE CORPUS HAS GROWN. A verdict is cached so the pass does not re-pay for
        # the same "no" every night, but it is not permanent: a contradiction can only be found
        # against material that exists, so every new document is a new chance. Skipping forever
        # would freeze the answer at whatever the corpus happened to hold the first night.
        #
        # AND RE-JUDGE WHEN THE JUDGE HAS CHANGED. Keying the cache on the corpus alone is silent
        # about the prompt: the 2026-08-03 rebalance would have shipped, passed its tests, and
        # changed nothing on the page, because every position already carried a "none found"
        # marker and the last ingest predated all of them. A verdict is only reusable if the same
        # judge produced it.
        seen = conn.execute(
            "SELECT MAX(written_at) AS at, judge_version AS ver FROM belief_tensions "
            "WHERE position_id=?",
            (row["id"],),
        ).fetchone()
        if seen and seen["at"] and (seen["ver"] or "") == _JUDGE_VERSION:
            newer = conn.execute(
                "SELECT 1 FROM documents WHERE ingested_at > ? LIMIT 1", (seen["at"],)
            ).fetchone()
            if not newer:
                continue
        try:
            tensions = find_tensions(
                conn, row["stance"], subject_kind=row["subject_kind"],
                subject_key=row["subject_key"], runner=runner, model=model,
            )
        except Exception as exc:                      # advisory: never block the nightly run
            log.warning("trajectory: tension pass failed for position %s: %s", row["id"], exc)
            continue
        if not tensions:
            # A position with no tension still counts as JUDGED — without a marker the pass would
            # re-pay for the same "no" every night, which is how a cached verdict earns its keep.
            with conn:
                # UPSERT, not INSERT OR IGNORE: a stale marker from a previous judge already
                # occupies (position_id, ''), so ignoring the conflict would leave the old
                # version in place and the position would be re-judged again every night.
                conn.execute(
                    "INSERT INTO belief_tensions (position_id, stance, conflicts_with, "
                    "reason, source, written_at, dismissed_at, judge_version) "
                    "VALUES (?,?,'','','',?,?,?) "
                    "ON CONFLICT(position_id, conflicts_with) DO UPDATE SET "
                    "written_at=excluded.written_at, judge_version=excluded.judge_version",
                    (row["id"], row["stance"], _utcnow(), _utcnow(), _JUDGE_VERSION),
                )
            continue
        for t in tensions:
            with conn:
                conn.execute(
                    "INSERT OR IGNORE INTO belief_tensions (position_id, stance, conflicts_with, "
                    "reason, source, written_at, judge_version) VALUES (?,?,?,?,?,?,?)",
                    (row["id"], t.stance, t.conflicts_with, t.reason, t.source, _utcnow(),
                     _JUDGE_VERSION),
                )
            written += 1
    return written


def open_tensions(conn, *, limit: int = 5) -> list[dict]:
    """Stored, undismissed tensions — what the daily page offers. Newest first."""
    try:
        rows = conn.execute(
            "SELECT id, position_id, stance, conflicts_with, reason, source FROM belief_tensions "
            "WHERE dismissed_at IS NULL AND TRIM(conflicts_with) != '' "
            "ORDER BY written_at DESC, id DESC LIMIT ?",
            (limit,),
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    return [dict(r) for r in rows]
