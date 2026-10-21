# ADR-006 — One taxonomy for everything: areas say where, kinds say what

**Status:** accepted
**Date:** 2026-10-22 (revised the same day, round 2d)
**Your words:** "Do applications and my CV really count as projects ... they are internship
applications and my CV, they do not feel like projects" (21 Oct 2026); "Need to sort how we
treat this properly once and for all, as this is a slightly messy taxonomy issue that I would
like to sort as it is key for the backend logic" (22 Oct); "3YP seems to belong in both studies
and project, (and course) ... this still feels really clunky ... include, with examples
EVERYTHING and where it gets labelled" (22 Oct); "A studies section could be good actually ...
the projects section on the home page could be personal projects, and then 3YP could go in the
studies section" (22 Oct).

## Context
The Slice 1 fixtures made "project" mean "anything with a view", so an application had a
Project view, opened in the Project window (which follows tmux) and showed blank under another
project's name. The first fix (kind `project`) left the word doing two jobs: an area (a part of
life) and a kind (a shape of thing), so 3YP looked as if it belonged in three places.

## Decision
**Everything in locus carries two independent labels, and one taxonomy serves every app.**

- **Area**: which part of your life it belongs to. Studies, Projects, Careers, Personal; Personal
  splits into streams (Family, Sport, Social, Travel and errands) and is filtered by stream, the
  others whole (plan B.3). An item has exactly one area (info.tex).
- **Kind**: what shape of thing it is, which decides the machinery it gets and the screen that
  shows it. A kind never implies an area, and an area never implies a kind.

The taxonomy is **global**: it lives in the shared root (the taxonomy file and the registry in
this repository, served by the one HTTP service; plan B.2), and every app and every project reads
the same one. Which app *stores* a thing (kairos keeps tasks, keryx keeps mail) is a storage
detail and plays no part in how it is labelled.

### Registry kinds (one entry per thing you work on)
| Kind | What it is | Gets |
|---|---|---|
| repo | a repository you work in | tmux sessions, Claude chats, change records, a branch graph, a decision log, a dossier; the Project window shows and follows it |
| course | a course you take | notes, lectures, deadlines, sheets, topic tracks; a course view (later) |
| application | a pursuit with stages, owned by a company | a stage and whose move, dates, mail, notes, the CV version sent, a prep plan; an Application screen |

### Kinds of everything else
**One kind per thing; a thing that seems to be two kinds is two linked things** (22 Oct): B3
Group Design Project is a course, 3YP the repo that does its work, linked by `course: b3`.
**A due date is an attribute, not a kind**: a task, an application's stage or a course milestone
can each carry one, hard or soft. "G-Research applications close 31 Oct" is a date on the
application.

| Kind | What it is | Linked to |
|---|---|---|
| task | something to do, with a **type** and optionally a due date: **sheet** (a weekly problem or tutorial sheet, planned as about five hours in two or three blocks), **coursework** (assessed work needing its own estimate: a lab report, a 3YP milestone), **admin** (a one-sitting errand), **chore** (life admin for a gap: laundry, a parcel) | its course, repo or application, or nothing |
| event | something that happens at a time (Futsal, a lecture, an interview) | anything |
| mail thread | an email conversation; can become a task or deadline through Approve | anything |
| source | a paper, book, lecture slides, a note; the library only grows | the course or repo it serves |
| session | one Claude Code chat | its repo |
| decision | an entry in a repo's decision log | its repo |
| dossier | a repo's understanding, rebuilt | its repo |
| goal | what you are learning for (Jump internship preparation, personal learning) | a course, application, or nothing |
| track | a topic you are learning (probability, PyTorch), shared by every goal | goals |
| practice sheet | a sheet made for the tablet from a track | its track and goal |
| prep plan | a goal tied to one application | its application |
| find, idea | a scout find or your own idea | optionally a repo or course |
| jot | something captured on the tablet or typed, filed through Approve | wherever it is filed |
| ink | a mark on a page | the source it is on |

A *goal* is a purpose for learning; a *mail thread* is an email conversation. They share nothing
but the word "thread" in older notes, which no longer names anything in locus.

### The grid: every area by every kind, with real examples
| | Studies | Projects | Careers | Personal |
|---|---|---|---|---|
| **repo** | 3YP (does B3's work) | quast, nabla, locus (with kairos and keryx), OQTS | CV | |
| **course** | B1 Engineering Computation, B2 Engineering in Society, B3 Group Design Project, B14 Information Engineering Systems, B15 Control Systems, B16 Software Engineering Systems, B18 Biomedical Modelling and Monitoring, B20 Machine Learning | | | |
| **application** | | | Jump, G-Research, Millennium | |
| **task** (type, due date) | sheet: B15 sheet 3, due fri; coursework: a lab report | admin: book an ARC GPU slot | admin: return internship paperwork, due 31 Oct | chore: laundry, the parcel (Travel and errands) |
| **event** | a lecture, the 3YP briefing | a supervisor meeting | an interview | Futsal (Sport) |
| **mail thread** | the 3YP group thread | RIKEN's numbers | a recruiter's email | family (Family) |
| **source** | lecture slides, a textbook | a tuning paper | a firm's report | |
| **session, decision, dossier** | 3YP's | quast's, nabla's | the CV's (sessions only) | |
| **goal** | personal learning (SVD) | | Jump internship preparation | |
| **track** | probability, linear algebra | | (shared: used by prep plans) | |
| **prep plan** | | | the Jump final round | |
| **find, idea** | what a course offers a project | Bayes-opt barrier tuning | | |
| **jot, ink** | a thought in a lecture | a margin note on the dossier | sponsor-talk notes | a person to remember (stays in jot) |

### Where you meet it
- **Home** has a section per area: personal projects (Projects), careers | studies (Careers,
  Studies; a course with a linked repo shows it on the course's row);
  Personal shows through the calendar and chores. Learning opens from the last row of studies.
- **Alt-2**, the Project window, shows repos only, following the project window's tmux session,
  whatever the repo's area. An application opens its own screen from Home; a course its view.

## Consequences
- `ProjectKind` is `repo | course | application`; `Project.course` links a repo to its course.
  `ProjectView` gains `questions` (from the dossier) and `due` (sheets, coursework, reports,
  exams, each `DueKind`). kairos's task types become sheet, coursework, admin, chore; its
  separate deadline object becomes a due date on whatever it belongs to (a Slice 0 change to
  kairos, raised there).
- The Project window needs no special cases: it only resolves a tmux session to a repo.
- To build: the Application screen (a Slice 1 page); the course view and Canvas (Slice 7).
- Home's "projects" section and the Projects area now mean the same thing.

## What would make this wrong
- A thing that needs two areas at once. The rule is one area per item (info.tex); if that
  breaks often, areas become tags.
- Tracks are labelled Studies by default although prep plans in Careers use them; if that
  misfiles them in search or the weekly review, tracks get no area and inherit their goal's.
