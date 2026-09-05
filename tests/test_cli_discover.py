"""The `locus discover` CLI wiring — the flag COMBINATIONS the deployed timers actually run.

WHY THIS FILE EXISTS. Until 2026-09-05 nothing tested `cmd_discover` at all. Every flag worked
in isolation and the modules beneath them were well covered, but the weekly unit runs

    locus discover --harvest --profiles --propose --push

and an unconditional `return` at the end of the propose branch made `--push` unreachable. The
result was invisible for five weeks: 2,192 candidates harvested, 10 queued as proposals, zero
delivered to the device, every unit exiting 0. This is §3's failure class — a path that looks
wired and isn't — and the only test that can catch it is one that asserts on the combination
rather than the parts.

Model-free and network-free: `--push`'s fetch and rmapi calls are monkeypatched, so what is under
test is the control flow that decides whether they are reached at all.
"""

from __future__ import annotations

import argparse
import sqlite3
from pathlib import Path

import pytest

from locus import cli
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.reading import proposals as P


@pytest.fixture()
def db(tmp_path: Path) -> Path:
    path = tmp_path / "discover.db"
    migrate(path)
    return path


@pytest.fixture()
def conn(db: Path):
    """The test's own handle. `cmd_discover` opens and closes its own, so this one survives."""
    c = get_connection(db)
    yield c
    c.close()


def _args(**kw) -> argparse.Namespace:
    """Every flag the discover subparser defines, defaulted off — as argparse would build it."""
    base = dict(
        link_reading=False, owner_only=False, write_why=None, rewrite_after=None,
        pull=False, seed=None, kind="paper", why=None, evidence=None, authors=None,
        url=None, deliver=None, file=None, stub=False, force=False, ttl=None,
        dry_run=False, harvest=False, profiles=False, rank=False, propose=False,
        top=10, no_sweep=False, no_judge=False, push=False, staging=None,
    )
    base.update(kw)
    return argparse.Namespace(**base)


class _FakeDiscoveryCfg:
    """Pinned config.

    NEVER `config.load()`: `config.toml` is gitignored, so a test that inherits it passes or
    fails per machine (§13). The caps in particular are the thing under test in the full-folder
    case, and DEFAULT_CAPS has already been changed once by hand.
    """

    caps = {"paper": 10, "book": 1}
    root_folder = "Locus/Reading"
    rmapi_binary = "rmapi"
    judge_enabled = False
    familiarity_weight = 0.25
    gap_weight = 0.0
    citation_weight = 0.0
    judge_drop_at_or_below = 0
    ttl_days = 21


class _FakeCfg:
    discovery = _FakeDiscoveryCfg()


@pytest.fixture()
def wired(db, tmp_path, monkeypatch):
    """Point cmd_discover at the tmp DB and stub out the network and the device."""
    monkeypatch.setattr(cli, "_open", lambda *a, **k: get_connection(db))
    monkeypatch.setattr(cli, "load", lambda: _FakeCfg())
    monkeypatch.setattr(cli, "PUSH_PAUSE_SECONDS", 0.0)   # the real 3s is for the cloud, not pytest

    delivered: list[str] = []

    def fake_fetch(url, dest, **kw):
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(b"%PDF-1.4 stub")
        return dest

    def fake_deliver(c, proposal, pdf_path, **kw):
        P.mark_proposed(c, proposal.id, device_uuid="uuid-x", filename=f"{proposal.title}.pdf",
                        device_folder="Proposed", local_path=str(pdf_path))
        delivered.append(proposal.title)
        return type("D", (), {"proposal_id": proposal.id, "filename": f"{proposal.title}.pdf",
                              "device_uuid": "uuid-x"})()

    monkeypatch.setattr("locus.reading.deliver.fetch_open_access", fake_fetch)
    monkeypatch.setattr("locus.reading.deliver.deliver_proposal", fake_deliver)
    return delivered


