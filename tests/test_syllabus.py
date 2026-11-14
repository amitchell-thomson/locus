"""Syllabi cut into submodule sections, verbatim, and filed by Canvas module (spec P, P8)."""

from __future__ import annotations

from pathlib import Path

from locus import syllabus

# Shapes from the real editions: a colon heading, a colon-less one (2024-25's p104), a wrapped
# title (2025-26's a402), page furniture, and the next paper's heading ending the last section.
TEXT = """1st Year Syllabus 2024-25
p103: Linear Algebra and Complex Algebra; 8 Lectures, 2 Tutorial Sheets
Rudiments of vector algebra including dot and cross product.
 
Department of Engineering Science
7
p104 Ordinary Differential Equations 1; 8 Lectures, 2 Tutorial Sheets
Homogeneous and non-homogeneous equations.
a402: Electromagnetic Fields for Energy Conversion and Electrical Motors, Generators and
Drives; 8 Lectures, 2 Tutorial Sheets
Integral forms of Maxwell's equations.
b704: Fluid Flow & Heat and Mass Transfer Laboratory
b704b Distillation Tray Simulator: Distillation columns are usually equipped
Paper P2: Electronic and Information Engineering
Not part of any section.
"""


def test_sections_are_cut_verbatim_with_their_counts():
    s = syllabus.sections(TEXT)
    assert [x.code for x in s] == ["p103", "p104", "a402", "b704"]
    assert s[0].title == "Linear Algebra and Complex Algebra" and s[0].counts.startswith("8 Lectures")
    assert "Department of Engineering Science" not in s[0].text and "\n7\n" not in s[0].text
    assert s[2].title.endswith("Generators and Drives") and "Maxwell" in s[2].text
    assert "b704b Distillation" in s[3].text and "Paper P2" not in s[3].text
    assert [x.course for x in s] == ["p1", "p1", "a4", "b7"]
    assert syllabus.course_of("b1501") == "b15" and syllabus.course_of("b101") == "b1"


def test_modules_match_by_name_and_wrong_neighbours_do_not():
    m = ["Linear Dynamic Systems", "Optimal Control", "Optimal Control - Kalman Filters"]
    assert syllabus.match_module("Optimal Control", m) == "Optimal Control"
    assert syllabus.match_module("Control Laboratory", m) is None
    assert syllabus.match_module("Electrical Laboratory", ["⚡ Electrical Lab", "🔧 Mechanical Laboratory"]) == "⚡ Electrical Lab"
    assert syllabus.match_module("Mechanical Vibrations", ["Mechanical Vibration"]) == "Mechanical Vibration"
    assert syllabus.match_module("anything", ["Energy Systems", "Electrical Machines"], "a402") == "Electrical Machines"
    assert syllabus.folder_name("Calculus 1: Single-variable calculus (1P1A)") == "calculus-1-single-variable-calculus"


def test_plan_files_each_section_under_its_course_with_his_notes():
    secs = syllabus.sections(TEXT)
    courses = {"p1": {"area": "Studies", "folder": "p1-mathematics"}}
    p = syllabus.plan(secs, courses=courses, modules={"p1": ["Linear Algebra and Complex Algebra (1P1B, 1P1C)"]},
                      edition="1st Year Syllabus 2024-25", source="src:218", root=Path("/f"),
                      notes=[("p103", "data/notes/a.md"), ("p103", "data/notes/b.md")])
    la = Path("/f/Studies/p1-mathematics/linear-algebra-and-complex-algebra/syllabus.md")
    assert set(p.files) == {la, Path("/f/Studies/p1-mathematics/ordinary-differential-equations-1/syllabus.md")}
    body = p.files[la]
    assert "his_notes: data/notes/a.md" in body and "his_notes: data/notes/b.md" in body
    assert "Rudiments of vector algebra including dot and cross product." in body
