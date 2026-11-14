"""The Canvas client is read only, and the registry mapping is what P1 proposes (spec P)."""

from __future__ import annotations

import io
import json
import tomllib

import pytest

from locus import canvas


class _Resp(io.BytesIO):
    def __init__(self, body, link=""):
        super().__init__(json.dumps(body).encode())
        self.headers = {"Link": link}


def test_only_get_is_possible():
    c = canvas.Client(token="t", base="https://x", opener=lambda *a, **k: None)
    with pytest.raises(canvas.CanvasError, match="read only"):
        c._request("POST", "https://x/api/v1/courses/1/assignments/2/submissions")
    assert not any(hasattr(c, m) for m in ("post", "put", "delete", "patch"))


def test_pages_are_followed_and_the_token_is_sent():
    seen = []

    def opener(req, timeout):
        seen.append((req.get_method(), req.full_url, req.headers["Authorization"]))
        if "page=2" in req.full_url:
            return _Resp([{"id": 2}])
        return _Resp([{"id": 1}], '<https://x/api/v1/courses?page=2>; rel="next"')

    out = canvas.Client(token="tok", base="https://x", opener=opener).get("courses")
    assert out == [{"id": 1}, {"id": 2}]
    assert all(m == "GET" and a == "Bearer tok" for m, _, a in seen) and len(seen) == 2


REG = """[[projects]]
id = "b18"
name = "B18 Biomedical Modelling and Monitoring"
folder = "b18-biomedical-modelling-and-monitoring"
area = "Studies"
kind = "course"
active = true

[[projects]]
id = "b16"
name = "B16 Software Engineering Systems"
area = "Studies"
kind = "course"
active = true

# ── application ──
[[projects]]
id = "x"
name = "X"
area = "Careers"
kind = "application"
active = true
"""

COURSES = [
    canvas.Course(298821, "B18 Biomedical Modelling and Monitoring", "2026-27 Academic Year"),
    canvas.Course(298819, "B16 Software Engineering", "2026-27 Academic Year"),
    canvas.Course(298809, "B5 Solid Mechanics", "2026-27 Academic Year"),
    canvas.Course(294649, "A1. Mathematics", "2025-26 Academic Year"),
    canvas.Course(299246, "MEng Engineering Science", "2026-27 Academic Year"),
]


def _entries(text):
    return {p["id"]: p for p in tomllib.loads(text)["projects"]}


def test_the_mapping_matches_by_paper_code_and_leaves_papers_not_taken():
    m = canvas.plan(_entries(REG), COURSES)
    assert {(r, c.id) for r, c in m.mapped} == {("b18", 298821), ("b16", 298819)}
    assert [c.id for c in m.new] == [294649]
    assert {c.id for c in m.unmatched} == {298809, 299246}
    assert [(r, c.name) for r, _, c in m.renamed] == [("b16", "B16 Software Engineering")]


def test_applying_writes_canvas_ids_and_new_entries_and_is_idempotent():
    m = canvas.plan(_entries(REG), COURSES)
    once = canvas.apply_to_registry(REG, m)
    e = _entries(once)
    assert e["b18"]["canvas"] == 298821 and e["b16"]["canvas"] == 298819
    assert e["b16"]["name"] == "B16 Software Engineering"
    assert e["a1"] == {"id": "a1", "name": "A1. Mathematics", "folder": "a1-mathematics", "area": "Studies",
                       "kind": "course", "canvas": 294649, "active": False}
    assert once.index('id = "a1"') < once.index("# ── application")
    m2 = canvas.plan(_entries(once), COURSES)
    assert not m2.new and canvas.apply_to_registry(once, m2) == once
