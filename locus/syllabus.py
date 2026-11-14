"""Syllabi as structure (spec P, P8, first half; built 14 Nov 2026).

The department publishes one syllabus per year (Canvas: MEng Engineering Science > Key Documents
> Syllabi; the library holds the editions he took: 1st Year 2024-25 src:218, 2nd Year 2025-26
src:162). Each is a run of submodule sections headed `p103: Linear Algebra and Complex Algebra;
8 Lectures, 2 Tutorial Sheets`, then the topics and learning outcomes in the department's words.

This module cuts a syllabus into those sections, VERBATIM (P-D9: the original text is the
authority; nothing is paraphrased), and files each as `syllabus.md` in its submodule's folder in
`~/files/Studies/<course>/<module>/`, where `<module>` is the Canvas module's name in F1 form (his
rule, 14 Nov: names identical to Canvas, except the shorthand; a bracketed exam code such as
"(1P1A)" is dropped, his choice the same day). A section whose Canvas module cannot be found
keeps the syllabus's own title as its folder, and is reported.

What it does not do yet (P8, second half, after P7): the heuris topic table, links from each topic
to the material, and the "uncovered" flags.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

# `p103: Title; 8 Lectures...`, `b202: Engineering Ethics: 4 Lectures...`, `b504: Solid Mechanics
# Laboratory`. A colon after the code is required: `b704b Distillation Tray Simulator: ...` is a
# paragraph inside a lab, not a section.
# Some editions drop the colon ("p104 Ordinary Differential Equations 1; 8 Lectures"); then the
# lecture count is what marks it as a heading.
_HEAD = re.compile(r"^\s*([pab]\d{3,4}[a-z]?)(?:\s*:\s*|\s+(?=[^:\n]+;\s*\d+(?:\.\d+)?\s+Lecture))(.+)$", re.I)
_COUNTS = re.compile(r"[;:]?\s*(\d+(?:\.\d+)?\s+Lectures?.*)$", re.I)
# A paper's own heading ("B19 Fluid Mechanics: ...", "P1 Mathematics") ends the section before it.
_PAPER = re.compile(r"^\s*(Paper\s+)?[PAB]\d{1,2}[A-Z]?[.:]?\s+[A-Z][A-Za-z]")
_PAGE_FURNITURE = re.compile(r"^\s*(\d+(st|nd|rd|th) Year Syllabus \d{4}-\d{2,4}|Department of Engineering Science|\d{1,3})\s*$")


@dataclass
class Section:
    code: str            # "p103"
    title: str           # "Linear Algebra and Complex Algebra"
    counts: str          # "8 Lectures, 2 Tutorial Sheets" ("" for a lab)
    text: str            # the section, verbatim, page furniture removed
    course: str = ""     # registry id: "p1"
    module: str | None = None   # the Canvas module it belongs to
    folder: str = ""     # F1 folder name


def course_of(code: str) -> str:
    """`p103` -> p1, `b1501` -> b15, `b101` -> b1: the paper is the code less its last two digits."""
    c = code.lower().rstrip("abcdefghijklmnopqrstuvwxyz") if len(code) > 4 else code.lower()
    return c[:-2]


def sections(text: str) -> list[Section]:
    lines = [ln.rstrip() for ln in text.splitlines()]
    out: list[Section] = []
    cur: list[str] | None = None

    def close():
        if cur is None:
            return
        head = cur[0]
        m = _HEAD.match(head)
        assert m
        code, rest = m.group(1).lower(), m.group(2).strip()
        body = cur[1:]
        # a title that wraps onto the next line ("a402: Electromagnetic Fields ... Generators and /
        # Drives; 8 Lectures, 2 Tutorial Sheets")
        if not _COUNTS.search(rest) and body and _COUNTS.search(body[0]) and len(body[0]) < 90:
            rest = f"{rest} {body[0].strip()}"
            body = body[1:]
        cm = _COUNTS.search(rest)
        title = (rest[: cm.start()] if cm else rest).strip(" ;:")
        counts = cm.group(1).strip() if cm else ""
        while body and not body[-1].strip():
            body.pop()
        out.append(Section(code, title, counts, "\n".join([head.strip(), *body]).strip() + "\n",
                           course=course_of(code)))

    for ln in lines:
        if _PAGE_FURNITURE.match(ln):
            continue
        if _HEAD.match(ln) and not re.match(r"^\s*[pab]\d{3,4}[a-z]\s", ln, re.I):
            close()
            cur = [ln]
        elif cur is not None and _PAPER.match(ln) and not _HEAD.match(ln):
            close()
            cur = None
        elif cur is not None:
            cur.append(ln)
    close()
    # Collapse the blank-line runs PDF extraction leaves; the words are untouched.
    for s in out:
        s.text = re.sub(r"\n\s*\n(\s*\n)+", "\n\n", s.text)
    return out


def folder_name(name: str) -> str:
    """A Canvas module name as a folder: the bracketed exam code dropped ("(1P1A)"), then F1."""
    name = re.sub(r"\s*\([^)]*\)\s*$", "", name.strip())
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def _words(s: str) -> set[str]:
    s = re.sub(r"\([^)]*\)", " ", s.lower())
    words = {"laboratory": "lab", "laboratories": "lab", "labs": "lab"}
    out = set()
    for w in re.findall(r"[a-z0-9]+", s):
        if w in {"and", "of", "the", "to", "in", "for", "a"}:
            continue
        w = words.get(w, w)
        out.add(w[:-1] if len(w) > 3 and w.endswith("s") and not w.endswith("ss") else w)   # Vibrations / Vibration
    return out


# Where the names alone cannot tell (14 Nov, checked against the module's files on Canvas): A4's
# "Energy Systems" module holds the power electronics and power networks lectures (a403), and
# "Electrical Machines" the machines lectures (a402).
OVERRIDES = {"a402": "Electrical Machines", "a403": "Energy Systems"}


def match_module(title: str, modules: list[str], code: str = "") -> str | None:
    """The Canvas module a syllabus section belongs to: the best overlap of words over both names
    (Jaccard), at least 0.4; None when nothing comes close. Measured on the three editions (14
    Nov): true matches score 0.5 to 1.0 ("Calculus 1" against "Calculus 1: Single-variable
    calculus" is 0.5); the wrong ones the shorter-name measure let through ("Systems Physiology"
    against "Cellular Physiology", "Control Laboratory" against "Optimal Control") score 0.33."""
    if code in OVERRIDES:
        hit = [m for m in modules if m.strip() == OVERRIDES[code]]
        if hit:
            return hit[0]
    best, score = None, 0.0
    tw = _words(title)
    for m in modules:
        mw = _words(m)
        if not tw or not mw:
            continue
        s = len(tw & mw) / len(tw | mw)
        if s > score:
            best, score = m, s
    return best if score >= 0.4 else None


def _notes_for(s: Section, notes: list[tuple[str, str]]) -> list[str]:
    """His notes for a section. A key is `code`, or `code|title` where the syllabus reuses a code
    (the 2024-25 edition has p102 for both Calculus 2 and Calculus 3)."""
    out = []
    for key, ptr in notes:
        code, _, title = key.partition("|")
        if code.lower() == s.code and (not title or _words(title) <= _words(s.title)):
            out.append(ptr)
    return out


@dataclass
class Plan:
    files: dict[Path, str] = field(default_factory=dict)
    rows: list[tuple[str, str, str, str]] = field(default_factory=list)   # code, title, module, folder


def plan(secs: list[Section], *, courses: dict[str, dict], modules: dict[str, list[str]],
         edition: str, source: str, root: Path, notes: list[tuple[str, str]] | None = None) -> Plan:
    """What to write. `courses` is the registry by id; `modules` maps a registry id to its Canvas
    module names; `notes` pairs a section code (or `code|title`) with a pointer to his note; a code
    can have several (P105's notes are "ODE's 2" and "Block Diagrams"). Sections of a course not in
    the registry (the B papers he does not take) are skipped."""
    p = Plan()
    by_folder: dict[Path, list[Section]] = {}
    for s in secs:
        c = courses.get(s.course)
        if not c or not c.get("folder"):
            continue
        s.module = match_module(s.title, modules.get(s.course, []), s.code)
        s.folder = folder_name(s.module) if s.module else folder_name(s.title)
        path = root / c["area"] / c["folder"] / s.folder / "syllabus.md"
        by_folder.setdefault(path, []).append(s)
        p.rows.append((s.code, s.title, s.module or "(no Canvas module: syllabus title)", s.folder))
    for path, ss in by_folder.items():
        head = [
            "---",
            f"course: {ss[0].course}",
            f"codes: [{', '.join(s.code for s in ss)}]",
            f"canvas_module: {ss[0].module or ''}",
            f"edition: {edition}",
            f"source: {source}",
        ]
        for s in ss:
            for ptr in _notes_for(s, notes or []):
                head.append(f"his_notes: {ptr}")
        head += ["verbatim: true", "---", ""]
        intro = ("The department's syllabus for this submodule, word for word. It is the authority on "
                 "what the course covers; nothing here is paraphrased.\n\n")
        p.files[path] = "\n".join(head) + "\n" + intro + "\n\n".join(s.text for s in ss)
    return p
