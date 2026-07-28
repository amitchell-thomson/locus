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
    "/brevan_howard/Jargon Sheet": {"ID": "uuid-jargon", "Name": "Jargon Sheet", "Type": "DocumentType"},
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
    assert idx["uuid-jargon"] == ("Jargon Sheet", "brevan_howard")
    assert idx["uuid-tanker"] == ("Tanker Flow Ideas", "projects")
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
