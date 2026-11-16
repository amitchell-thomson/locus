# L: learning, from a goal to a curriculum you work through

**State:** approved 16 Nov 2026 ("spec fine"), with one amendment: the learning documents live in
`~/files/Learning/` is the one place for documents and code, a git repo synced with the tablet
like the rest (L-D11, L-D12) ·
**Written:** 2026-11-16 · **Runs in:** `claude` (locus-dev)

**From:** the plan (C.10 interview prep, C.11 the learning system; heuris.tex §E "one shape",
revision and the module cycle), the order of 13 Nov (C, P, then L), and your words of 16 Nov:
"give a goal of learning with a potential, 'master by' date and then a curriculum gets
constructed", with three goals to start, the first two "fed by the jump trading feedback call
notes/ any other feedback notes that I have for guiding weaknesses and choosing what exactly to
target."

**Done when:**
- You type a goal and a master-by date (in the locus window, or to any Claude), and within a few
  minutes a proposed curriculum is waiting in Approve: topics in order, each with why it is there
  (a pointer to the feedback line, prep plan, repo file or syllabus topic that put it there), hours,
  and the weekly time it needs to land by the date.
- Approved, it becomes kairos tasks that `plan` spreads across your weeks, and the Learning screen
  shows it live: per goal, the topics with a confidence level each, what is next, and whether you
  are on pace for the date.
- Practice is marked: sheets go to the tablet, your ink comes back, each answer is marked against
  the answer written when the sheet was made, you confirm or correct the marks, and they move the
  confidence levels. Coding practice is marked from the code you wrote by hand.
- The three goals below run through it for real, each with at least one marked practice round.

**Builds on:**
- heuris (search, facts, records, briefs): the evidence a curriculum is built from;
- the syllabus topics (P8): course structure, already linked to lectures;
- capture and `to_remarkable`: sheets out, ink back (`markups`, Loop B transcription);
- kairos tasks and `plan`: the time;
- Approve: the one place you agree to things.

---

## The three goals, as the test of the design

These are what the spec is for. Every decision below is checked against them.

**G1. Quant interview mastery** for the firms after Millennium: Jump, HRT, Tower, G-Research and
the like (your goal, fact:196: "work as a QT or QR alongside a systematic pod at an HFT firm").
**Master by: the end of Trinity 2027 exams.** Millennium starts the Monday after they finish
(fact:424), and the days between are for resting, not prep; after Millennium the interview
processes start (fact:423). So the work is spread over this term, the Christmas and Easter
vacations and Hilary, at the pace below, with nothing planned in exam weeks.
- *What the evidence says to target.* The Jump feedback call (src:614, 23 Oct): "v strong
  quantitatively", "areas for growth: programming", "do data structures and algorithms". The
  second feedback note (src:623): "a lot of claude usage, do the extra step of digging into things,
  rigorous understanding", "really learn all of the EV", the chessboard question, "probability/
  stats/regression stuff", "manual coding in projects, digging deeper", "maths/stats: just way
  more practice", data science "was good, not the reason". The onsite format (src:614): linear
  algebra, optimisation and linear regression; a programming round; a data-science case with
  partial data; a trader round of brainteasers and probability (green book).
- *What it builds on.* Your June prep plans (src:461, src:463, src:460): Zetamac, archetype Anki
  cards keyed setup to method, green book then Joshi, NeetCode patterns, timed sets and mocks.
  The curriculum adopts what worked rather than starting again.
- *Practice.* Probability and brainteaser sheets on the tablet, marked; linear algebra and
  regression derivations on the tablet; DSA problems written by hand (G2's rule), timed; one
  data-science case per few weeks on a real dataset in `interview-practice`.

**G2. C++ from the ground up, alongside OXDAQ, with Python mastery.** Master by: no hard date; the
aim is OXDAQ usable this academic year, so **roughly before Trinity term** (fact:423). The aim is
writing your own code, since agentic coding "was a big point of weakness in the jump interview
process".
- *The cycle* (your words, 16 Nov): "learn c++ concepts, then build a part of oxdaq using them and
  repeat". So G2's curriculum is a list of **rounds**, each a few C++ concepts (taught, practised,
  marked) then one OXDAQ part built with them. A round ends when its part works and you can explain
  it, not when time runs out.
- *First, OXDAQ is replanned* (your answer 2): the scope of the C++ engine, what stays Python, and
  where the C++ to Python boundary sits (pybind11 or a socket) decide which concepts the rounds
  need. That is a planning session with you, in chat, before G2's curriculum is proposed (step L8).
