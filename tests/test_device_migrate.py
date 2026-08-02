"""Device reorganisation: planning, review gating, snapshot gating, and the moves themselves.

Model-free and device-free (CLAUDE.md §14) — both rmapi runners are injected, so every path here
runs against a fake tree. The cases that matter are the ones where a wrong answer is SILENT: a
mixed folder planned without review, a collision that would half-finish the migration, and a
plan that runs before anything has been backed up.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from locus.capture import device_migrate as DM

# The tree as it actually stood on 2026-08-01, trimmed to one document per folder.
DEVICE_TREE = """\
[d] /Locus
[d] /Locus/Reading
[d] /Locus/Reading/Proposed
[f] /Locus/Reading/Proposed/2026-07-31 Covariance Shrinkage
[d] /Locus/Reading/In-Progress
[f] /Locus/Reading/In-Progress/2026-07-28 Regime Shifts
[f] /Locus/daily-2026-08-01
[f] /Locus/daily-2026-07-30
[f] /Locus/pour-runbook
[d] /reading_list
[f] /reading_list/Advanced Portfolio Management
[f] /reading_list/reading list from Tom
[d] /engineering
[f] /engineering/B14 Vibrations
[d] /quantum_ml
[f] /quantum_ml/week 3 notes
[d] /brevan_howard
[f] /brevan_howard/speaker notes
[d] /projects
[d] /projects/oqts
[f] /projects/oqts/design
[d] /careers
[f] /careers/CV thoughts
[d] /admin
[f] /admin/tenancy agreement
[d] /trash
[f] /trash/reading_list/old book
"""


def fake_runner(listing: str = DEVICE_TREE, *, calls: list | None = None):
    def run(args: list[str]) -> tuple[int, str, str]:
        if calls is not None:
            calls.append(args)
        if args[0] == "find":
            return 0, listing, ""
        if args[0] in ("mkdir", "mv"):
            return 0, "", ""
        return 1, "", f"unexpected {args!r}"

    return run


def planned() -> dict[str, DM.Move]:
    moves = DM.plan(DM.list_device(fake_runner()))
    return {m.src: m for m in moves}


# --- planning ---------------------------------------------------------------------------------


def test_reading_tree_moves_out_from_under_locus():
    by_src = planned()
    assert by_src["/Locus/Reading/Proposed/2026-07-31 Covariance Shrinkage"].dest == "/Reading/Proposed"
    assert by_src["/Locus/Reading/In-Progress/2026-07-28 Regime Shifts"].dest == "/Reading/In-Progress"


def test_daily_pages_archive_by_their_own_month_not_todays():
    by_src = planned()
    assert by_src["/Locus/daily-2026-08-01"].dest == "/Daily/2026-08"
    assert by_src["/Locus/daily-2026-07-30"].dest == "/Daily/2026-07"


def test_topic_folders_move_under_notes_preserving_subfolders():
    by_src = planned()
    assert by_src["/engineering/B14 Vibrations"].dest == "/Notes/engineering"
    assert by_src["/brevan_howard/speaker notes"].dest == "/Notes/brevan_howard"
    # the oqts grouping he made himself must survive the move
    assert by_src["/projects/oqts/design"].dest == "/Notes/projects/oqts"


def test_admin_moves_and_trash_is_never_touched():
    by_src = planned()
    assert by_src["/admin/tenancy agreement"].dest == "/Admin"
    assert not any(src.startswith("/trash") for src in by_src)


def test_folders_themselves_are_never_moved():
    """Only documents move — an `rmapi mv` on a folder moves a whole subtree in one unreviewable
    call, which is the operation the plan file exists to make impossible."""
    for src in planned():
        assert not src.rstrip("/").endswith(("/Locus", "/engineering", "/projects"))
    assert all(not m.src.endswith("/oqts") for m in DM.plan(DM.list_device(fake_runner())))


def test_reading_list_is_flagged_for_review_because_it_is_mixed():
    """The whole reason the plan is item-level: this folder holds a book AND a handwritten note,
    and filing the note as a book would hand his own writing to the annotation sweep."""
    by_src = planned()
    assert by_src["/reading_list/Advanced Portfolio Management"].review
    assert by_src["/reading_list/reading list from Tom"].review


def test_unrecognised_folder_gets_no_guessed_destination():
    # The `[d]` line matters: without it the document reads as one whose NAME contains a slash.
    listing = DEVICE_TREE + "[d] /something_new\n[f] /something_new/a doc\n"
    moves = {m.src: m for m in DM.plan(DM.list_device(fake_runner(listing)))}
    move = moves["/something_new/a doc"]
    assert move.dest == "" and move.review


def test_a_document_already_at_its_destination_produces_no_move():
    """Re-running after a partial migration must be a no-op, not a self-move."""
    listing = "[d] /Notes\n[d] /Notes/engineering\n[f] /Notes/engineering/B14 Vibrations\n"
    assert DM.plan(DM.list_device(fake_runner(listing))) == []


def test_empty_listing_raises_rather_than_planning_against_nothing():
    with pytest.raises(RuntimeError):
        DM.list_device(fake_runner("[d] /Locus\n"[:0]))


# --- validation -------------------------------------------------------------------------------


def test_validate_reports_review_items_and_collisions():
    moves = [
        DM.Move("/a/x", "/Notes/t", "r"),
        DM.Move("/b/x", "/Notes/t", "r"),          # same name, same destination
        DM.Move("/c/y", "/Notes/t", "r", review=True),
    ]
    check = DM.validate(moves)
    assert len(check.collisions) == 1
    assert len(check.needs_review) == 1
    assert not check.ok


# --- plan file --------------------------------------------------------------------------------


def test_plan_file_round_trips_and_survives_hand_editing(tmp_path: Path):
    path = tmp_path / "device-migration.toml"
    DM.write_plan(path, DM.plan(DM.list_device(fake_runner())))

    # He retargets the handwritten notebook and clears its review flag, exactly as the header says.
    text = path.read_text()
    text = text.replace(
        'src    = "/reading_list/reading list from Tom"\ndest   = "/Reading/In-Progress"',
        'src    = "/reading_list/reading list from Tom"\ndest   = "/Notes/brevan_howard"',
    )
    path.write_text(text)

    by_src = {m.src: m for m in DM.read_plan(path)}
    assert by_src["/reading_list/reading list from Tom"].dest == "/Notes/brevan_howard"
    # still flagged, because he has not cleared the flag yet
    assert by_src["/reading_list/reading list from Tom"].review


# --- snapshot + apply -------------------------------------------------------------------------


def test_snapshot_mirrors_the_device_tree_locally(tmp_path: Path):
    fetched: list[tuple[list[str], Path]] = []

    def snap(args: list[str], cwd: Path) -> tuple[int, str, str]:
        fetched.append((args, cwd))
        return 0, "", ""

    items = DM.list_device(fake_runner())
    result = DM.snapshot(items, tmp_path, runner=snap)
    assert result.complete
    assert (tmp_path / "engineering").is_dir()
    assert (["get", "/engineering/B14 Vibrations"], tmp_path / "engineering") in fetched


def test_snapshot_records_a_failure_instead_of_raising(tmp_path: Path):
    def snap(args: list[str], cwd: Path) -> tuple[int, str, str]:
        return (1, "", "boom") if "Vibrations" in args[1] else (0, "", "")

    result = DM.snapshot(DM.list_device(fake_runner()), tmp_path, runner=snap)
    assert not result.complete and result.failed


def test_apply_refuses_without_a_complete_snapshot():
    moves = [DM.Move("/engineering/x", "/Notes/engineering", "r")]
    with pytest.raises(RuntimeError, match="snapshot"):
        DM.apply(moves, runner=fake_runner(), snapshot_result=None)


def test_apply_refuses_while_anything_needs_review():
    moves = [DM.Move("/reading_list/x", "/Reading/In-Progress", "r", review=True)]
    snap = DM.SnapshotResult(directory=Path("/tmp"), fetched=["/reading_list/x"])
    with pytest.raises(RuntimeError, match="review"):
        DM.apply(moves, runner=fake_runner(), snapshot_result=snap)


def test_apply_creates_destinations_then_moves_each_document():
    calls: list[list[str]] = []
    moves = [DM.Move("/projects/oqts/design", "/Notes/projects/oqts", "r")]
    snap = DM.SnapshotResult(directory=Path("/tmp"), fetched=["/projects/oqts/design"])

    applied = DM.apply(moves, runner=fake_runner(calls=calls), snapshot_result=snap)

    assert [a.ok for a in applied] == [True]
    # rmapi mkdir makes one level at a time — all three must be created, in order
    mkdirs = [c[1] for c in calls if c[0] == "mkdir"]
    assert mkdirs == ["Notes", "Notes/projects", "Notes/projects/oqts"]
    assert ["mv", "/projects/oqts/design", "/Notes/projects/oqts"] in calls


def test_one_failed_move_does_not_abort_the_rest():
    def runner(args: list[str]) -> tuple[int, str, str]:
        if args[0] == "mv" and "bad" in args[1]:
            return 1, "", "entry already exists"
        return 0, "", ""

    moves = [
        DM.Move("/engineering/bad", "/Notes/engineering", "r"),
        DM.Move("/engineering/good", "/Notes/engineering", "r"),
    ]
    snap = DM.SnapshotResult(directory=Path("/tmp"), fetched=["x"])
    applied = DM.apply(moves, runner=runner, snapshot_result=snap)
    assert [a.ok for a in applied] == [False, True]
    assert "already exists" in applied[0].error


# ---------- a document whose NAME contains a slash ----------
#
# The device permits it and rmapi does not survive it: rmapi splits paths on `/`, so `stat`, `get`
# and `mv` all report "file doesn't exist". Found live on 2026-08-02 — a note named
# `Learn List/ Questions` sitting in `/brevan_howard`, which `rmapi find` renders exactly like a
# document inside a `Learn List` subfolder. The tell is that no `[d]` line declares that folder.

SLASH_IN_NAME = """\
[d] /brevan_howard
[f] /brevan_howard/Learn List/ Questions
[f] /brevan_howard/Jargon Sheet
"""


def test_a_name_containing_a_slash_is_detected_and_never_planned():
    items = DM.list_device(fake_runner(SLASH_IN_NAME))
    assert DM.unaddressable(items) == ["/brevan_howard/Learn List/ Questions"]
    planned = {m.src for m in DM.plan(items)}
    assert planned == {"/brevan_howard/Jargon Sheet"}, "planning it would guarantee a failure"


def test_a_real_subfolder_is_still_planned_normally():
    """The detection must key on the missing `[d]` line, not on the depth of the path."""
    listing = "[d] /brevan_howard\n[d] /brevan_howard/Brian EM Rates\n" \
              "[f] /brevan_howard/Brian EM Rates/Dashboard\n"
    items = DM.list_device(fake_runner(listing))
    assert DM.unaddressable(items) == []
    assert DM.plan(items)[0].dest == "/Notes/brevan_howard/Brian EM Rates"


def test_an_unfetchable_document_outside_the_plan_does_not_block_the_migration():
    """The defect this pins: `apply` demanded a snapshot of the WHOLE DEVICE, so one document
    that was not even being moved stopped the entire migration."""
    moves = [DM.Move("/engineering/x", "/Notes/engineering", "r")]
    snap = DM.SnapshotResult(
        directory=Path("/tmp"),
        fetched=["/engineering/x"],
        failed=[("/somewhere/else", "file doesn't exist")],
    )
    applied = DM.apply(moves, runner=fake_runner(), snapshot_result=snap)
    assert [a.ok for a in applied] == [True]


def test_a_planned_document_that_could_not_be_backed_up_still_blocks():
    """The half that must NOT be relaxed: never move what we failed to copy."""
    moves = [DM.Move("/engineering/x", "/Notes/engineering", "r")]
    snap = DM.SnapshotResult(
        directory=Path("/tmp"),
        fetched=["/engineering/other"],
        failed=[("/engineering/x", "timeout")],
    )
    with pytest.raises(RuntimeError, match="could not be backed up"):
        DM.apply(moves, runner=fake_runner(), snapshot_result=snap)
