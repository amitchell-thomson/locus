"""P5: PDFs between the tablet and ~/files, both ways, never twice, never a deletion."""

from __future__ import annotations

import hashlib

from locus import files_content as fc

FOLDER = "Studies/b18-biomedical/cellular-physiology"


def doc(name, sha, folders=FOLDER, **kw):
    return {"uuid": f"u-{name}", "folders": folders.split("/"), "name": name, "sha": sha,
            "notebook": sha is None, "sent": False, **kw}


def run(docs, laptop, state=None, manifest=None, **kw):
    kw.setdefault("laptop_dirs", {FOLDER, "Studies/b18-biomedical", "Projects/x"})
    return fc.plan(docs, laptop, manifest or {"files": {}}, state or {}, **kw)


def test_his_pdfs_come_down_clean_and_notebooks_and_locus_own_do_not():
    p = run([doc("B18 Lecture 1", "a"), doc("Sheet notebook", None), doc("Syllabus", "s", sent=True),
             doc("Loose", "r", folders="Reading")], {})
    assert [(d.dest, d.uuid) for d in p.down] == [(f"{FOLDER}/b18-lecture-1.pdf", "u-B18 Lecture 1")]
    assert not p.up


def test_canvas_files_are_left_to_p3_and_p4():
    man = {"files": {"1": {"sha256": "c", "tablet_name": f"{FOLDER}/B18 Slides"},
                     "2": {"sha256": "d", "previous_sha256": "old"}}}
    p = run([doc("B18 Lecture 1", "c"), doc("B18 Slides", "pptx-pdf"), doc("Old upload", "old")],
            {"Studies/b18-biomedical/b18-lecture-2.pdf": "d"}, manifest=man)
    assert not p.down and not p.up


def test_new_laptop_pdfs_go_up_and_reports_figures_solutions_and_past_years_do_not():
    laptop = {f"{FOLDER}/my-summary.pdf": "m", f"{FOLDER}/sheet-2-solutions.pdf": "x",
              "Studies/p1-maths/old.pdf": "o"}
    p = run([], laptop, laptop_only={"Studies/p1-maths"})
    assert [(u.folder, u.title) for u in p.up] == [(FOLDER, "my-summary")]
    assert p.solutions == [f"{FOLDER}/sheet-2-solutions.pdf"]


def test_the_walk_skips_report_figures_and_repos(tmp_path):
    for rel in ("Studies/b1/report/r.pdf", "Studies/b1/figures/f.pdf", "Projects/repo/x.pdf",
                "Studies/b1/notes.pdf"):
        (tmp_path / rel).parent.mkdir(parents=True, exist_ok=True)
        (tmp_path / rel).write_bytes(rel.encode())
    (tmp_path / "Projects/repo/.git").mkdir()
    cache: dict = {}
    got = fc.laptop_pdfs(tmp_path, cache)
    assert list(got) == ["Studies/b1/notes.pdf"]
    assert got["Studies/b1/notes.pdf"] == hashlib.sha256(b"Studies/b1/notes.pdf").hexdigest()
    assert fc.laptop_pdfs(tmp_path, cache) == got        # cached: same answer


def test_the_same_bytes_or_the_same_name_is_never_sent_again():
    # same bytes on both sides: nothing; same name, other bytes: his earlier version, left alone
    p = run([doc("B14 L3", "tab"), doc("Same", "same")],
            {f"{FOLDER}/b14-l3.pdf": "lap", f"{FOLDER}/other.pdf": "same"})
    assert not p.down and not p.up
    assert sorted(p.other_version) == [f"{FOLDER}/B14 L3", f"{FOLDER}/b14-l3.pdf"]


def test_a_file_deleted_on_one_side_is_a_question_never_a_copy_back():
    state = {"down": {"a": f"{FOLDER}/b18-lecture-1.pdf"}, "up": {"m": f"{FOLDER}/my-summary"}}
    # he deleted the lecture on the laptop and the summary on the tablet
    p = run([doc("B18 Lecture 1", "a")], {f"{FOLDER}/my-summary.pdf": "m"}, state=state)
    assert not p.down and not p.up
    assert sorted((q["on"], q["path"]) for q in p.questions) == [
        ("laptop", f"{FOLDER}/b18-lecture-1.pdf"), ("tablet", f"{FOLDER}/my-summary.pdf")]
    state["questions"] = p.questions
    assert fc.settle([f"{FOLDER}/my-summary.pdf"], state) == [f"{FOLDER}/my-summary.pdf"]
    p = run([doc("B18 Lecture 1", "a")], {f"{FOLDER}/my-summary.pdf": "m"}, state=state)
    assert [q["path"] for q in p.questions] == [f"{FOLDER}/b18-lecture-1.pdf"] and not p.up


def test_a_folder_waiting_on_a_question_gets_nothing_and_a_run_is_capped():
    p = run([doc("X", "x", folders="Studies/Bad Name")], {"Studies/Bad Name/y.pdf": "y"},
            laptop_dirs={"Studies/Bad Name"}, blocked={"Studies/Bad Name"})
    assert not p.down and not p.up and len(p.waiting_folder) == 2
    many = {f"{FOLDER}/f{i}.pdf": f"s{i}" for i in range(35)}
    p = run([], many, limit=30)
    assert len(p.up) == 30 and p.held_over == 5


def test_push_skips_bytes_already_there_and_waits_out_429(tmp_path):
    data = b"%PDF-1.4 x"
    sha = hashlib.sha256(data).hexdigest()
    (tmp_path / f"{sha}.pdf").write_bytes(data)
    calls, slept = [], []

    def deliver(pdf, folder):
        calls.append((pdf.name, folder))
        if len(calls) == 1:
            raise RuntimeError("HTTP 429 too many requests")

    items = [{"sha": sha, "folder": FOLDER, "title": "my-summary"},
             {"sha": "f" * 64, "folder": FOLDER, "title": "there"},
             {"sha": "e" * 64, "folder": "Personal/x", "title": "no"}]
    got = fc.push(items, stage_dir=tmp_path, deliver=deliver, hashes={"f" * 64}, sleep=slept.append)
    assert got[sha] == "sent" and got["f" * 64] == "sent" and got["e" * 64].startswith("error: refused")
    assert calls == [("my-summary.pdf", FOLDER)] * 2 and 60.0 in slept


def test_clean_bytes_prefer_the_library_copy_and_check_the_hash(tmp_path):
    import sqlite3
    import zipfile

    data = b"%PDF clean"
    sha = hashlib.sha256(data).hexdigest()
    db = tmp_path / "l.db"
    c = sqlite3.connect(db)
    c.execute("CREATE TABLE documents (content_hash TEXT, raw_path TEXT)")
    c.execute("INSERT INTO documents VALUES (?, ?)", (sha, "r.pdf"))
    c.commit()
    c.close()
    (tmp_path / "r.pdf").write_bytes(data)
    assert fc.clean_bytes(sha, raw_store=tmp_path, db=db, cache=tmp_path / "none", fetch=None) == data
    # not in the library: from a fetched bundle, ink and all, only the PDF comes out
    other = b"%PDF other"
    b = tmp_path / "b.rmdoc"
    with zipfile.ZipFile(b, "w") as z:
        z.writestr("u.pdf", other)
        z.writestr("u/1.rm", b"ink")
    assert fc.clean_bytes(hashlib.sha256(other).hexdigest(), raw_store=tmp_path, db=db,
                          cache=tmp_path / "none", fetch=lambda: b) == other
