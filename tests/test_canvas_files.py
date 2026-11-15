"""P3: the mirror into the library (server) and into ~/files (laptop), never over his work."""

from __future__ import annotations

import hashlib
import json

from locus import canvas_files

COURSES = {
    "b18": {"canvas": 9, "folder": "b18-bio", "active": True},
    "p1": {"canvas": 8, "folder": "p1-maths", "active": False},
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def mirror(tmp_path, files: dict[str, tuple[str, bytes]]):
    """files: path -> (course, bytes)."""
    root = tmp_path / "canvas"
    man = {"files": {}, "links": {}}
    for i, (rel, (course, data)) in enumerate(files.items()):
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_bytes(data)
        man["files"][str(i)] = {"id": i, "course": course, "path": rel, "status": "ok", "sha256": sha(data)}
    (root / "manifest.json").write_text(json.dumps(man))
    return root


def test_this_years_documents_are_queued_for_the_library_once_and_past_years_and_code_are_not(tmp_path):
    root = mirror(tmp_path, {
        "b18-bio/cellular-physiology/b18-lecture-1.pdf": ("b18", b"L1"),
        "b18-bio/cellular-physiology/b18-solutions.pdf": ("b18", b"SOL"),
        "b18-bio/cellular-physiology/b18-lecture-2.pdf": ("b18", b"L2"),
        "b18-bio/cellular-physiology/hhmodel.m": ("b18", b"code"),
        "p1-maths/calculus/p1-sheet.pdf": ("p1", b"old"),
    })
    inc = tmp_path / "incoming"
    q = canvas_files.queue_library(COURSES, {sha(b"L2")}, incoming=inc, root=root)
    # Solutions included: the library is fed from the mirror, not from what reaches the tablet.
    assert sorted(q.queued) == ["b18-bio/cellular-physiology/b18-lecture-1.pdf",
                                "b18-bio/cellular-physiology/b18-solutions.pdf"]
    assert q.in_library == 1
    assert (inc / "studies/b18-bio/cellular-physiology/b18-solutions.pdf").read_bytes() == b"SOL"
    # Ingested and gone from incoming: not queued again.
    for p in inc.rglob("*.pdf"):
        p.unlink()
    q2 = canvas_files.queue_library(COURSES, set(), incoming=inc, root=root)
    assert (q2.queued, q2.in_library) == ([], 0)


def test_a_quarantined_copy_is_never_queued_again(tmp_path):
    root = mirror(tmp_path, {"b18-bio/x/bad.pdf": ("b18", b"bad")})
    inc = tmp_path / "incoming"
    (inc / ".quarantine/studies/b18-bio/x").mkdir(parents=True)
    (inc / ".quarantine/studies/b18-bio/x/bad.pdf").write_bytes(b"bad")
    q = canvas_files.queue_library(COURSES, set(), incoming=inc, root=root)
    assert q.held == ["b18-bio/x/bad.pdf"] and not q.queued


def run_files(tmp_path, root, studies, state):
    man = json.loads((root / "manifest.json").read_text())

    def copy(pairs):
        for s, t in pairs:
            t.parent.mkdir(parents=True, exist_ok=True)
            t.write_bytes((root / s).read_bytes())

    return canvas_files.to_files(COURSES, studies=studies, manifest=man, copy=copy, state_path=state)


def test_files_arrive_once_and_never_over_his_work(tmp_path):
    rel = "b18-bio/cellular-physiology/b18-lecture-1.pdf"
    root = mirror(tmp_path, {rel: ("b18", b"v1"), "p1-maths/c/old.pdf": ("p1", b"old"),
                             "b18-bio/cellular-physiology/notes.pdf": ("b18", b"n")})
    studies, state = tmp_path / "files" / "Studies", tmp_path / "state.json"
    p = run_files(tmp_path, root, studies, state)
    assert sorted(c.why for c in p.copies) == ["new", "new"]
    assert (studies / rel).read_bytes() == b"v1" and not (studies / "p1-maths").exists()
    assert not run_files(tmp_path, root, studies, state).copies          # a second run: nothing

    # Canvas changes a file he never touched: replaced in place.
    root2 = mirror(tmp_path, {rel: ("b18", b"v2"), "b18-bio/cellular-physiology/notes.pdf": ("b18", b"n")})
    p = run_files(tmp_path, root2, studies, state)
    assert [c.why for c in p.copies] == ["replaced"] and (studies / rel).read_bytes() == b"v2"

    # He edits it, then Canvas changes it again: the new one lands beside his, once.
    (studies / rel).write_bytes(b"his edit")
    root3 = mirror(tmp_path, {rel: ("b18", b"v3"), "b18-bio/cellular-physiology/notes.pdf": ("b18", b"n")})
    p = run_files(tmp_path, root3, studies, state)
    assert [c.why for c in p.copies] == ["beside"]
    assert (studies / rel).read_bytes() == b"his edit"
    assert (studies / "b18-bio/cellular-physiology/b18-lecture-1-v2.pdf").read_bytes() == b"v3"
    assert not run_files(tmp_path, root3, studies, state).copies

    # He deletes one: it is not brought back.
    (studies / "b18-bio/cellular-physiology/notes.pdf").unlink()
    p = run_files(tmp_path, root3, studies, state)
    assert not p.copies and p.gone == ["b18-bio/cellular-physiology/notes.pdf"]


def test_a_module_he_is_not_taking_goes_nowhere(tmp_path):
    root = mirror(tmp_path, {"b18-bio/mine/a.pdf": ("b18", b"a"), "b18-bio/other-project/b.pdf": ("b18", b"b")})
    skip = {"b18": {"other-project"}}
    q = canvas_files.queue_library(COURSES, set(), incoming=tmp_path / "inc", root=root, skip=skip)
    assert q.queued == ["b18-bio/mine/a.pdf"]
    man = json.loads((root / "manifest.json").read_text())
    p = canvas_files.plan_files(man, COURSES, {}, tmp_path / "Studies", skip=skip)
    assert [c.src for c in p.copies] == ["b18-bio/mine/a.pdf"]


def test_the_skip_list_names_real_folders():
    for rid, folders in canvas_files.skipped_modules().items():
        assert all(f == f.lower() and " " not in f for f in folders), rid


def test_a_file_he_already_has_under_his_own_name_is_not_copied_beside_it(tmp_path):
    root = mirror(tmp_path, {"b18-bio/cellular-physiology/b18-lecture-1.pdf": ("b18", b"L1")})
    man = json.loads((root / "manifest.json").read_text())
    man["files"]["0"]["size"] = 2
    studies = tmp_path / "Studies"
    (studies / "b18-bio/elsewhere").mkdir(parents=True)
    (studies / "b18-bio/elsewhere/Lecture 1 (mine).pdf").write_bytes(b"L1")
    p = canvas_files.plan_files(man, COURSES, {}, studies, skip={})
    assert not p.copies and p.same == 1
