# Usability backlog (his notes, 18 Nov 2026)

Gripes from using locus day to day, written down by Alec on 18 Nov. **Not to be actioned yet**:
they wait until what is on the plan is done (the OXDAQ plan and cpp-oxdaq curriculum, L4 and L5,
the nightly reading digest, the closing pass). Then they are the next round of small fixes.

His overall verdict, for context: "The system is gradually getting better and better and closer
to what I envisioned ... I am definitely using kairos, and I do enjoy looking at home and at
studies and I do find it useful." He works mostly through Claude Code, not claude.ai, so the
size of the one locus MCP's tool list matters less.

Each item: what he said, then what it probably involves (a note for later, not a decision).

---

## Home

### 1. The projects list: nicer, consistent titles, and control over what shows

> "I do not really look at the projects list, and find the red to be misleading as something
> that is 'wrong', I want the projects list to be there, as I like seeing the different tracks/
> projects that I currently have going on, but I want some way to make it look nicer and better
> control the display title so they are a consistent format and which ones are shown."

- The red reads as an error. Find what it encodes (stale? no sessions?) and use a neutral mark.
- Display titles: one consistent format across entries (see item 2), probably a short display
  name in the registry separate from the long name.
- Which ones show: a per-entry switch (the registry's `active`, or a `home` flag) he can set.

### 2. One pithy project name, used everywhere, linked to its module

> "3YP is different from B3 Group design project which is different from med-ml-3yp and they all
> describe the same thing, so I want a better project title that includes the actual project
> name in a pithy way that is used across the board (except for B3 group design project in files
> and studies) as that is the module title, but the project should be linked to it like out of
> bits is linked to GR AI/ML."

- The module keeps "B3 Group Design Project" (Studies, files). The project gets one short name
  that says what it is, used in Home, kairos, keryx, sessions and folders, linked to the module
  the way `out-of-bits` links to `gresearch-ml` (registry `course = "b3"` already exists).
- Same audit for every entry with more than one name (the 18 Nov Out of Bits rename is the
  pattern: registry, kairos, heuris, keryx links, fixtures).

### 3. Careers and studies: his favourite part; fill the gaps with more of it

> "My favorite part of home is the look, polish and layout of the careers and studies section,
> I am a big fan, but there is often a gap between learning and tasks (or cv and approve), which
> should not be there, and there should instead be more rows of studies and careers content
> being shown."

- The layout leaves empty rows between sections at some heights. Let careers and studies take
  the spare rows instead (a layout rule, checked at several terminal sizes).

### 4. The Home chat: a new chat by default

> "trying to chat from home continues in the last chat when it should start a new one unless I
> go into the conversations view and explicitly want to continue in a specific chat, as this
> matches the quick questions nature of the question box. I also wonder whether the
> conversations view have one pane just actually be a claude code session that you select in
> the left hand bar ... the custom interface feels a little clunky, but maybe I can get used to
> it."

- Quick question box: a new conversation each time. Continuing is an explicit choice in the
  conversations view.
- To think about: the conversations view's right pane as a real Claude Code session (a tmux
  pane running `claude --resume <id>`), picked from the list on the left, instead of the custom
  chat renderer. Open question, not a decision; he may get used to the current one.

---

## kairos

### 5. Moving tasks quickly: lag, and the selection jumping

> "kairos is starting to be really good, but sometimes when I move tasks around quickly they
> lag, or my selection jumps to a different thing and I end up annoyingly moving something that
> I did not want to."

- Likely the held-keys write (`HOLD_QUIET`, 400 ms) racing the 3 s index poll and the replan:
  the selection follows a uid that a replan replaced, or lands on the item that moved into its
  place. Reproduce with fast ⇧-arrow runs; the selection must never change item between keys.
  This one is a correctness bug, not polish: it moves the wrong thing.

### 6. Time tracking: everything, not just projects, and project time seems wrong

> "The project work tracking view should be extended to any planned kairos time and claude
> sessions for those in a unified view. I want to be able to see how much time I spent on
> certain problem sheets/ projects/ notes etc and when, not just projects, and I feel the
> project time tracking is also not working properly."

- One view of time spent: checked-in kairos blocks of any task (sheets, learning topics, notes)
  and measured Claude sessions, by item and by day, for any registry item, not only repos.
- First check why project time looks wrong (measured spans per machine, the 10 minute quiet
  rule, sessions in folders that do not match a registry entry, the out-of-bits rename).

---

## Tasks and Approve

### 7. Learning sessions swamp the task list

> "approve and tasks are fine, with an issue being that the learning plan sessions DOMINATE the
> task view massively, to the point where I cannot scroll to see anything else, especially when
> I did not plan those and there are so many of them."

- 32 interview-prep topic tasks (more with cpp-oxdaq and quast). Group them: one row per
  learning goal ("interview-prep: 3 sessions this week, next Fri 15:10"), expandable, or a
  filter that hides them by default. Tasks he made himself come first.

---

## keryx (mail)

### 8. Too busy and unpolished; he still uses Mail

> "I still think keryx is too busy and unpolished so I have not taken to using it fully yet, and
> still end up using mail fairly frequently. Mainly the personal/ oxford/ oqts/ oqts-shared
> split and what ends up in approve (why is oqts-shared ending up in approve...) is the main
> issue, along with some ui being too dense and crammed issues, or like I try to mark something
> as for and it does not work properly and drops my assignment etc - polish things like that."

- The account split: rethink how the four accounts are shown (one inbox by default, accounts as
  a filter?).
- OQTS shared in Approve: shared mail is read-only in keryx and every Claude write there needs
  approval, so cleanups propose it. Probably exclude OQTS shared from cleanups altogether.
- Density: a pass on spacing and what each row shows.
- `f` / `m` (what it is for) dropping his choice: a bug to reproduce and fix.

---

## Everywhere

### 9. Add, edit and delete things himself, not only through Claude

> "whenever I want to add something new like a new project or a new career thing or a new
> learning goal etc, I feel like it needs to happen through claude, and be big and bulky editing
> code sometimes ... the option to add edit and delete things myself, (and expose claude simply
> to those tools) would also be good."

- Forms in the locus window for registry entries (projects, applications, courses), learning
  goals and their pace, and facts; the same operations as small CLI commands and MCP tools, so
  Claude calls one tool instead of editing `registry.toml` and its fixtures by hand.
- The registry is a committed TOML file today, so a form must write it, commit it, and update
  the fixtures the window's check needs; or the registry moves to data with the TOML as an
  export. A design question for when this comes up.
