"""Which stored category a drop gets from its folder (ADR-003: area folders, 2026-10-19)."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

from locus import ingest_pipeline as ip


@pytest.fixture()
def incoming(tmp_path: Path, monkeypatch) -> Path:
    d = tmp_path / "incoming"
    d.mkdir()
    monkeypatch.setattr(ip, "load", lambda: SimpleNamespace(paths=SimpleNamespace(incoming=d)))
    return d


@pytest.mark.parametrize(
    "rel, category",
    [
        # The area folders map onto the categories the library still keys behaviour on.
        ("studies/control/lecture8.pdf", "coursework"),
        ("projects/nabla/brief.pdf", "project"),
        ("careers/general/cv.pdf", "career"),
        ("personal/recipe.pdf", "personal"),  # not `note`: that would make it his writing
        ("Studies/control/x.pdf", "coursework"),        # case-insensitive, as before
        # The old category folders keep working.
        ("papers/x.pdf", "paper"),
        ("coursework/x.pdf", "coursework"),
        # The root is "let the system decide": nothing decides yet.
        ("x.pdf", "uncategorized"),
    ],
)
def test_drop_folder_decides_the_category(incoming: Path, rel: str, category: str) -> None:
    assert ip._category(incoming / rel) == category


@pytest.mark.parametrize(
    "uri, area_drop",
    [
        ("data/incoming/studies/control/x.pdf", True),
        ("/home/alec/locus/data/incoming/Projects/brief.md", True),
        ("data/incoming/personal/tenancy.docx", True),
        ("data/incoming/career/cv.docx", False),     # the old category folder, unchanged
        ("data/notes/idea.md", False),
        ("repos/regime-ml", False),
    ],
)
def test_area_drops_are_recognised_by_path(uri: str, area_drop: bool) -> None:
    assert ip.is_area_drop(uri) is area_drop
