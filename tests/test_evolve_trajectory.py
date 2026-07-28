"""Understanding-evolution: the dated chain and the advisory tension pass (plan §3.4, §6.3).

Model-free — the judge runner and the embedder are injected. The behaviours that matter are that
the chain is a pure join (it cannot invent a stance) and that a tension the judge names but was
never shown is dropped rather than rendered.
"""

from __future__ import annotations

import json
import struct
from pathlib import Path

import pytest

from locus.agent import state
from locus.agent.claude import ClaudeError, ClaudeResult
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.evolve import trajectory as tj

KEY = state.entity_key("portfolio construction", "concept")


def _doc(conn, doc_id, title):
    with conn:
        conn.execute(
            "INSERT INTO documents (id, content_hash, source_type, source_uri, raw_path, title, "
            "ingested_at, ingest_model) VALUES (?,?,?,?,?,?,?,?)",
            (doc_id, f"h{doc_id}", "markdown", f"notes/{doc_id}.md", f"r{doc_id}", title,
             "2026-01-01T00:00:00Z", "test"),
        )


def _proposition(conn, doc_id, text, vec):
    with conn:
        conn.execute(
            "INSERT INTO sections (id, doc_id, position, title, summary) VALUES (?,?,0,'s','s')",
            (doc_id, doc_id),
        )
        cur = conn.execute(
            "INSERT INTO propositions (section_id, doc_id, position, text, embed_model) "
            "VALUES (?,?,0,?,'m')",
            (doc_id, doc_id, text),
        )
        packed = struct.pack("768f", *(vec + [0.0] * (768 - len(vec))))
        conn.execute(
            "INSERT INTO proposition_vectors (proposition_id, embedding) VALUES (?,?)",
            (cur.lastrowid, packed),
        )


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "evolve.db"
    migrate(db)
    c = get_connection(db)
    _doc(c, 1, "Early reading notes")
    _doc(c, 2, "Brevan Howard notes")
    state.record_position(c, subject_kind="concept", subject_key=KEY, source_doc_id=1,
                          stance="mean-variance optimisation is the right default",
                          dated_at="2026-01-15")
    state.record_position(c, subject_kind="concept", subject_key=KEY, source_doc_id=2,
                          stance="equal weight beats mean-variance out of sample",
                          dated_at="2026-05-01")
    yield c
    c.close()


def _runner(payload):
    return lambda prompt, model: ClaudeResult(text=json.dumps(payload), cost_usd=0.001)


# --- the chain ---------------------------------------------------------------------------------


def test_trajectory_is_ordered_by_source_date_with_provenance(conn):
    traj = tj.build_trajectory(conn, "concept", KEY)
    assert traj.label == "portfolio construction"
    assert [e.dated_at for e in traj.entries] == ["2026-01-15", "2026-05-01"]
    assert traj.entries[1].source_title == "Brevan Howard notes"


def test_empty_trajectory_says_so_rather_than_composing_one(conn):
    traj = tj.build_trajectory(conn, "concept", state.entity_key("nothing", "concept"))
    assert not traj
    assert "No recorded positions" in tj.render_trajectory(traj)


def test_render_shows_the_dated_chain(conn):
    out = tj.render_trajectory(tj.build_trajectory(conn, "concept", KEY))
    assert "### portfolio construction" in out
    assert "- **2026-01-15** — Early reading notes: mean-variance" in out
    assert out.index("2026-01-15") < out.index("2026-05-01")


def test_project_subject_is_labelled_by_its_object_title(conn):
    oid, _ = state.upsert_object(conn, type_="project", title="regime-ml")
    state.record_position(conn, subject_kind="project", subject_key=str(oid),
                          stance="pivoting to flow features", dated_at="2026-06-01")
    traj = tj.build_trajectory(conn, "project", str(oid))
    assert traj.label == "regime-ml"


def test_all_trajectories_lists_every_subject_with_a_chain(conn):
    assert [t.label for t in tj.all_trajectories(conn)] == ["portfolio construction"]


# --- tensions ------------------------------------------------------------------------------------


def test_tension_against_an_earlier_position_is_reported(conn):
    earlier = "mean-variance optimisation is the right default"
    tensions = tj.find_tensions(
        conn, "equal weight beats mean-variance out of sample",
        subject_kind="concept", subject_key=KEY,
        embed_fn=lambda _t: [0.0] * 768,
        runner=_runner({"tensions": [
            {"conflicts_with": earlier, "reason": "one prefers MVO, the other rejects it"}
        ]}),
    )
    assert len(tensions) == 1
    assert tensions[0].conflicts_with == earlier
    assert tensions[0].source == "your position of 2026-01-15"


