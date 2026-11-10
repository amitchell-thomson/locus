"""markups finds a document by its name however it is spelled (10 Nov: a chat asked for
"B15 Sheet 1" eleven ways and never reached the notebook the tablet calls `B15_sheet1`)."""

from __future__ import annotations

from locus.capture import review


def test_a_name_matches_however_it_is_spelled(monkeypatch):
    paths = [
        "/Studies/b15-control-systems/linear-dynamic-systems/B15_sheet1",
        "/Studies/b15-control-systems/linear-dynamic-systems/B15_lecture_notes",
        "/Careers/jump-trading/daily-sheets/2026-09-23 Jump sheet 1 Wed 23 Sep",
    ]
    monkeypatch.setattr(review, "device_file_paths", lambda runner: paths)
    for ask in ["B15 Sheet 1", "b15-sheet-1", "B15_sheet1", "b15sheet1"]:
        assert review.find_on_device(None, ask) == [paths[0]], ask
    assert review.name_key("B15 Sheet 1") == review.name_key("B15_sheet1")


def test_nothing_matching_offers_the_nearest_names():
    paths = ["/Studies/b15/B15_sheet1", "/Studies/b15/B15_lecture_notes", "/Personal/recipes"]
    near = review.nearest_names(paths, "B15 problem sheet 1 question 5")
    assert near[0] == "/Studies/b15/B15_sheet1" and "/Personal/recipes" not in near
