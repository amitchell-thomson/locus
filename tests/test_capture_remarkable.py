"""Loop A identify layer (agent-layer §8.1, Phase 1 ④): map a staged <uuid>.pdf to its
reMarkable name/folder/category. The rmapi cloud is faked (find + stat), so this is offline."""

from __future__ import annotations

from pathlib import Path

from locus.capture.remarkable import CaptureItem, build_uuid_index, identify_staged

# A fake reMarkable tree: find output + stat metadata per path.
_FIND = "\n".join([
    "[d] /",
    "[d] /brevan_howard",
    "[f] /brevan_howard/Jargon Sheet",
    "[d] /projects",
    "[f] /projects/Tanker Flow Ideas",
    "[d] /rough_notes",
    "[f] /rough_notes/Random thought",
    "[d] /trash",
    "[f] /trash/Old scribble",
])
_STAT = {
    "/brevan_howard/Jargon Sheet": {"ID": "uuid-jargon", "Name": "Jargon Sheet", "Type": "DocumentType",
                                    "ModifiedClient": "2026-07-10T13:13:56Z"},
    "/projects/Tanker Flow Ideas": {"ID": "uuid-tanker", "Name": "Tanker Flow Ideas"},
    "/rough_notes/Random thought": {"ID": "uuid-rough", "Name": "Random thought"},
    "/trash/Old scribble": {"ID": "uuid-trash", "Name": "Old scribble"},
}


def _fake_runner(calls: list | None = None):
    def run(args):
        if calls is not None:
            calls.append(args)
        if args[:2] == ["find", "/"]:
            return 0, _FIND, ""
        if args[0] == "stat":
            meta = _STAT.get(args[1])
            import json
            return (0, json.dumps(meta), "") if meta else (1, "", "not found")
        return 1, "", "unexpected"
    return run


def test_build_uuid_index_maps_and_skips_excluded():
    idx = build_uuid_index(_fake_runner(), excluded_folders=("trash",))
    # ModifiedClient rides along: it is the only honest date for handwriting (the device's own
    # last-edited stamp). A document that reports none yields None, never a guessed date.
    assert idx["uuid-jargon"] == ("Jargon Sheet", "brevan_howard", "2026-07-10T13:13:56Z")
    assert idx["uuid-tanker"] == ("Tanker Flow Ideas", "projects", None)
    assert "uuid-trash" not in idx  # trash folder excluded


def test_index_does_not_stat_excluded_folders():
    calls: list = []
    build_uuid_index(_fake_runner(calls), excluded_folders=("trash",))
    stat_paths = [a[1] for a in calls if a[0] == "stat"]
    assert "/trash/Old scribble" not in stat_paths  # excluded folders never stat'd


def test_identify_staged_assigns_categories(tmp_path: Path):
    staging = tmp_path / "stage"
    staging.mkdir()
    for u in ("uuid-jargon", "uuid-tanker", "uuid-rough", "uuid-orphan"):
        (staging / f"{u}.pdf").write_bytes(b"%PDF-1.7")

    items, unmapped = identify_staged(staging, runner=_fake_runner(), excluded_folders=("trash",))

    by_uuid = {i.uuid: i for i in items}
    # Category is a coarse KIND facet: handwriting defaults to note; the folder is kept as
    # provenance (item.folder), not mirrored into a taxonomy. Only genuine-KIND folders override.
    assert by_uuid["uuid-jargon"].category == "note"        # brevan_howard -> note (default)
    assert by_uuid["uuid-jargon"].folder == "brevan_howard"  # folder preserved as provenance
    assert by_uuid["uuid-jargon"].name == "Jargon Sheet"
    assert by_uuid["uuid-tanker"].category == "project"     # projects -> project (KIND exception)
    assert by_uuid["uuid-rough"].category == "note"         # rough_notes -> note (default)
    assert unmapped == ["uuid-orphan"]                       # no cloud match -> reported, not guessed


def test_folder_category_override(tmp_path: Path):
    staging = tmp_path / "stage"
    staging.mkdir()
    (staging / "uuid-tanker.pdf").write_bytes(b"%PDF")
    items, _ = identify_staged(
        staging, runner=_fake_runner(), folder_category={"projects": "paper"},
    )
    assert items[0].category == "paper"  # override wins over the built-in 'project'


