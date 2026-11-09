"""~/files and the tablet keep one folder tree (files-and-chat M1)."""
import pytest

from locus import files_sync as fs

REG = {"Studies": {"b1-engineering-computation", "b14-information-engineering-systems"},
       "Projects": {"quast"}, "Careers": {"brevan-howard"}, "Personal": set()}
BOTH = {"Studies", "Projects", "Careers", "Personal", "Studies/b1-engineering-computation",
        "Studies/b14-information-engineering-systems", "Projects/quast", "Careers/brevan-howard"}


def test_a_deep_folder_new_on_either_side_is_made_on_the_other():
    laptop = BOTH | {"Studies/b1-engineering-computation/report"}
    tablet = BOTH | {"Projects/quast/papers", "Projects/quast/papers/rates"}
    p = fs.plan(laptop, tablet, REG)
    assert p.to_tablet == ["Studies/b1-engineering-computation/report"]
    assert p.to_laptop == ["Projects/quast/papers", "Projects/quast/papers/rates"]   # parent first
    assert p.questions == []


def test_a_registered_folder_missing_on_a_side_is_made_there():
    p = fs.plan(BOTH - {"Projects/quast"}, BOTH, REG)
    assert p.to_laptop == ["Projects/quast"]


def test_an_unregistered_thing_folder_is_a_question_and_its_children_wait():
    tablet = BOTH | {"Studies/c5-new-course", "Studies/c5-new-course/sheets"}
    p = fs.plan(BOTH, tablet, REG)
    assert p.to_laptop == []
    assert p.questions == [{"path": "Studies/c5-new-course", "on": "tablet", "why": "unregistered"}]


def test_personal_has_no_registry_so_it_mirrors_freely():
    p = fs.plan(BOTH | {"Personal/recipes"}, BOTH, REG)
    assert p.to_tablet == ["Personal/recipes"] and p.questions == []


def test_a_name_with_spaces_is_never_carried_across():
    p = fs.plan(BOTH, BOTH | {"Projects/quast/Old Notes"}, REG)
    assert p.to_laptop == []
    (q,) = p.questions
    assert q["why"] == "name" and q["suggest"] == "Projects/quast/old-notes"


def test_a_pending_rename_does_not_make_a_second_folder():
    """Before the rename, the tablet has `B14 Information Engineering Systems`; making
    `b14-information-engineering-systems` beside it would leave two folders for one course."""
    tablet = (BOTH - {"Studies/b14-information-engineering-systems"}) | {"Studies/B14 Information Engineering Systems"}
    renames = [("Studies/B14 Information Engineering Systems", "Studies/b14-information-engineering-systems")]
    p = fs.plan(BOTH, tablet, REG, renames)
    assert "Studies/b14-information-engineering-systems" not in p.to_tablet
    assert p.questions == [] and p.renames == renames


def test_rename_plan_reads_the_real_registry():
    tablet = {"Studies", "Studies/B14 Information Engineering Systems", "Studies/B3 Group Design Project",
              "Careers/Brevan Howard", "Projects/quast", "Projects/Tanker-flow"}
    got = dict(fs.rename_plan(tablet))
    assert got == {
        "Studies/B14 Information Engineering Systems": "Studies/b14-information-engineering-systems",
        "Studies/B3 Group Design Project": "Studies/b3-group-design-project",
        "Careers/Brevan Howard": "Careers/brevan-howard",
        "Projects/Tanker-flow": "Projects/tanker-flow",
    }


def test_the_laptop_tree_does_not_look_inside_repos_or_environments(tmp_path):
    repo = tmp_path / "Studies" / "b1-engineering-computation" / "b1-coding-practical-mt24"
    (repo / ".git").mkdir(parents=True)
    (repo / "uuv_mission" / "__pycache__").mkdir(parents=True)
    (tmp_path / "Studies" / "b1-engineering-computation" / "report" / "figures").mkdir(parents=True)
    (tmp_path / "Projects" / "quast" / ".hidden").mkdir(parents=True)
    got = fs.laptop_folders(tmp_path)
    assert "Studies/b1-engineering-computation/b1-coding-practical-mt24" in got
    assert not any("uuv_mission" in g for g in got)
    assert "Studies/b1-engineering-computation/report/figures" in got
    assert not any(".hidden" in g for g in got)


def _folder(i, name, parent="", deleted=False):
    return {"DocumentID": i, "Metadata": {"visibleName": name, "parent": parent,
                                          "type": "CollectionType", "deleted": deleted}}


def test_the_tablet_tree_has_only_live_folders_under_the_areas():
    tree = {"Docs": [
        _folder("s", "Studies"), _folder("b", "B1 Engineering Computation", "s"),
        _folder("x", "Gone", "s", deleted=True), _folder("t", "Old", "trash"),
        _folder("r", "Reading"), _folder("rp", "Proposed", "r"),
        {"DocumentID": "d", "Metadata": {"visibleName": "a note", "parent": "b", "type": "DocumentType"}},
    ]}
    assert fs.tablet_folders(tree) == {"Studies", "Studies/B1 Engineering Computation"}


def test_an_unreadable_tablet_is_an_error_not_an_empty_tablet():
    with pytest.raises(ValueError):
        fs.tablet_folders({"Documents": []})


def test_suggested_names_follow_the_registry_rule():
    assert fs.suggest("B1 Engineering Computation") == "b1-engineering-computation"
    assert fs.is_folder_name(fs.suggest("Citadel FI&M Trading!"))


def test_a_folder_under_its_old_name_is_one_folder_not_two():
    """The tablet's `Technology Strategy` and the laptop's `technology-strategy` (9 Nov)."""
    laptop = BOTH | {"Studies/b1-engineering-computation/technology-strategy"}
    tablet = BOTH | {"Studies/b1-engineering-computation/Technology Strategy"}
    p = fs.plan(laptop, tablet, REG)
    assert p.to_tablet == [] and p.to_laptop == []
    assert [q["why"] for q in p.questions] == ["name"]


def test_general_mirrors_in_every_area_and_missing_areas_are_made():
    p = fs.plan(BOTH, (BOTH - {"Personal"}) | {"Careers/general", "Careers/general/cover-letters"}, REG)
    assert p.to_laptop == ["Careers/general", "Careers/general/cover-letters"]
    assert p.to_tablet == ["Personal"] and p.questions == []


def test_deep_renames_go_deepest_first_and_never_merge():
    tablet = {"Studies", "Studies/b14-x", "Studies/b14-x/Image Processing",
              "Studies/b14-x/Image Processing/Week One", "Studies/b14-x/Signal", "Studies/b14-x/signal"}
    assert fs.deep_rename_plan(tablet) == [
        ("Studies/b14-x/Image Processing/Week One", "Studies/b14-x/Image Processing/week-one"),
        ("Studies/b14-x/Image Processing", "Studies/b14-x/image-processing"),
    ]
