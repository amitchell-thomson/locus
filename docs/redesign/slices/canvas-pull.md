# P: the Canvas pull, and `~/files` and the tablet in step

**State:** approved 14 Nov 2026 with two amendments: syllabi are structured, per submodule (P-D9); no ink or notebook snapshots in `~/files`, and a clear split of what lives where (P-D0, P-D5). Waiting on the Canvas token (you, 15 Nov) · **Written:** 2026-11-14 · **Runs in:** `claude` (locus-dev)

**From:** your order of 13 Nov (cleanup, then this, then L), and your words: "I will want sheets/
notes/ lectures etc to appear automatically in files and on my remarkable if the type is
correct." The plan's line for Canvas is `[you, 16 Oct: "only want to pull relevant info"]`:
deadlines, PDFs, lectures, notes and course information `[you, 19 Oct]`; announcements already
arrive by mail.

**Done when:**
- A lecture, notes or examples PDF that appears on a course's Canvas page is in
  `~/files/Studies/<course>/<sub>/` and on the tablet at the same path within an hour, with no
  hand step.
- **What is on the tablet is in `~/files`:**
  - the course PDFs you put there by hand (B14, B15 and B18 today);
- A PDF you drop into `~/files/Studies/...` reaches the tablet if its type is right.
- Canvas's assessed deadlines arrive in Approve as kairos tasks.
- The library has every course PDF, and heuris search finds it directly (Q10's kind of miss
  cannot happen again).
- Every course's submodules have their syllabus as structured topics, each linked to the lectures,
  notes and sheets that cover it; the Year 1 and 2 ones too, and Q31 is scored.

**Builds on:**
- `locus files sync` (files-and-chat M1): it keeps folders in step, but not their contents.
- capture: it reads the tablet with one `rmapi ls` per tick, and `deliver_pdf` records every push.
- the library's ingest.
- heuris.

---

## What is there today (14 Nov)

- **The tablet's `Studies/`:** 39 documents.
  - 27 course PDFs you uploaded by hand: B14 IP and SP lectures, notes and examples; B15 notes,
    examples and answers; B18 lectures, notes and the examples sheet; B2 lectures and tutorial
    questions.
  - 5 notebooks: sheets you worked on the tablet, and B3's intro session.
  - Ink on 13 of them.
- **`~/files/Studies/`:**
  - B1's practicals and reports, and B2's technology-strategy PDFs: the only course PDFs that
    are on both sides.
  - **None of B14's, B15's or B18's course PDFs.** That is the gap you named.