def test_default_category_for_unmapped_folder(tmp_path: Path):
    # A doc in a folder with no mapping falls to default_category, not a crash.
    find = "[f] /misc/Loose note"
    def runner(args):
        import json
        if args[:2] == ["find", "/"]:
            return 0, find, ""
        return 0, json.dumps({"ID": "uuid-misc", "Name": "Loose note"}), ""
    staging = tmp_path / "s"; staging.mkdir()
    (staging / "uuid-misc.pdf").write_bytes(b"%PDF")
    items, _ = identify_staged(staging, runner=runner, default_category="note")
    assert items[0].category == "note" and items[0].folder == "misc"


def test_identify_carries_the_device_modified_date(tmp_path):
    """The date the owner last WROTE, not the date capture ran — belief_positions.dated_at
    ultimately comes from here, and dating handwriting by capture time flattens the trajectory."""
    (tmp_path / "uuid-jargon.pdf").write_bytes(b"%PDF-1.4\n")
    items, _ = identify_staged(tmp_path, runner=_fake_runner(), excluded_folders=("trash",))
    assert [i.modified for i in items] == ["2026-07-10T13:13:56Z"]


# --- the 2026-08 device reorganisation: category keys BELOW the notes root --------------------

_MIGRATED_FIND = "\n".join([
    "[d] /Notes",
    "[d] /Notes/engineering",
    "[f] /Notes/engineering/B14 Vibrations",
    "[d] /Notes/quantum_ml",
    "[f] /Notes/quantum_ml/week 3 notes",
    "[d] /Notes/careers",
    "[f] /Notes/careers/CV thoughts",
    "[d] /Reading/In-Progress",
    "[f] /Reading/In-Progress/Advanced Portfolio Management",
    "[d] /Daily",
    "[f] /Daily/2026-08-02",
])
_MIGRATED_STAT = {
    "/Notes/engineering/B14 Vibrations": {"ID": "u-eng", "Name": "B14 Vibrations"},
    "/Notes/quantum_ml/week 3 notes": {"ID": "u-qml", "Name": "week 3 notes"},
    "/Notes/careers/CV thoughts": {"ID": "u-car", "Name": "CV thoughts"},
    "/Reading/In-Progress/Advanced Portfolio Management": {"ID": "u-book", "Name": "APM"},
    "/Daily/2026-08-02": {"ID": "u-daily", "Name": "2026-08-02"},
}


def _migrated_runner():
    def run(args):
        if args[:2] == ["find", "/"]:
            return 0, _MIGRATED_FIND, ""
        if args[0] == "stat":
            meta = _MIGRATED_STAT.get(args[1])
            import json
            return (0, json.dumps(meta), "") if meta else (1, "", "not found")
        return 1, "", "unexpected"
    return run


def test_topic_folder_keys_below_the_notes_root():
    """After the move every note's TOP folder is `Notes`, so keying on the top level would
    collapse every category to the default — silently, since a wrong category is not an error."""
    from locus.capture.remarkable import topic_folder

    assert topic_folder("/Notes/engineering/B14 Vibrations") == "engineering"
    assert topic_folder("/Notes/projects/oqts/design") == "projects"
    # unmigrated device: unchanged behaviour, which is what lets capture keep running during the move
    assert topic_folder("/engineering/B14 Vibrations") == "engineering"
    # a document dropped directly in /Notes has no topic, so it takes default_category
    assert topic_folder("/Notes/loose doc") == "Notes"


def test_categories_survive_the_reorganisation(tmp_path: Path):
    for uuid in ("u-eng", "u-qml", "u-car"):
        (tmp_path / f"{uuid}.pdf").write_bytes(b"%PDF-1.4")
    items, unmapped = identify_staged(tmp_path, runner=_migrated_runner())
    by_uuid = {i.uuid: i for i in items}
    assert by_uuid["u-eng"].category == "coursework"   # lecture annotations stay study material
    assert by_uuid["u-qml"].category == "note"         # research internship, not coursework
    assert by_uuid["u-car"].category == "career"       # was silently falling through to `note`
    assert not unmapped


def test_locus_owned_folders_are_never_captured_as_handwriting(tmp_path: Path):
    """Invariant 5: ingesting our own delivered pages as though they were his writing."""
    for uuid in ("u-book", "u-daily"):
        (tmp_path / f"{uuid}.pdf").write_bytes(b"%PDF-1.4")
    items, unmapped = identify_staged(tmp_path, runner=_migrated_runner())
    assert items == []
    assert sorted(unmapped) == ["u-book", "u-daily"]
