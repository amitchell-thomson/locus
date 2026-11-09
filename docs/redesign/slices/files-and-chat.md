# Files and the Home chat

**State:** spec written 9 Nov 2026, awaiting approval · **Written:** 2026-11-09 · **Runs in:** `claude` (locus-dev window 4)

**From:** the 9 Nov review of where locus stands. You agreed to:
- Cut to what you will use now. Everything else is kept, not deleted, and comes back only when
  you want an improvement to something you already use.
- Drop the Project view for the time being.
- Keep the fix window (Alt-5) while the system is young.
- Do these two things first, while you read `heuris.tex` in parallel.

**Done when:**
- **Files:** you put coursework and notes in `~/files`, never in `~/local-projects`.
- **One tree:** the laptop tree and the tablet tree have the same shape, under the same names.
- **New folders:** a new folder on either side is either mirrored to the other or put to you as
  a question.
- **The chat:** Home has a chat bar you use for anything that needs your whole life as context
  but no single project.

---

## Decisions already taken

- **D1, no spaces in names.** Your words, 9 Nov: "I do not wanted spaces in names anywhere on my
  laptop... the shape of the tablet and computer trees are identical, and they are perfectly
  linked... if the remarkable folders should be renamed that is fine. We also need a way to
  handle new folders on both."
- **D2, a native chat bar on Home,** with its own view on its own key (9 Nov).
- **D4, no deep project work in the chat** (9 Nov, agreed), so C3 is decided.
- **D3, the chat acts as well as answers.** Your words, 9 Nov: it should "help me work within
  locus, ie. help me create new projects to be in locus or write tasks/ plan events/ schedule
  time/ write email drafts/ sort my notes... the home for short tasks/ questions that need full
  context about my life but do not necessarily have one specific project".

## Decisions for you (my recommendation first)

- **F1, one folder name on both sides** `[proposal]`. The registry gains `folder`, a lowercase
  hyphenated name (`b1-engineering-computation`, `brevan-howard`, `quast`). The tablet's
  level-2 folders are renamed to it, so both trees use the same names.
  - The alternative is to keep Canvas names on the tablet with a mapping. That means two names
    for one thing, and a mapping that can drift.
- **F2, keep the area folders as they are** (`Studies Projects Careers Personal`) on both sides
  `[proposal]`. They have no spaces, and renaming them touches every capture path for nothing.
- **F3, mirror folders only, not file contents** `[proposal]`.
  - The tablet holds what you read and write by hand; `~/files` holds what you open on the
    laptop.
  - Copying content both ways ("pdf of everything together back in files", heuris.tex B) is
    heuris's job, and waits for it.
