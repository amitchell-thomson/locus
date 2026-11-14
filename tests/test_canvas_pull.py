"""The Canvas mirror (P2): names, placement, and a second run fetching nothing."""

from __future__ import annotations

import hashlib

import pytest

from locus import canvas_pull
from locus.canvas import CanvasError


class FakeCanvas:
    """Modules and file metadata as Canvas returns them (shapes from the real account, 14 Nov)."""

    token = "t"

    def __init__(self):
        self.files = {
            1: {"display_name": "B18 Lecture 1.pdf", "content-type": "application/pdf", "size": 3,
                "updated_at": "2026-10-27T09:19:07Z", "url": "u1"},
            2: {"display_name": "hhmodel.m", "content-type": "text/x-matlab", "size": 3,
                "updated_at": "2026-10-28T09:00:00Z", "url": "u2"},
            3: {"display_name": "Lab demo.mp4", "content-type": "video/mp4", "size": 3,
                "updated_at": "2026-10-28T09:00:00Z", "url": "u3"},
            4: {"display_name": "Lab sheet.pdf", "content-type": "application/pdf", "size": 3,
                "updated_at": "2026-10-28T09:00:00Z", "url": "u4"},
        }
        self.calls: list[str] = []

    def get(self, path, params=None):
        self.calls.append(path)
        if path.endswith("/modules"):
            return [
                {"name": "Cellular Physiology ", "items": [
                    {"type": "SubHeader", "title": "Lectures"},
                    {"type": "File", "title": "B18 Lecture 1.pdf", "content_id": 1},
                    {"type": "File", "title": "hhmodel.m", "content_id": 2},
                    {"type": "ExternalUrl", "title": "Playlist", "external_url": "https://panopto/x"},
                ]},
                {"name": "Wearable Technology Laboratory ", "items": [
                    {"type": "File", "title": "demo", "content_id": 3},
                    {"type": "Page", "title": "Files needed", "page_url": "files-needed"},
                    {"type": "File", "title": "again", "content_id": 1},
                ]},
            ]
        if "/pages/" in path:
            return {"body": '<a href="/courses/9/files/4/download">sheet</a> <a href="/courses/9/files/4">x</a>'}
        fid = int(path.rsplit("/", 1)[1])
        return dict(self.files[fid])


@pytest.fixture
def fetched(monkeypatch):
    got: list[str] = []

    def fake_download(client, url, dest):
        got.append(url)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(b"abc")
        return hashlib.sha256(b"abc").hexdigest()

    monkeypatch.setattr(canvas_pull, "_download", fake_download)
    return got


COURSES = {"b18": {"canvas": 9, "folder": "b18-biomedical", "active": True},
           "b99": {"name": "not mapped"}}


def test_file_names_are_f1_with_the_extension_kept():
    assert canvas_pull.file_name("B18 Lecture 1.pdf") == "b18-lecture-1.pdf"
    assert canvas_pull.file_name("B14_SP_MT26_L1.PDF") == "b14-sp-mt26-l1.pdf"
    assert canvas_pull.file_name("Notes (docx version)") == "notes-docx-version"


def test_the_pull_places_files_by_module_and_a_second_run_fetches_nothing(tmp_path, fetched):
    fake = FakeCanvas()
    rep = canvas_pull.pull(fake, COURSES, root=tmp_path)
    assert sorted(rep.new) == ["b18/cellular-physiology/b18-lecture-1.pdf",
                               "b18/cellular-physiology/hhmodel.m",
                               "b18/wearable-technology-laboratory/lab-sheet.pdf"]
    assert rep.skipped == ["b18/wearable-technology-laboratory/lab-demo.mp4 (skipped: video)"]
    assert (tmp_path / "b18-biomedical/cellular-physiology/b18-lecture-1.pdf").read_bytes() == b"abc"
    man = canvas_pull.load_manifest(tmp_path)
    one = man["files"]["1"]
    assert (one["module"], one["subheader"], one["sha256"]) == (
        "Cellular Physiology", "Lectures", hashlib.sha256(b"abc").hexdigest())
    assert man["files"]["4"]["via"] == "page: Files needed"
    assert man["links"]["b18"] == [{"module": "Cellular Physiology", "title": "Playlist",
                                    "url": "https://panopto/x"}]
    fetched.clear()
    rep2 = canvas_pull.pull(fake, COURSES, root=tmp_path)
    assert (rep2.new, rep2.changed, rep2.skipped, fetched) == ([], [], [], [])
    assert rep2.unchanged == 3


def test_a_file_changed_on_canvas_is_fetched_again_in_place(tmp_path, fetched):
    fake = FakeCanvas()
    canvas_pull.pull(fake, COURSES, root=tmp_path)
    fake.files[1].update(updated_at="2026-11-01T00:00:00Z", display_name="B18 Lecture 1 (revised).pdf")
    rep = canvas_pull.pull(fake, COURSES, root=tmp_path)
    assert rep.changed == ["b18/cellular-physiology/b18-lecture-1.pdf"]


def test_a_past_course_is_not_asked_about_files_it_already_has(tmp_path, fetched):
    fake = FakeCanvas()
    past = {"p1": {"canvas": 9, "folder": "p1-maths", "active": False}}
    canvas_pull.pull(fake, past, root=tmp_path)
    fake.calls.clear()
    canvas_pull.pull(fake, past, root=tmp_path)
    assert not [c for c in fake.calls if "/files/" in c and "/pages/" not in c]


def test_a_dry_run_writes_nothing(tmp_path, fetched):
    rep = canvas_pull.pull(FakeCanvas(), COURSES, root=tmp_path, dry_run=True)
    assert len(rep.new) == 3 and not fetched and not (tmp_path / "manifest.json").exists()


def test_a_course_that_fails_is_an_error_and_the_rest_continue(tmp_path, fetched):
    class Broken(FakeCanvas):
        def get(self, path, params=None):
            if path.startswith("courses/8/"):
                raise CanvasError("GET x: HTTP 403")
            return super().get(path, params)

    rep = canvas_pull.pull(Broken(), {"a": {"canvas": 8, "folder": "a"}, **COURSES}, root=tmp_path)
    assert rep.errors == ["a: GET x: HTTP 403"] and len(rep.new) == 3
