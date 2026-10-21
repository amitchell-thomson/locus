# ADR-006 — The registry holds three kinds: project, course, application

**Status:** accepted
**Date:** 2026-10-22
**Your words:** "Do applications and my CV really count as projects ... they are internship
applications and my CV, they do not feel like projects" (21 Oct 2026); "they do not have git
repos, they are not tmux sessions ... Need to sort how we treat this properly once and for all,
as this is a slightly messy taxonomy issue that I would like to sort as it is key for the
backend logic" (22 Oct 2026); on the model proposed: accepted, with a request for this map
(22 Oct 2026).

## Context
The Slice 1 fixtures made "project" mean "anything with a view" (plan D, 19 Oct: "include other
types of work like careers"), so an internship application was a project with a Project view.
That broke in use: an application has no repository and no tmux session, yet Enter on it handed
it to the Project window, which follows tmux, and it showed as a blank page under another
project's name. The backend needs one rule for what follows tmux, what has a dossier, and where
each thing is listed.

## Decision
The registry (plan B.2, owned by the shared root) has one entry per thing you work on, of three
kinds. The **kind** decides which view shows it and whether anything follows it; the **area**
decides where Home lists it and its colour.

| Kind | What it is | Has | Opens in |
|---|---|---|---|
| project | something you build over weeks | a repository, tmux sessions, Claude chats, change records, a branch graph, a decision log, a dossier (by depth) | the Project window (Alt-2), which follows tmux |
| course | a course you are taking | notes in `studies/<course>/`, lectures in the library, Canvas deadlines in kairos, topic tracks | a course view (later slice) |
| application | a pursuit with stages, owned by a company | a stage and whose move it is, dates in kairos, mail threads in keryx, `careers/<company>/` notes, the CV version sent, a prep plan | an Application screen, opened in place from Home |

Three rules follow:
1. **The Project window shows only projects.** It follows tmux sessions, and only projects have
   them. An application never takes it over.
2. **Home's sections go by area, not kind.** "Projects" lists everything outside Careers;
   "careers" lists the Careers area. The CV is a project in the Careers area: it has a
   repository, a `cv` tmux session and chats, and its per-firm branches and release tags are its
   branch graph. 3YP is a project in the Studies area ("in the format of a long running
   project", 21 Oct).
3. **Learning goals are not registry entries.** heuris owns them (plan B.2). A prep plan is a
   goal tied to an application; topic tracks are shared by every goal.

## The map

```
 THE SHARED ROOT ─────────────────────────────────────────────────────────────────────────
  areas      Studies         Projects            Careers                Personal
  streams                                                               Family · Sport ·
                                                                        Social · Travel and
                                                                        errands (filtered by
                                                                        stream; the rest whole)

  registry   kind ╲ area     Studies     Projects                Careers
             project         3YP         quast nabla locus OQTS  CV
             course          B1 practical
             application                                         Jump · G-Research · Millennium

 WHAT EACH KIND CARRIES, AND WHO OWNS IT (plan B.2) ───────────────────────────────────────
  project      repository + tmux sessions ─▶ Claude sessions (Claude Code hooks)
               commits and chats ─▶ change records ─▶ dossier, decision log (heuris)
               locus: kairos and keryx are components of it, not projects of their own
  course       notes studies/<course>/ (you) · lectures, PDFs (library) · deadlines (kairos)
  application  stage, whose move (heuris) · dates (kairos) · mail (keryx)
               careers/<company>/ notes (you) · CV version sent (the CV project's tags)

 ACROSS EVERY KIND ─────────────────────────────────────────────────────────────────────────
  time         events, work blocks, tasks, chores (kairos), each with an area or stream
  learning     goals (heuris) ─▶ syllabus of topic tracks ◀─ shared by every goal
               a prep plan = a goal tied to one application
  information  sources (the library) · work (repositories and chats, through change records) ·
  (info.tex)   learning state; each item carries exactly one area label
  explore      scout finds and your ideas, each optionally pointing at a project or course

 WHERE YOU MEET IT ─────────────────────────────────────────────────────────────────────────
  Home         projects (area ≠ Careers) · careers (area = Careers) · learning (goals)
  Alt-2        the Project view: projects only, following the project window
  from Home    an application opens its Application screen in the Home window
```

## Consequences
- `ProjectKind` is `project | course | application` (was `code | light | course |
  opportunity`); the CV is `project`, the applications `application`.
- The Project window's follow logic needs no special cases: it only ever resolves a tmux session
  to a project.
- An Application screen is needed; it is designed as its own page in the Slice 1 rounds. Until
  it exists, an application opens the Project view in place, never in the Project window.
- Courses get their own view in a later slice; B1 practical is registered but inactive now.

## What would make this wrong
- An application that grows a repository and sessions of its own (a take-home project): it
  would be a project in the Careers area linked to the application, not a fourth kind.
- If courses and projects turn out to need the same view, `course` folds into `project` with
  the Studies area doing the work.