- **C1, conversations are yours to start and return to** (9 Nov: "I should be able to start new
  conversations myself for different tasks, and view old ones... should be easy").
  - **Default:** the bar continues the conversation you used last. Nothing is started
    automatically.
  - **Long conversations:** once a conversation is long, the bar says so ("long conversation ·
    n for a fresh one"). An endless conversation gets slower and costs more each turn, so you
    decide when to start over.
  - **What lasts:** anything that must outlive a conversation goes where it belongs: registry,
    kairos, keryx, `~/files`, and later heuris's `remember`.
- **C2, every change it makes is listed under its reply,** with how to undo it `[proposal]`.
- **C3, it hands deep project work to that project's session** (agreed 9 Nov, D4). It can read every
  repository, but it does not code in one. When a question turns into project work, it says
  which tmux session to take it to.

## M1 Files

1. **Folder names in the registry.**
   - **Change:** `folder` is added to every entry. It must be unique within each area, and a
     test enforces that. For an application, it is the company's name.
   - **Check:** the test lists every folder name, and fails on a duplicate or a space.
2. **Capture matches on `folder`.**
   - **Change:** `capture/device.py` matches a level-2 folder on `folder`. It still matches the
     old names, so a folder not yet renamed keeps working. Deployed to the server.
   - **Check:** `locus status` on the server shows no unmatched folders before the rename, and
     none after it.
3. **Rename the tablet's level-2 folders** to `folder`, one `rmapi mv` each, paced against the
   429s.
   - **Safe for marks:** marks follow a document by its uuid, so nothing is lost.
   - **Check:** a listing of `/Studies /Projects /Careers` printed before and after, plus one
     capture tick with no new unmatched folders.
4. **`~/files` on the laptop.**
   - **Change:** the same tree, made from the registry.
   - **Moves:** `b1-practical` goes to `Studies/b1-engineering-computation/`, and
     `b2-tech-strategy` goes to `Studies/b2-engineering-in-society/technology-strategy/`.
   - **Their chat histories move too:** Claude Code files a chat under its folder's path, so
     without this, `claude --continue` there would find nothing.
   - **tmux:** the `b1-numerical-analysis` session is pointed at the new path.
   - **Check:** the tree is printed; `claude --resume` in the moved folder lists its old
     sessions.
5. **Keeping the trees in step** (`locus files sync`, on the capture timer and on demand). Both
   trees are compared below the area folders:
   - **A folder deeper than level 2,** new on one side, is made on the other.
   - **A new level-2 folder,** new on one side, becomes a question for you, in Approve and in
     the chat: register it as a course, project or company? On yes, the registry entry and the
     other side's folder are made.
   - **Nothing is ever deleted or renamed by sync.** A rename shows up as one folder gone and
     one new, and is put to you.
   - **Check:** a scratch folder made on each side appears on the other within one tick.
     A scratch level-2 folder appears as a question.

## M2 The Home chat

1. **The bar.**
   - **Layout:** one line pinned to the bottom of Home, showing the current conversation's name
     and its last reply.
   - **Space** focuses the bar from anywhere on Home. Enter sends, and Esc leaves the bar
     without sending.
   - **Tab keeps its meaning** (9 Nov: Tab behaving differently here would be wrong).
   - **Check:** a dump of Home with the bar.
2. **The chat view, on `i`** (free on every screen today).
   - **Layout:** the conversations listed down the left, newest first. Each is named from its
     first message, and you can rename it with `r`.
   - **Right side:** the selected conversation in full, with its changes list (C2) under each
     reply.
   - **Keys:** ↑/↓ choose a conversation. Space or Enter types into it. `n` starts a new one.
     `o` opens it in full Claude Code, in a new window of the locus session. Esc goes back to
     Home.
   - **Check:** a dump of the chat view with three conversations, one of them reopened.
3. **The engine.**
   - **Each message is one turn:** `claude -p --resume <id> --output-format stream-json`, with
     its working directory at `~/local-projects` and its brief in
     `crates/locus-tui/chat/brief.md`.
   - **Plan:** it runs on your subscription, because it is interactive.
   - **Streaming:** the reply streams into the view as it arrives.
   - **Check:** one question answered from two different repositories' histories.
4. **What it may do (D3), set by permission rules** rather than trust:
   - **Read:** read and search anything under `~/local-projects`, `~/files` and the Claude
     histories.
   - **kairos and keryx:** their MCP tools. kairos queues its risky changes for Approve itself.
     keryx only drafts and never sends.
   - **Files and the registry:** edit and move inside `~/files`, and edit
     `taxonomy/registry.toml`. Both changes appear in C2's list.
   - **Everything else is refused,** and it tells you to press `o`.
   - **Check:** one asked-for task in kairos, one draft in keryx and one note moved in
     `~/files`. A refused edit inside a repository says `o`.

## Departures from the plan

- **The Project view (Slice 3) is set aside.** The whole-life chat takes over what its ask line
  was for.
- **The notes repository is `~/files`, as heuris.tex B already says,** with one change: the
  tablet's folder names change to match it.

## Not in this

- Content sync between the tablet and `~/files`.
- Indexing `~/files` for search.
- The heuris tools in the chat.
- Mail-derived facts.
- Home going live.

All of these come after this spec, in the order you settle once you have read `heuris.tex`.

## Report

*(written when it closes)*