- *Shape.* The curriculum follows the build: each C++ topic lands just before the OXDAQ step that
  needs it (value semantics and RAII before the order type; containers and allocation before the
  book; templates and move semantics before the matching core; concurrency and memory ordering
  before the gateway; profiling and cache behaviour before the latency work). oxdaq-core today is
  Python, pre-build (its README, Phase 1); the C++ engine is the plan in fact:186, so the C++ build
  is a new phase of OXDAQ that this goal drives.
- *Python mastery* rides alongside as its own track: the language below the level you use it at
  (data model, iterators and generators, typing, asyncio, performance), practised in OXDAQ's Python
  client and bots.
- *The rule that makes it work* (L-D6): practice code is written by you, without Claude writing
  it. Claude sets the exercise, reviews and marks what you wrote, and explains; it does not
  produce the solution until you have submitted yours.

**G3. QUAST, start to finish: the physics and the ML.** **Lowest priority, little and often**
(fact:423): a fixed small weekly slot (my suggestion: two 30-minute sessions), no hard date, and it
gives way first when the week is full. Before the paper draft is final would be a natural target.
- *Syllabus from the repo itself:* `state.md`, the decision and finding files
  (`references/findings/9.1` to `9.11`), the simulator README and the code, plus the papers in the
  library they cite. Topics: double quantum dots and the charge stability diagram; exchange and the
  barrier gate; the CZ gate and its fidelity; noise (nuclear spins in natural silicon); the
  simulator's model; the tuning loop (what it measures, how it chooses, how sure it is); the ML
  underneath (the surrogate, its uncertainty, the search); and what pulse shaping would change.
- *Practice:* explain-back questions on the tablet ("why does the barrier gate give 200 times
  better gates than detuning?"), derivations, and "defend every claim" questions taken from the
  paper draft, marked against the repo's own findings.

**Pace** (fact:425): **5 hours a week for G1, 5 for G2, 1 for G3**, changeable at any time (L-D7).

**Priority** (fact:423): G1 and G2 on one level, G3 below. When a week cannot hold everything,
`plan` shortens G3 first and says so.

What they share is the point of shared tracks: linear algebra and statistics serve G1 and G3;
DSA and C++ serve G1 and G2; probability serves G1 and your B courses.

---

## Decisions for you (my recommendation first)

**L-D1. The shape stays the one you liked** (heuris.tex §E, 28 Oct: "that taxonomy is good"):
a goal has a target and date, a curriculum, tracks with confidence, sessions in kairos, and
marked practice. One change: the plan said "syllabus" for a goal's contents; I call it the
**curriculum** here, so it is never confused with a course's syllabus (P8), which is one of its
sources.

**L-D2. A curriculum is a tree of topics, each one grounded.** Built by one model call (Opus, it is
rare and matters) over the evidence the goal points at, then checked:
- every topic says **why it is there**, with a pointer (a feedback line, a prep-plan item, a repo
  file, a syllabus topic, a library section), and the pointer must exist and its quoted line must
  be in it, or the topic is dropped (grounded or silent; the same check facts and syllabus topics
  pass, recorded in `locus gates`);
- a topic may also be there because the goal's subject needs it with no note saying so (C++
  templates); those say "the subject" and cite a library source or a standard reference instead;
- each topic has an estimate in hours, prerequisites, and a weight: **weaker first**, where
  weakness comes from the feedback notes, from earlier marks, and from the baseline (L-D4).

**L-D3. Where a goal's evidence comes from.** You name the goal; locus gathers, and the proposal
shows what it used so you can add what it missed:
- feedback and prep notes about you: for G1 and G2 the main source is the **Feedback** notebook
  in Careers/Jump Trading on the tablet (src:623, transcribed; your answer 4), with the Jump call
  note (src:614) and the June prep plans; other firms' feedback when there is some;
- facts about you (goals, education: fact:341 says Years 1 and 2 can be treated as known well);
- for a project goal, the repo's `state.md`, docs and code;
- for a course, its syllabus topics (P8) and past papers;
- the library for teaching material.

