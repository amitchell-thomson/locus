"""P4: to the tablet, only what is not there, never solutions, never twice."""

from __future__ import annotations

import hashlib
import json

from locus import canvas_tablet

COURSES = {"b14": {"canvas": 1, "folder": "b14-ies", "active": True},
           "p1": {"canvas": 2, "folder": "p1-maths", "active": False}}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def mirror(tmp_path, files):
    root = tmp_path / "canvas"
    man = {"files": {}, "links": {}}
    for i, (rel, (course, name, data)) in enumerate(files.items()):
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_bytes(data)
        man["files"][str(i)] = {"id": i, "course": course, "name": name, "path": rel, "status": "ok",
                                "sha256": sha(data)}
    (root / "manifest.json").write_text(json.dumps(man))
    return root, man


FILES = {
    "b14-ies/image-processing/b14-ip-mt26-l2.pdf": ("b14", "B14_IP_MT26_L2.pdf", b"L2"),   # his hand upload
    "b14-ies/image-processing/b14-ip-mt26-l3.pdf": ("b14", "B14_IP_MT26_L3.pdf", b"L3"),
    "b14-ies/image-processing/b14-ip-selectedanswers.pdf": ("b14", "B14_IP_SelectedAnswers.pdf", b"A"),
    "b14-ies/signal-analysis/handel.wav": ("b14", "handel.wav", b"wav"),
    "b14-ies/signal-analysis/copy-of-l3.pdf": ("b14", "copy of L3.pdf", b"L3"),
    "p1-maths/calculus/old.pdf": ("p1", "old.pdf", b"old"),
}


def test_only_what_is_not_on_the_tablet_goes_and_solutions_never(tmp_path):
    root, man = mirror(tmp_path, FILES)
    p = canvas_tablet.plan(man, COURSES, {sha(b"L2")}, skip={})
    assert [(s.folder, s.title) for s in p.sends] == [("Studies/b14-ies/image-processing", "B14_IP_MT26_L3")]
    assert p.solutions == ["b14-ies/image-processing/b14-ip-selectedanswers.pdf"]
    # His copy, and the same bytes a second time on Canvas: neither sent.
    assert sorted(p.on_tablet) == ["b14-ies/image-processing/b14-ip-mt26-l2.pdf",
                                   "b14-ies/signal-analysis/copy-of-l3.pdf"]


def test_a_sent_file_is_never_sent_again_even_after_he_deletes_it(tmp_path):
    root, man = mirror(tmp_path, FILES)
    got = []
    p = canvas_tablet.plan(man, COURSES, {sha(b"L2")}, skip={})
    sent, errors = canvas_tablet.send(p, deliver=lambda pdf, folder: got.append((pdf.name, folder)),
                                      root=root, pause=0, log=lambda *_: None)
    assert (sent, errors, got) == (1, [], [("B14_IP_MT26_L3.pdf", "Studies/b14-ies/image-processing")])
    man2 = json.loads((root / "manifest.json").read_text())
    p2 = canvas_tablet.plan(man2, COURSES, set(), skip={})    # gone from the tablet
    assert not [s for s in p2.sends if s.src.endswith("l3.pdf")] and p2.sent_before == 2


def test_a_name_taken_by_other_bytes_lands_beside_it_and_429s_back_off(tmp_path):
    root, man = mirror(tmp_path, FILES)
    p = canvas_tablet.plan(man, COURSES, {sha(b"L2")}, skip={})
    calls, slept = [], []

    def deliver(pdf, folder):
        calls.append(pdf.name)
        if len(calls) == 1:
            raise RuntimeError("rmapi put failed: HTTP 429 Too Many Requests")
        if pdf.name == "B14_IP_MT26_L3.pdf":
            raise RuntimeError("rmapi put failed: entry already exists")

    sent, errors = canvas_tablet.send(p, deliver=deliver, root=root, pause=0, log=lambda *_: None,
                                      sleep=slept.append)
    assert (sent, errors) == (1, []) and slept == [60.0]
    assert calls[-1] == "B14_IP_MT26_L3 (v2).pdf"


def test_titles_are_canvas_names_without_the_extension():
    assert canvas_tablet.title_of("B18 Lecture 1.pdf") == "B18 Lecture 1"
    assert canvas_tablet.title_of("Notes (docx version)") == "Notes (docx version)"


def test_his_copy_of_an_earlier_version_by_name_is_not_duplicated(tmp_path):
    root, man = mirror(tmp_path, FILES)
    names = {canvas_tablet.name_key("Studies/b14-ies/image-processing", "B14 IP MT26 L3.pdf")}
    p = canvas_tablet.plan(man, COURSES, {sha(b"L2")}, skip={}, tablet_names=names)
    assert p.other_version == ["b14-ies/image-processing/b14-ip-mt26-l3.pdf"]
    assert "b14-ies/image-processing/b14-ip-mt26-l3.pdf" not in [s.src for s in p.sends]
