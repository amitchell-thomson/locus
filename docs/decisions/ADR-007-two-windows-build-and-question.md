# ADR-007 — The project window builds; the locus window questions and learns

**Status:** accepted
**Date:** 2026-10-22
**Your words:** "I think the primary split will be project window for code writing and implementing plans etc, and this window for learning and questioning things, if that split is clearly recorded then follow up makes sense, as I feel I would be following up a lot and would want it to be ergonomic to do so" (22 Oct 2026, the Answered round)
**Builds on:** ADR-006

## Context
Two places can hold a conversation with Claude about a project. The project window is the tmux
session where you work in the repo, with Claude Code sessions writing code. The locus window
asks about a project from outside it: the ask line on the Project view, the ask line on a
decision, a circled `?` in your ink, and follow-ups on Answered. Without a rule, a question could
start in either place, an answer could sprout code, and following up would mean choosing where
every time.

## Decision
The **project window is for building**: writing code, carrying out a plan, changing the repo.
The **locus window is for questioning and learning**: why something is the way it is, what a
method does, how a strategy works, what a decision rests on. Follow-ups on an answer stay in the
locus window, in that answer's thread (Space on Answered), so following up costs one key. When
an answer turns into something to build, `c` moves its thread to the project window as a working
session; nothing moves the other way without you asking.

Answers are as long as the question needs. Most are a few sentences; an explanatory question (the
physics behind a quast method, how a nabla strategy works) gets a full explanation, and only an
answer of that length offers to go to the tablet (`t`).

## Alternatives
- **One window for both:** every question would run in a coding session, which carries the repo's
  context and tools and is slow and costly for a question; and answers would land in a scroll of
  code work, where they cannot be found again.
- **Follow-ups in the project window:** the natural next question would leave the place the
  answer is read, so every follow-up would cost a window switch.
- **Ask whichever window you are in:** the same question asked twice could land in two places,
  and Answered would no longer be the one record of what you asked.

## Consequences
- Answered is the record of what you asked and learned about a project; the project window's
  sessions are the record of what was built.
- The answering job (heuris, Slice 3) runs read-only on the project's material, never edits the
  repo, and writes to the length the question needs, within the background budget.
- `c` on Answered is the one bridge from a question to building, and it carries the thread with
  it.
- A decision questioned here and found wrong is reopened here (`o`), and its fix is built in the
  project window.

## What would make this wrong
- If most questions turn out to need the repo's working state (uncommitted changes, a running
  test) to answer, questioning belongs in the project window after all.
- If follow-ups often turn into code, the bridge is the common path and should become the
  default rather than a key.
