"""The tablet read from rmapi's tree cache, and what each document on it is (Slice 0.12).

These replace the tests of the old staging identification (`identify_staged`), keeping what
those protected: categories follow where his writing lives, Locus's own delivery targets are
never read as his handwriting, and the date is the device's. Added: the kind of document decides
the reading, folder levels match the registry, and an unreadable cache fails rather than looking
like an empty tablet.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from locus.capture.device import (
    Placement, Registry, load_registry, parse_tree, place, read_device, unmatched_folders,
)


def _folder(uuid, name, parent=""):
    return {"DocumentID": uuid, "Files": [],
            "Metadata": {"visibleName": name, "type": "CollectionType", "parent": parent}}


def _doc(uuid, name, parent="", *, pdf=None, rm=(), meta_hash="m1", modified="1792301621657",
         deleted=False):
    files = [{"DocumentID": f"{uuid}.metadata", "Hash": meta_hash},
             {"DocumentID": f"{uuid}.content", "Hash": "c1"}]
    if pdf:
        files.append({"DocumentID": f"{uuid}.pdf", "Hash": pdf})
    files += [{"DocumentID": f"{uuid}/{i}.rm", "Hash": h} for i, h in enumerate(rm)]
    return {"DocumentID": uuid, "Files": files,
            "Metadata": {"visibleName": name, "type": "DocumentType", "parent": parent,
                         "lastModified": modified, "deleted": deleted}}


TREE = {"Docs": [
    _folder("f-stu", "Studies"),
    _folder("f-b14", "B14 Information Engineering Systems", "f-stu"),
    _folder("f-car", "Careers"),
    _folder("f-bh", "Brevan Howard", "f-car"),
    _folder("f-adm", "Admin"),
    _folder("f-arc", "Archive"),
    _folder("f-rd", "Reading"),
    _folder("f-inb", "Inbox"),
    _doc("jot", "Jot", rm=("r1",)),
    _doc("ip", "B14 Image Processing", "f-b14", rm=("r1", "r2")),
    _doc("l2", "B14_IP_MT26_L2.pdf", "f-b14", pdf="hash-l2", rm=("r3",)),
    _doc("swaps", "Swaps", "f-bh", rm=("r4",)),
    _doc("nda", "NDA", "f-adm", pdf="hash-nda"),
    _doc("daily", "daily-2026-10-18", "f-arc", pdf="hash-daily", rm=("r5",)),
    _doc("apm", "Advanced Portfolio Management", "f-rd", pdf="hash-apm"),
    _doc("plan", "2026-10-19 Locus plan v3", "f-inb", pdf="hash-plan", rm=("r6",)),
    _doc("gone", "Old", "trash", rm=("r7",)),
    _doc("del", "Deleted", "f-stu", rm=("r8",), deleted=True),
]}


def _by_name(docs):
    return {d.name: d for d in docs}


def test_parse_tree_builds_paths_and_kinds():
    docs = _by_name(parse_tree(TREE))
    assert docs["B14 Image Processing"].path == (
        "/Studies/B14 Information Engineering Systems/B14 Image Processing")
    assert docs["Jot"].folders == () and docs["Jot"].is_notebook
    assert not docs["B14_IP_MT26_L2.pdf"].is_notebook
    assert docs["B14_IP_MT26_L2.pdf"].pdf_hash == "hash-l2"
    assert docs["B14 Image Processing"].ink_pages == 2
    assert docs["Swaps"].modified.startswith("2026-")      # the device's date, ISO
    assert "Old" not in docs and "Deleted" not in docs      # trashed and deleted are not there


def test_the_ink_key_ignores_a_move_and_follows_the_ink():
    """Moving or opening a notebook rewrites its .metadata; re-billing a transcription for that
    would be paying for nothing. A new stroke layer must move the key."""
    base = parse_tree({"Docs": [_doc("n", "N", rm=("a",), meta_hash="m1")]})[0]
    moved = parse_tree({"Docs": [_doc("n", "N", rm=("a",), meta_hash="m2")]})[0]
    inked = parse_tree({"Docs": [_doc("n", "N", rm=("a", "b"), meta_hash="m1")]})[0]
    assert base.ink_key == moved.ink_key
    assert base.ink_key != inked.ink_key


def test_an_unreadable_cache_fails_rather_than_reading_as_an_empty_tablet(tmp_path):
    with pytest.raises(ValueError):
        parse_tree({"Hash": "x"})
    with pytest.raises(ValueError):
        parse_tree({"Docs": [{"no": "id"}]})

    def ok(args):
        return 0, "", ""

    with pytest.raises(RuntimeError, match="tree cache"):
        read_device(ok, tree_cache=tmp_path / "missing.cache")
    with pytest.raises(RuntimeError, match="429"):
        read_device(lambda args: (1, "", "request failed with status 429"),
                    tree_cache=tmp_path / "missing.cache")


def test_read_device_makes_one_cloud_call(tmp_path):
    calls: list = []
    cache = tmp_path / "tree.cache"
    cache.write_text(json.dumps(TREE))

    def runner(args):
        calls.append(args)
        return 0, "", ""

    docs = read_device(runner, tree_cache=cache)
    assert calls == [["ls", "/"]]
    assert len(docs) == 8                     # ten documents, one trashed, one deleted


REG = Registry({
    ("Studies", "b14 information engineering systems"): "b14",
    ("Careers", "brevan howard"): "brevan-howard-2026",
})


def _placements(sent=frozenset({"hash-plan"})):
    return {d.name: place(d, sent_hashes=set(sent), registry=REG) for d in parse_tree(TREE)}


def test_what_a_document_is_decides_how_it_is_read():
    p = _placements()
    assert p["Jot"] == Placement("notebook")
    assert p["B14 Image Processing"] == Placement(
        "notebook", area="Studies", thing="b14",
        thing_folder="B14 Information Engineering Systems")
    assert p["B14_IP_MT26_L2.pdf"].role == "source"
    assert p["Swaps"].category == "career"
    assert p["B14 Image Processing"].category == "coursework"
    assert p["2026-10-19 Locus plan v3"].role == "sent"
    assert p["Advanced Portfolio Management"].role == "reading"


def test_locus_own_and_paperwork_folders_are_never_read():
    """Invariant 5 (Locus's own pages are not his writing) and the paperwork in Admin: not read
    at all, by construction, rather than by an exclusion list someone has to keep current."""
    p = _placements()
    assert p["daily-2026-10-18"] is None      # Archive
    assert p["NDA"] is None                   # Admin


def test_a_pdf_in_the_inbox_locus_did_not_send_is_left_alone():
    assert _placements(sent=frozenset())["2026-10-19 Locus plan v3"] is None


def test_area_folders_match_case_insensitively():
    tree = {"Docs": [_folder("s", "studies"), _doc("n", "Note", "s", rm=("a",))]}
    (doc,) = parse_tree(tree)
    assert place(doc, sent_hashes=set(), registry=REG).area == "Studies"


def test_an_unregistered_level_two_folder_keeps_its_area_and_is_reported():
    tree = {"Docs": [_folder("s", "Studies"), _folder("t", "B99 Typo", "s"),
                     _doc("n", "Note", "t", rm=("a",))]}
    (doc,) = parse_tree(tree)
    where = place(doc, sent_hashes=set(), registry=REG)
    assert where.area == "Studies" and where.thing is None
    assert unmatched_folders([(doc, where)]) == ["Studies/B99 Typo"]


def test_the_real_registry_matches_the_tablet_folders():
    """Every level-2 folder on his tablet on 2026-10-27, against the committed registry. Careers
    folders are per firm and match an application's company."""
    reg = load_registry()
    for area, folder in [
        ("Studies", "B14 Information Engineering Systems"), ("Studies", "B3 Group Design Project"),
        ("Projects", "quast"), ("Projects", "oqts"), ("Projects", "locus"),
        ("Projects", "Tanker-flow"), ("Careers", "Brevan Howard"), ("Careers", "Jump Trading"),
        ("Careers", "G-Research"), ("Careers", "Millennium"), ("Careers", "Jane Street"),
    ]:
        assert reg.match(area, folder), f"{area}/{folder} is not in taxonomy/registry.toml"


def test_a_missing_registry_is_empty_not_an_error(tmp_path):
    assert load_registry(tmp_path / "nope.toml").match("Studies", "B1") is None


def test_root_pdf_has_no_home():
    (doc,) = parse_tree({"Docs": [_doc("p", "loose.pdf", pdf="h")]})
    assert place(doc, sent_hashes=set(), registry=REG) is None


def test_tree_paths_are_built_from_the_cache_not_assumed():
    """A folder renamed on the tablet renames every path under it on the next read."""
    tree = json.loads(json.dumps(TREE))
    for d in tree["Docs"]:
        if d["DocumentID"] == "f-b14":
            d["Metadata"]["visibleName"] = "B14 IES"
    docs = _by_name(parse_tree(tree))
    assert docs["B14 Image Processing"].path == "/Studies/B14 IES/B14 Image Processing"
    assert Path(docs["B14 Image Processing"].path).parent.name == "B14 IES"