def test_a_tension_the_judge_was_never_shown_is_dropped(conn):
    """Grounded-or-silent: a paraphrased or invented conflicting claim is not a citation."""
    tensions = tj.find_tensions(
        conn, "equal weight beats mean-variance out of sample",
        subject_kind="concept", subject_key=KEY,
        embed_fn=lambda _t: [0.0] * 768,
        runner=_runner({"tensions": [
            {"conflicts_with": "Markowitz proved diversification is optimal", "reason": "made up"}
        ]}),
    )
    assert tensions == []


def test_no_tension_is_the_normal_answer(conn):
    tensions = tj.find_tensions(
        conn, "equal weight beats mean-variance out of sample",
        subject_kind="concept", subject_key=KEY,
        embed_fn=lambda _t: [0.0] * 768, runner=_runner({"tensions": []}),
    )
    assert tensions == []


def test_near_propositions_are_offered_and_far_ones_are_not(conn):
    seen = {}

    def runner(prompt, model):
        seen["prompt"] = prompt
        return ClaudeResult(text='{"tensions": []}')

    _doc(conn, 3, "Covariance estimation paper")
    _doc(conn, 4, "Tanker flow notes")
    _proposition(conn, 3, "Shrinkage covariance stabilises mean-variance weights", [1.0, 0.0])
    _proposition(conn, 4, "Tanker freight rates lead crude spreads", [0.0, 1.0])
    tj.find_tensions(
        conn, "equal weight beats mean-variance out of sample", subject_kind="concept",
        subject_key=KEY, embed_fn=lambda _t: [1.0, 0.0] + [0.0] * 766, runner=runner,
    )
    assert "Shrinkage covariance" in seen["prompt"]     # distance 0 -> offered
    assert "Tanker freight rates" not in seen["prompt"]  # orthogonal -> beyond _MAX_DISTANCE


def test_judge_failure_yields_no_callout(conn):
    def failing(prompt, model):
        raise ClaudeError("nope")

    assert tj.find_tensions(
        conn, "x y z", subject_kind="concept", subject_key=KEY,
        embed_fn=lambda _t: [0.0] * 768, runner=failing,
    ) == []


def test_embedding_failure_degrades_to_positions_only(conn):
    def bad_embed(_t):
        raise RuntimeError("ollama down")

    tensions = tj.find_tensions(
        conn, "equal weight beats mean-variance out of sample", subject_kind="concept",
        subject_key=KEY, embed_fn=bad_embed,
        runner=_runner({"tensions": [
            {"conflicts_with": "mean-variance optimisation is the right default", "reason": "r"}
        ]}),
    )
    assert len(tensions) == 1  # stored positions still available without the embedder


def test_no_neighbours_means_no_model_call(conn):
    def explode(prompt, model):
        raise AssertionError("must not call the judge with nothing to judge")

    # A subject with a single position and no propositions has nothing to compare against.
    key = state.entity_key("lonely", "concept")
    state.record_position(conn, subject_kind="concept", subject_key=key, stance="s",
                          dated_at="2026-01-01")
    assert tj.find_tensions(conn, "s", subject_kind="concept", subject_key=key,
                            embed_fn=lambda _t: [0.0] * 768, runner=explode) == []


def test_tension_renders_as_an_advisory_callout(conn):
    traj = tj.build_trajectory(conn, "concept", KEY)
    traj.tensions = [tj.Tension(stance="now equal weight", conflicts_with="MVO is the default",
                                reason="incompatible defaults", source="your position of 2026-01-15")]
    out = tj.render_trajectory(traj)
    assert "> [!ai] Tension" in out
    assert "> Conflicts with (your position of 2026-01-15): MVO is the default" in out


def test_trajectory_note_is_written_as_agent_owned(conn, tmp_path):
    traj = tj.build_trajectory(conn, "concept", KEY)
    res = tj.write_trajectory_note(traj, run_id="7", out_dir=tmp_path / "_generated")
    text = res.path.read_text()
    assert "author: agent" in text and "generated: true" in text and "source_run: 7" in text
    assert res.path.name == "portfolio-construction.md"