def _seed_candidate(conn, title="A Queued Paper", oa="https://example.org/x.pdf") -> int:
    new_id = P.add_candidate(
        conn, kind="paper", title=title, why="grounded in your regime work",
        why_kind="discovery", evidence_key="project:regime-ml",
        authors="Someone", url="https://example.org/abs", oa_pdf_url=oa,
    )
    assert new_id is not None
    return new_id


def test_push_runs_when_combined_with_propose(conn, wired, capsys):
    """THE REGRESSION. `--propose --push` must reach the push step.

    This is the exact combination the weekly unit runs. Asserting `--push` alone works is not
    enough and never was: the bug lived entirely in the branch that `--propose` took first.
    """
    _seed_candidate(conn)

    cli.cmd_discover(_args(propose=True, push=True, no_judge=True))

    out = capsys.readouterr().out
    assert wired == ["A Queued Paper"], f"push did not run under --propose --push; output:\n{out}"


def test_push_alone_still_works(conn, wired, capsys):
    """The isolated path — the one that was always green while the combination was broken."""
    _seed_candidate(conn, title="Solo Push")

    cli.cmd_discover(_args(push=True))

    assert wired == ["Solo Push"]


def test_push_delivering_nothing_says_so_loudly(conn, wired, capsys):
    """A zero must never print as a bare count.

    The old failure printed nothing at all; the shape that replaced it prints `0 paper(s)`, which
    reads as "all quiet" rather than "the shelf is stuck". The loud line is the whole point.
    """
    _seed_candidate(conn, title="Not Open Access", oa=None)

    cli.cmd_discover(_args(push=True))

    out = capsys.readouterr().out
    assert wired == []
    assert "NOTHING DELIVERED" in out
    assert "no open-access PDF" in out


def test_push_records_every_outcome_to_the_gate_log(conn, wired):
    """`locus gates` is what makes a dead step visible, so push must feed it both ways."""
    _seed_candidate(conn, title="Good One")
    _seed_candidate(conn, title="No PDF", oa=None)

    cli.cmd_discover(_args(push=True))

    rows = conn.execute(
        "SELECT passed, rejected FROM gate_log WHERE gate='reading.push'"
    ).fetchall()
    assert rows, "push recorded nothing — a silent step is the bug this file exists for"
    assert sum(r["passed"] for r in rows) == 1
    assert sum(r["rejected"] for r in rows) == 1


def test_empty_queue_is_distinguished_from_a_full_folder(conn, wired, capsys):
    """Two different diagnoses that used to print the same way round."""
    cli.cmd_discover(_args(push=True))

    out = capsys.readouterr().out
    assert "nothing queued" in out
    assert "full" not in out


def test_push_stops_on_a_rate_limit_instead_of_burning_the_queue(conn, wired, monkeypatch, capsys):
    """A 429 is the account being throttled, not one bad file.

    Measured 2026-09-05 on the live shelf: four uploads landed and the remaining six all failed
    429, each having already downloaded its PDF. Continuing past the first one spends bandwidth
    and gate-log space on certain failures, so the loop stops and leaves them queued.
    """
    for i in range(4):
        _seed_candidate(conn, title=f"Paper {i}")

    calls: list[str] = []

    def rate_limited(c, proposal, pdf_path, **kw):
        calls.append(proposal.title)
        raise RuntimeError("rmapi mkdir 'Reading' failed: request failed with status 429")

    monkeypatch.setattr("locus.reading.deliver.deliver_proposal", rate_limited)

    cli.cmd_discover(_args(push=True))

    out = capsys.readouterr().out
    assert len(calls) == 1, f"kept pushing after a 429: tried {calls}"
    assert "rate limited" in out
    # They must still be pushable next run — a throttle is not a rejection of the paper.
    still = conn.execute(
        "SELECT COUNT(*) n FROM reading_proposals WHERE status='candidate'"
    ).fetchone()["n"]
    assert still == 4
