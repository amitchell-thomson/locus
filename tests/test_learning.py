"""Spec L, step L1: learning state, its pace, its backup; the Learning/ folder in the sync."""

from __future__ import annotations

from datetime import date

import pytest

from locus import backup as bk
from locus.heuris import db as hdb
from locus.heuris import learning as L


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "h.db")
    yield c
    c.close()


def test_a_goal_has_a_readable_id_and_a_pace_that_changes_any_time(conn):
    with pytest.raises(L.LearningError, match="lowercase words"):
        L.add_goal(conn, "Interview Prep", "x")
    g = L.add_goal(conn, "interview-prep", "Quant interview mastery", priority=1)
    assert g.pace is None and g.state == "proposed"
    L.set_pace(conn, "interview-prep", 5, since="2026-11-16")
    # a vacation ramp that ends by itself (16 Nov: "easy to ramp up or down at any point in time")
    L.set_pace(conn, "interview-prep", 8, since="2026-12-06", until="2027-01-10", note="Christmas")
    assert L.current_pace(conn, "interview-prep", date(2026, 11, 20)) == (5.0, None)
    assert L.current_pace(conn, "interview-prep", date(2026, 12, 20)) == (8.0, "2027-01-10")
    assert L.current_pace(conn, "interview-prep", date(2027, 1, 11)) == (5.0, None)
    assert len(L.pace_history(conn, "interview-prep")) == 2        # every change kept
    with pytest.raises(L.LearningError):
        L.set_pace(conn, "interview-prep", 5, since="2027-01-10", until="2027-01-01")


def test_learning_state_survives_a_backup_and_restores_without_overwriting(tmp_path):
    db = tmp_path / "heuris.db"
    c = hdb.connect(db)
    L.add_goal(c, "quast", "QUAST start to finish", priority=3)
    L.set_pace(c, "quast", 1, since="2026-11-16")
    c.close()
    snap = tmp_path / "snap"
    snap.mkdir()
    assert bk._backup_ledger(db, snap / "heuris-ledger.db") >= 2
    fresh = tmp_path / "fresh.db"
    added = bk.restore_ledger(snap, fresh)
    assert added["goals"] == 1 and added["pace"] == 1
    assert bk.restore_ledger(snap, fresh)["goals"] == 0             # nothing twice
    c = hdb.connect(fresh)
    assert L.get_goal(c, "quast").pace == 1.0
    c.close()


def test_learning_folder_syncs_its_pdfs_but_never_its_code(tmp_path):
    from locus import files_content, files_sync

    root = tmp_path / "files"
    (root / "Learning" / ".git").mkdir(parents=True)                # the folder is a git repo
    sheets = root / "Learning" / "interview-prep" / "sheets"
    code = root / "Learning" / "cpp-oxdaq" / "code" / "ex01"
    sheets.mkdir(parents=True)
    code.mkdir(parents=True)
    (sheets / "baseline.pdf").write_bytes(b"%PDF-1.4 sheet")
    (code / "solution.pdf").write_bytes(b"%PDF-1.4 not for the tablet")
    (code / "main.cpp").write_text("int main() {}")
    folders = files_sync.laptop_folders(root)
    assert "Learning/interview-prep/sheets" in folders
    assert not any("/code" in f for f in folders)                   # code folders stay on the laptop
    pdfs = files_content.laptop_pdfs(root, {})
    assert "Learning/interview-prep/sheets/baseline.pdf" in pdfs
    assert not any("/code/" in p for p in pdfs)
    # a goal's folder is not a registry entry, and is never asked about
    plan = files_sync.plan(folders, {"Learning"}, {a: set() for a in files_sync.AREAS})
    assert "Learning/interview-prep" in plan.to_tablet and not plan.questions