- **Canvas:**
  - Nothing reads it, and no access token is stored anywhere.
  - Its assessed deadlines (B1's computational tasks) also come by mail from EngExams.
  - Tutorial sheet deadlines come from tutors by mail, not from Canvas (e.g. B18's sheet,
    Isobel Voysey, 9 Nov).
- **The library:**
  - Capture queues any non-Locus PDF in `Studies/` into `incoming/studies/`, so the hand-uploaded
    PDFs are already ingested.
  - heuris searches the library through the old retrieval code (`library_candidates`), not its
    own index.

---

## Decisions for you (my recommendation first)

**P-D0. What lives where** (your amendment, 14 Nov: "a clear division of what exists where between
remarkable, locus and the files tree").

| | reMarkable | `~/files` | locus |
|---|---|---|---|
| **Is for** | reading and writing by hand | opening and editing on the laptop | remembering and finding |
| **Holds** | course PDFs to read, sheets you work (with pasted plots), notebooks, Claude's documents | Canvas originals (clean), answers, code, data, LaTeX, reports, syllabi, `course.md` | an index over both, the library, facts, records; your ink read back (`markups`, transcriptions) |
| **Your ink** | lives here, viewed here | never | read and searchable, pointing back to the tablet |
| **Clean PDFs** | the ones you read | all of them | ingested into the library |

One rule follows: **files cross between the tablet and `~/files` only as clean PDFs.** Ink and
notebooks stay on the tablet; locus reads them so they are searchable and quotable, and you view
them in the reMarkable app. Nothing locus makes for itself (records, renders, caches) appears in
`~/files`.

**P-D1. How locus reads Canvas.**
- *Recommended:* a Canvas **access token** that you make once:
  - Canvas → Account → Settings → "+ New access token", named "locus", with an expiry at the end
    of the year.
  - It is stored only on the server, in `~/.config/locus/canvas.env`, never in the repo.
  - It reads with your rights and can do nothing you can't, but it *could* submit work, so locus
    calls only read (GET) endpoints, enforced in one client function.
- *If Oxford's Canvas has tokens turned off for students:*
  - Each course's calendar feed (the "Calendar Feed" link, no token) still gives deadlines.
  - For files, the fallback is Claude in Chrome reading the Files pages while you're signed in.
    That is slower and needs the laptop, so only if tokens are off.

**P-D2. What is pulled, and where it goes.**

| On Canvas | `~/files` | Tablet | Library + heuris | kairos |
|---|---|---|---|---|
| Lecture slides and notes (PDF) | yes | yes | yes | |
| Examples or problem sheets (PDF) | yes | yes | yes | |
| Answers or solutions (PDF) | yes | **no**, see P-D3 | yes | |
| PowerPoint or Word | as is, plus a PDF | the PDF | yes | |
| Code, data, zip, notebooks | yes | no | no | |
| Videos, Panopto links | a link list, `lectures.md` | no | no | |
| Assessed deadlines (assignments) | | | a fact | a task, in Approve |
| Course page, reading list | `course.md` | no | yes | |
| Each submodule's syllabus | `syllabus.md` | no | topics, linked (P-D9) | |
| Announcements | no (mail has them) | | | |

**P-D3. Answers stay off the tablet.**
- *Recommended:* solutions go to `~/files` and the library, and not to the tablet. Then a
  worked sheet on the tablet is never sitting next to its answers.
- `locus files to-tablet <file>`, or one key in Home, sends one when you want it.
- The alternative is to send everything.

**P-D4. Folder and file names.**
- **Subfolders:** a Canvas module or folder becomes the subfolder its name suggests under F1:
  "Image Processing" → `image-processing`.
  - An existing subfolder that matches wins.
  - A course with one module goes straight under the course folder.
- **Names on the laptop:** files keep Canvas's name in F1 form, with no spaces: `B18 Lecture
  1.pdf` → `b18-lecture-1.pdf`. That is your 9 Nov rule.
- **Names on the tablet:** the readable name, `B18 Lecture 1`.
- **Hand-uploaded copies already there:**
  - matched by the sha256 of the PDF bytes, so they are never pushed twice;
  - a tablet copy with your ink is never touched.

**P-D5. Content in both directions, by type** (this replaces files-and-chat's F3, "folders only").
- **Tablet → `~/files`:**
  - A PDF that isn't Locus's own lands in the matching folder.
  - Only the clean original bytes (the PDF as uploaded), never your ink.
  - Notebooks are not copied or rendered (P-D0): they stay on the tablet, and locus transcribes
    them so they are searchable. Your marked-up copies are read by `markups`, not copied.
    *Your call, 14 Nov:* "what would the handwritten documents actually do sat in my files";
    revisit only if a real use appears.
- **`~/files` → tablet:**
  - A PDF in `Studies/`, `Projects/` or `Careers/` that P-D2 sends to the tablet, and that
    isn't there, is pushed.
  - Files under a `.git`, `.venv`, `report/` or `figures/` folder are not. Your own LaTeX
    output is yours to send.
- **Neither direction ever deletes.**
  - Something gone on one side is put to you as a question, as folder sync already does.
  - A Canvas file that changes:
    - replaces the tablet copy if it has no ink (`deliver_pdf`'s existing rule);
    - otherwise arrives beside it as `<name> (v2)`, and Home says so.

**P-D6. Where it runs.**
- *Recommended:* the **server**, every 30 minutes, beside capture. It has rmapi, the library and
  the token, and it is always on.
- It keeps a mirror in `data/canvas/`.
- The laptop's existing `com.locus.files` job pulls the mirror into `~/files` (rsync over ssh,
  copy-only), so the laptop can be closed for a week and catch up.

**P-D7. Deadlines.**
- Canvas assignments with a due date become kairos task *proposals* in Approve, like keryx's
  suggestions: a name, the due date, and the course as its locus.
- The estimate is 5 h for a sheet (the scheduler's default), or what you type.
- Never created directly. A deadline the EngExams mail already produced isn't proposed twice
  (same course and due time).
- A tutor's sheet deadline from mail stays keryx's job.

**P-D8. The library moves into heuris** (cleanup D5, deferred to here).
- Library documents are indexed as heuris items (`kind = source`, the course as their thing),
  using the chunks and vectors they already have, so heuris search ranks them with everything
  else.
- `library_candidates` and the old retrieval path are then removed.
- The library's ingest (extract, LLM passes, figures) stays frozen as it is.

**P-D9. Syllabi are structure, not facts** (your amendment, 14 Nov: each submodule "has its own
very specific syllabus... the fundamental guidelines for what we should be learning/ focussing on
come revision").
- **Kept verbatim:** each submodule's syllabus, as Canvas states it, is saved as
  `~/files/Studies/<course>/<sub>/syllabus.md`, with its source link. That is the authority.
- **Read into topics:** a `syllabus` table in heuris holds one row per topic: course,
  submodule, order, the topic as written, and its source pointer. The topics are extracted from
  the page, and every topic must quote a line of the page or it is dropped (grounded or silent).
- **Linked to the material:** each topic points at the lectures, notes, sheets and your ink that
  cover it, matched by search and reranked. A link must clear the rerank floor, so a topic with
  nothing behind it shows as uncovered, not as a weak match.
- **What it is for:**
  - L builds on it: what to learn, what is covered, and what is thin before revision.
  - Revision planning reads coverage per topic, not per course.
- **Facts are only the summary:** `education:` facts say which courses and submodules you took
  and when (proposed in Approve, since they are about you). The topics themselves are not
  facts: they are the course's, and change only when Canvas does.
- **Year 1 and 2:** the same structure, from their Canvas pages. Q31 is scored against it.
- **On a change:** a syllabus page Canvas updates is re-read. The changed topics are shown to
  you, and their old rows are kept as history.

**P-D10. What comes later.** Panopto recordings themselves, transcripts of lectures, and
"what's new on Canvas" in Home. Lectures arrive as a link list only. Say if you want more.

---

## Steps

Each step ends with its check on real data, and is committed and deployed before the next.

**P1. Access** (P-D1).
- You make the token. I store it on the server.
- `locus canvas courses` lists your courses: this year's B-papers and the Year 1 and 2 ones.
- Each is mapped to a registry id: a `canvas = <id>` line on each course entry, proposed to you
  as one diff.
- *Check:* the list, and the registry diff, before anything else is read.

**P2. The pull, into a mirror** (P-D2, P-D4).
- `locus canvas pull` walks each mapped course's modules and files (GET only).
- It writes `data/canvas/<course>/<sub>/<file>`, plus a manifest: Canvas id, updated time,
  sha256 and type.
- Re-running fetches only what changed.
- *Check:*
  - B14, B15 and B18's lecture PDFs are in the mirror;
  - their sha256s match the hand-uploaded tablet copies;
  - a second run fetches nothing.

**P3. Mirror → `~/files`** (P-D6).
- The laptop job rsyncs the mirror into `~/files/Studies/` (copy-only, `--ignore-existing`
  for anything you edited), with F1 names.
- *Check:* B18's lectures, notes and sheet are in `~/files`, and folder sync reports nothing new.

**P4. To the tablet** (P-D2, P-D3, P-D5).
- New mirror files whose type goes to the tablet are pushed through `deliver_pdf`, recorded in
  `device_sent` before upload, paced against 429s.
- A file already on the tablet by sha256 is skipped.
- *Check:*
  - a dry run lists exactly what would go (nothing for B14, B15 or B18, which are already
    there by hand);
  - then one real course with something new.

**P5. Tablet → `~/files`, and `~/files` → tablet** (P-D5).
- **From the tablet:** non-Locus PDFs copied down as their clean bytes; no notebooks, no ink.
- **To the tablet:** new PDFs from `~/files` pushed, by the same type rule.
- Questions for anything gone.
- *Check:*
  - B18's hand-uploaded lectures appear in `~/files`, clean, and its notebook sheet does not;
  - a scratch PDF dropped in `~/files/Studies/b18-.../` reaches the tablet in one tick;
  - a scratch file in `report/` does not.

**P6. Deadlines** (P-D7).
- *Check:* this term's assessed deadlines appear in Approve once each.
- The B1 Numerical Algorithms one, already done, is not proposed.

**P7. The library into heuris** (P-D8).
- *Check:*
  - Q10 ("my GPTQ notes") and the H1/H2 answers that cite library documents (Y3, Y9, 10, 21)
    still find them through heuris's own index;
  - `library_candidates` is gone;
  - search latency stays within its budget.

**P8. Syllabi as structure, then Q31** (P-D9).
- `syllabus.md` saved per submodule.
- The topics extracted, each with its quoted line, and linked to the material.
- The `education:` facts proposed.
- *Check:*
  - B14, B15 and B18's submodules each list their topics, with the lectures behind each and the
    uncovered ones marked;
  - you read one submodule's list against its Canvas page;
  - Q31 is re-run and scored.

---

## Edge cases

- **The same PDF under two names** (Canvas, and your upload): matched by bytes, kept once.
- **Canvas re-uploads a corrected file** with the same name: v2 beside an inked copy; it
  replaces an un-inked one.
- **A file in two modules:** one copy, in the first module's folder; the manifest lists both.
- **A huge file** (a dataset or video): over 50 MB is listed in the manifest and not downloaded,
  and Home says so.
- **A course with no modules, only Files:** Canvas's folder tree is used instead.
- **A locked or unpublished file:** listed as locked, fetched when it opens.
- **The token expires or is revoked:**
  - the pull stops;
  - `locus status` and System say so in red, once, with how to renew it;
  - nothing else breaks.
- **The tablet is offline or rate-limited:**
  - pushes wait, and the mirror and `~/files` still update;
  - a partial `ls` is never read as deletions.
- **A file you moved or renamed in `~/files`:**
  - the manifest remembers it by sha256 and doesn't put it back;
  - a copy you deleted is not re-fetched either: the manifest records it as removed by you.
- **Year 1 and 2 courses:** pulled for the syllabi only (P-D9), not their files, unless you
  say so.
- **Notes you write beside a lecture in `~/files`** (markdown): left alone; P-D5 moves PDFs only.

## Not in this

- Panopto recordings and transcripts (P-D10).
- Submitting anything to Canvas. The client is GET-only.
- Projects' and Careers' content beyond P-D5's PDF rule.
- L (learning), which comes next and builds on these course folders.
- The Explore reading digest, which waits for its own spec.

## Risks

- **Oxford turns tokens off.** P1 finds out before anything is built, and the fallback is in
  P-D1.
- **A flood to the tablet.** Every push is listed first (P4's dry run). A limit of 30 pushes
  per run stops more, and asks.
- **Overwriting your ink.** It never happens: `deliver_pdf`'s rule, plus v2 files.
- **A path that looks wired and isn't** (CLAUDE.md §3). Each step's check is on real courses.
  `locus status` shows when the last pull ran and what it fetched, so a dead token can't pass
  for a quiet week.

---

## Progress

**P1 done (14 Nov 2026).**
- The token is on the server (`~/.config/locus/canvas.env`, 600). `locus/canvas.py` is GET-only by
  construction. `locus canvas courses` lists all 33 courses; on the laptop it asks the server.
- **Finding:** a course's Files tab is 403 for students; Modules are 200, and a file reached
  through a module item downloads. P2 walks modules.
- **His rule (14 Nov):** "the canvas titles and naming of courses/submodules in my system should
  all be identical (except of course for our shorthand names)". So every course's registry
  `name` is Canvas's title (B16 became "B16 Software Engineering"), its folder is that title in F1
  form, and a submodule's folder is its Canvas module name in F1 form. Renamed on both sides:
  `b16-software-engineering-systems` to `b16-software-engineering`;
  in B1, `b1-numerical-algorithms` to `numerical-algorithms` and `b1-coding-practical-mt24` to
  `coding-practical-1`. B1's `report/` is his coding practical report, not a module, and stays.
- Prelims (p1-p5) and Part A (a1-a5) are registered, inactive, with folders: a home for their
  syllabi (P8) and the few Year 1-2 items he keeps. He judged the rest of his GoodNotes years
  not worth bringing in (14 Nov): it would dilute locus.
- Order that matters: the contract's `canvas` field reached kairosd and every Mac binary before
  the registry used it (the readers refuse unknown keys).
- `rmapi mv <path> <bare-name>` moves to the tablet ROOT, not a rename in place: always give the
  full destination path.

**P8, first half, done for all three years (14 Nov 2026).** He asked for it before P2: "all of
that syllabus can be treated as known material" (Years 1 and 2, fact:341).
- `locus canvas syllabus <pdf>` (`locus/syllabus.py`) cuts a year's syllabus into its submodule
  sections, verbatim, and writes each as `syllabus.md` in `~/files/Studies/<course>/<module>/`.
  The module folder is the Canvas module name in F1 form, minus a bracketed exam code ("(1P1A)",
  his choice). 79 files: Year 1 from the 2024-25 edition (src:218), Year 2 from 2025-26
  (src:162), the B papers he takes from 2026-27 (Canvas).
- Each file names its edition and source, and links his GoodNotes notes for that code
  (`his_notes:`, 29 links).
- Matching a section to its Canvas module is word overlap (Jaccard 0.4, measured: true matches
  0.5 to 1.0, the wrong neighbours 0.33), and a hand override where names cannot tell (A4's
  a402 and a403, checked against the modules' files).
- Not searchable yet: heuris does not index `~/files`. The second half of P8 (the topic table,
  links to material, uncovered flags) loads them, after P7.

**GoodNotes years (14 Nov 2026).** Only his 29 summary notes from Prelims and Part A come in
(448 pages), transcribed by capture's vision transcription into `data/notes/goodnotes-*.md` as his
writing. The GoodNotes text layer was useless for maths. Everything else stays on Google Drive.