**L-D4. Where you stand: a baseline before teaching** (your words, 10 Nov: "for new topic the
refresher is preceded by a baseline quiz, so we know where the gaps are"). The first thing an
approved goal sends is a short baseline sheet across its topics. Its marks set each topic's
starting confidence; the curriculum re-weights once from it, and you approve the change.

**L-D5. Confidence is earned only by marked practice.** A topic's confidence moves when an answer
is marked, and decays without practice (spaced review: a topic you got right comes back later, one
you got wrong comes back soon). Reading or a session logged without practice moves nothing. This
keeps the bars honest (C.10's point).

**L-D6. Marking.** A sheet's answers (the answer key) are written when the sheet is made, before
you see it, and stored; your ink is transcribed (Loop B) and each answer is marked against the key
by a model call, with a one-line reason. Model marking is not trusted blindly (CLAUDE.md §11:
self-assessment is not a usable signal), so **marks wait for you**: the marked sheet shows in
Approve, `y` accepts, `e` corrects a mark. Only accepted marks move confidence. For code: you
submit a file (from a practice folder in the repo, never one Claude wrote), it is run against the
exercise's tests where it has them, then reviewed against the reference; the test result is
deterministic, the review is advice.

**L-D7. Time.** Each goal has a **pace**: hours a week, which you set and can change whenever you
like (your words, 16 Nov: "easy to change if I want to ramp up or down the practice at any point
in time"). Changing it takes one step from anywhere: `+`/`-` on a goal in the Learning screen (an
hour a step), `locus learn pace g1 7`, or telling any Claude ("ramp interview prep up to 8 hours
for the vacation"), through a `set_pace` tool that applies at once and can be undone (a pace is
small scheduling, so it does not wait in Approve). A change re-spreads the goal's open tasks with
`plan` and re-draws the pace line ("at 5 h a week: done by 10 June; at 3 h: 4 weeks late"). A pace
can be set for a period ("8 h until 10 Jan, then 5"), so a vacation ramp ends by itself.

The curriculum's total hours and the master-by date give the load it needs; approving
the goal creates kairos tasks per topic (with `locus` = the goal's thing, estimates from the hours,
due dates from the order), and `plan` places them. Revision respects your rule (28 Oct): none in
Michaelmas or Hilary term for revision goals; G1 to G3 are not revision, so they take a few hours a
week in term and more in the vacations, which the weekly load shows before you approve.
**Recommendation:** each goal states its weekly hours in term and in the vacation, and the
proposal says whether the date is reachable at that pace.

**L-D8. Where it lives.** Learning state is not derived (your marks and goals exist nowhere else),
so it lives in `heuris.db` beside the facts ledger, outside `INDEX_TABLES`, and `locus backup`
copies it. Tables: `goals`, `topics` (the tree, per goal, with tracks shared by name),
`topic_evidence` (the pointers), `sheets`, `answers` (key, transcription, mark, accepted),
`confidence_log`.

**L-D9. How you use it.**
- **The Learning screen** (`L`) goes live and its sample data goes (C3's rule): goals with pace;
  in a goal, the topic tree with confidence bars and what is next; keys: `s` send the next sheet,
  `+` a new goal, `⏎` a topic (its evidence and history).
- **Approve** gets two new kinds: a proposed curriculum (or its re-weighting) and a marked sheet.
- **Any Claude** gets tools: `learning` (goals, a goal's curriculum and pace, a topic's
  history), `propose_goal(goal, master_by, notes)` (writes a proposal to Approve, never a goal),
  and `send_sheet(goal|topic)`; the chat can also explain a topic in depth on paper with the
  existing `to_remarkable`.

**L-D10. What a sheet is.** One LaTeX page or two in the device format (the reMarkable skill's
contract), 4 to 8 questions mixing topics by weight and due review, space to write under each, a
footer with its id so the readback finds it. Coding exercises are not sheets.

**L-D11. Code practice lives in the same place** (your amendment, 16 Nov: "I do not want two
different learning locations, that learning folder in files can itself be a repo"). See L-D12:
`~/files/Learning/` is one folder and one git repo, holding the documents and the exercises.

**L-D12. One place for everything: `~/files/Learning/`, a git repo, in `~/files` and on the
tablet** (your amendments, 16 Nov: "a directory under files that is also on the remarkable and
does all of the regular syncing, so that I can see the documents in both", and no second
location for code).
- **Goal ids are readable** (your words, 16 Nov: "goal id's should be readable"): lowercase words
  joined by hyphens, the same rule as registry folders, and a goal's folder is its id. The three:
  `interview-prep` (G1), `cpp-oxdaq` (G2), `quast` (G3).
- **Layout.** One subfolder per goal, `~/files/Learning/<goal>/`, each with `sheets/`,
  `explainers/` and `marked/` (PDFs) and `code/` (an exercise per folder: a stub, its tests, your
  solution). Top level rather than under an area, because goals cross areas (interview prep is
  Careers, OXDAQ and QUAST are Projects) and share tracks.
- **The whole folder is one git repo**, a registry entry (`learning`, kind repo) so your commits
  and exercises are indexed and searchable, and kairos counts the time you spend in it (it already
  matches a session in any folder named `learning`). Its `CLAUDE.md` says the rule that makes G2
  work: Claude never writes solution code here; it sets exercises, reviews and marks. Generated
  PDFs are ignored by git (`*.pdf` in `.gitignore`): they are rebuilt from locus and synced, not
  versioned.
- **The sync looks inside it.** `files_sync` today treats any folder with a `.git` as code and
  never looks inside (`_OPAQUE_MARKERS`), so `Learning/` is the one exception: its PDFs move to
  and from the tablet like `Studies/`; code and `.git` never leave the laptop's repo (and never
  reach the tablet). `Learning/` joins the top-level folders the sync knows (today Studies,
  Projects, Careers, Personal).
- **Indexing reads it where it is.** `repos_push` reads registry repos only under
  `~/local-projects`; a registry entry gains an optional `path`, and `learning` sets it to
  `~/files/Learning`.
- **Practice and product stay apart** (your case, 16 Nov: C++ exercises in `Learning/cpp-oxdaq/code/`
  and real code in OXDAQ). Exercises are small and may be thrown away; the OXDAQ part a round
  builds lives in OXDAQ. OXDAQ never imports from `Learning/`: an exercise that grows into something
  OXDAQ needs is rewritten there by hand (itself practice). Build output (`build/`, binaries) is
  ignored by git and skipped by the sync, so compiled code never reaches the server or the tablet.
- **Your rule of 14 Nov holds** (P-D0): only clean PDFs cross. Your answers in ink stay on the
  tablet and are read back into locus; `~/files` keeps the clean sheet. A marked sheet is a new
  clean PDF locus writes (your answer transcribed, the mark and the reason beside it), in
  `marked/`, so you can see the marking on the laptop too.
- **Capture learns the folder:** it reads `Learning/` on the tablet (today it reads only Studies,
  Projects, Careers, Inbox and Reading) and treats a sheet there as locus's own document, so your
  ink on it is read as answers to that sheet (matched by the id in its footer).

One correction: **the "build" half of each G2 round belongs in OXDAQ's own repo**, not the learning
repo. That code is the product. It is reviewed and marked the same way, but it lives where it runs.

---

## Steps

Each step ends with a printed check against real data.

**L1. Learning state and its folder.** The tables (L-D8, with a `pace` per goal and its dated
changes), backup and restore of them, and `locus learn` (list, show, pace); `~/files/Learning/`
made as a git repo with its `CLAUDE.md` and `.gitignore`, a registry entry with `path`, the sync's
exception for it, and `repos_push` reading it. Check: an empty goal round-trips through a backup,
a pace change is logged with its date, a PDF dropped in `Learning/` reaches the tablet while a
`.py` beside it does not.

**L2. A goal becomes a proposed curriculum** (L-D2, L-D3). `propose_goal` and `locus learn
propose`; evidence gathering; the Opus call with a schema; the grounding check and its gate; the
proposal in Approve. Check: G1 proposed from the real notes, every topic's pointer opens and its
quote is there; you read it.

**L3. Approval to time** (L-D7). Approving writes goal and topics and creates kairos tasks; `plan`
places them. Check: G1 approved, its first weeks visible in the calendar at the weekly load shown.

**L4. Sheets out** (L-D10, L-D4, L-D12). The `Learning/` folder in `~/files`, on the tablet and in
the sync; the baseline sheet and practice sheets, answer keys stored first, written to
`Learning/<goal>/sheets/`, tracked in `sheets`. Check: G1's baseline on the tablet and in
`~/files`.

**L5. Ink back and marked** (L-D6, L-D12). Capture reads `Learning/`; Loop B transcription of a
sheet; marking against the key; the marked sheet in Approve; accepted marks move confidence
(L-D5) and write the marked PDF to `marked/`. Check: your answers to G1's
baseline marked, corrected where wrong, confidence set.

**L6. The Learning screen live** (L-D9). Fixtures go; goals, pace, topics, history. Check: the
screen shows G1 as it stands, and `s` sends its next sheet.

**L7. Code practice** (L-D6, G2). Exercise files with tests, submission, deterministic run, review.
Check: one G2 exercise written by you, run, reviewed, marked.

**L8. G2 and G3 through it.** First the OXDAQ replanning session with you (scope of the C++
engine, the Python boundary, milestones rejigged); then G2 proposed as rounds against those
milestones, and G3 from QUAST's repo; both approved, one marked round each. Check: three goals live, shared tracks counting across them
(probability marked for G1 shows in a B-course goal later; linear algebra for G1 and G3).

**L9. Report** and the test questions 27 to 32 and P3 to P5 re-run.

---

## Edge cases

- A feedback note says something vague ("digging deeper"): it can justify a topic only through a
  concrete reading the proposal states ("write projects' core code by hand"), which you approve or
  strike.
- A topic both goals need (linear algebra): one track, two goals; its confidence is one number.
- You miss a week: pace shows behind; `plan` re-spreads; nothing is silently dropped.
- An answer the transcription cannot read: marked "unreadable", not wrong, and asked again.
- A sheet never answered: after two weeks it is shown as stale, and its topics come back due.
- The master-by date is unreachable at the stated weekly hours: the proposal says so before you
  approve, with the three choices (more hours, a later date, a smaller curriculum).
- A goal is abandoned: archived, its tasks dropped through kairos (which asks), its marks kept.

## Not in this

- Exam revision goals and the module cycle (heuris.tex §E): the same machinery, but B-course
  revision belongs to the vacation; it is the next use after L, and L's tables are built for it.
- Mock interviews by voice or live chat.
- "Defend every bullet" from the CV (C.12): a later goal type once L works.
- The nightly reading digest (ADR-012): its own spec; it will read L's goals for what to suggest.

## Risks

- **A curriculum that looks right and is generic.** The grounding check stops invented reasons,
  but a list of standard topics with real citations can still miss what you need. Mitigation: the
  proposal shows the evidence it used; you strike and add before approving; the baseline re-weights.
- **Model marking.** Mitigated by L-D6: nothing moves until you accept the marks.
- **Load.** Three goals at once on top of term: the pace line makes the cost visible before you
  approve, and goals can be paused.
- **Cost.** One Opus call per curriculum and per re-weighting; sheet writing and marking on
  Sonnet, batched where nobody waits; prompts cached. A few dollars a month at the pace above.

## Answered 16 Nov (fact:423)

1. G1 by the start of Millennium; G2 roughly before Trinity term, no hard date; G3 little and
   often, lowest priority. G1 and G2 share the top priority.
2. G2 follows a reopened OXDAQ plan, in rounds of concepts then a part built (step L8).
3. One place: `~/files/Learning/`, a git repo holding documents and exercises; OXDAQ's build code stays in OXDAQ (L-D11, L-D12).
4. The Feedback notebook in Careers/Jump Trading is the main feedback source.

## Still open

- Answered too: G1 ends with Trinity exams (Millennium starts the Monday after, fact:424); pace 5,
  5 and 1 hours a week, changeable at any time (fact:425).
- Nothing else blocks approval.

## Progress

**L1 done (16 Nov 2026).**
- Learning state in `heuris.db` beside the ledger (`locus/heuris/learning.py`): `goals`, `topics`,
  `topic_evidence`, `pace`. A pace is hours a week from a date, optionally until one, so a ramp
  ends by itself; every change is a row, so its history is kept.
- `locus learn list | show | add | pace | state`; `locus restore-ledger <snapshot>` puts a
  snapshot's facts and learning state back, adding only rows that are missing (the ledger had a
  backup and no way back until now).
- `locus backup` copies the learning tables with the ledger.
- `~/files/Learning/` is a git repo with `CLAUDE.md` (Claude never writes solution code there),
  `.gitignore` (PDFs and build output), and a folder per goal (`interview-prep`, `cpp-oxdaq`,
  `quast`), each with `sheets/`, `explainers/`, `marked/`, `code/`.
- The registry has `learning` (kind repo, `path = "~/files/Learning"`, no area folder); the
  registry check, the locus window, kairos's time measurement and `repos_push` find it by path.
- The sync knows `Learning/`: its goal folders need no registry entry, its PDFs move like
  `Studies/`, and `code/` never leaves the laptop (`tests/test_learning.py`).
