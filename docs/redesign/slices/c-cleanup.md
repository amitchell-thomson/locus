# C: cleanup (less, simpler, and safe when the day changes)

**State:** approved 13 Nov 2026 with amendments: C5 (the live day) is parked ("I need to think about that some more"), except D3a, which is yes; D3b is paused with it. The System and Budget page stays and becomes live ("it should properly show real data as that real data exists"). D5 goes with the Canvas pull, as recommended · **Written:** 2026-11-13 · **Runs in:** `claude` (locus-dev)
**From:** your 13 Nov message: "before we implement anything new, I would like to tackle messiness
and overcomplication, as those are the most pressing problems with the system", including "an
analysis of edge cases and potential uses that have not been considered, especially ... when I
would want to move events/task blocks very dynamically as my day changes live", and "how handling
emails looks from the users perspective, especially reading threads, as the current message
highlighting scheme and thread scrolling is really unintuitive and difficult". The Project window
goes ("I agree project can go").
**Builds on:** H2 (closed 13 Nov). **Comes before:** the Canvas pull (moved ahead of L, your call
13 Nov), then L.

**Done when:**
- one set of keys means the same thing in Home, the calendar and mail, and every window's help
  is true;
- the Project window and the old engine's dead code are gone, and System and Budget show live data, every remaining test passes, and
  nothing live reads a table nobody writes;
- a late start, a block done early, an unplanned two hours and a cancelled lecture each take one
  action, and none of them can touch a whole lecture series by accident;
- a thread reads like a document: what you see is what `⏎`, `z` and `r` act on;
- a rebuilt binary reaches the windows without you restarting them.

---

## What I found (four read-only audits, 13 Nov)

**Code nobody runs.** `locus/` is 45,241 lines of Python, and about 14,700 of them are reachable
only through three stray imports. One of them means every heuris `search` loads the old daily-page
engine. Also:
- 37 of 64 CLI commands are dead (`daily`, `decide`, `link`, `promote` and others).
- 28 test files (10,296 lines) test dead code, and 14 of the 16 failing tests are among them.
- Two timers run for nothing. `discover-pull` ran 704 times in 30 days and found nothing. `maintain`
  bills a nightly `structure` pass whose tables nothing reads.
- About 270 of CLAUDE.md's 811 lines describe removed things, and heuris is missing from it.
- One silent failure, the §3 shape. The library's entity arm has missed every document since 19
  Oct: aliases were last built then, so 8,787 entities from the 121 new documents are never
  matched, and nothing says so.

**locus-tui.** It has about 197 key bindings, kairos about 70 and keryx about 107. The same action
often has different keys in different apps:
- undo is three different behaviours;
- edit is `e`, `c` or `r` in Home and `e`, `K` or `L` in the calendar;
- `v` means select in one app and details in another;
- `m` has four meanings, and `s` and `i` three each.

Within the screens themselves:
- Home quietly overrides its own section letters, and the help is wrong in places.
- The Project window and its sub-screens are about 2,800 lines of Rust and 3,100 lines of sample
  data.
- Search, System, Review, Learning, Explore and Prep still show sample data. So do three things on
  live Home: "3 jobs running", "weekly review ready", and quast marked as attended.
- `R` reloads the sample data over your live Home.
- A rebuild reaches no window until you restart it by hand.

**The calendar when the day changes** (read from the code and today's log):
- On 13 Nov, moving the B18 block from Sunday to Friday took 9 separate changesets, and it ended up
  pinned over the B1 practical.
- **A hazard: moving or stretching one lecture of a series moves the whole series.** It applies at
  once, because your own edits skip approval. `update_op` always edits the series' master event,
  and nothing stops it for a recurring event. "Not going" and delete on one lecture likewise act on
  the whole series.
- A late start has no single action: each block moves on its own, 30 minutes at a time, and every
  block you move becomes pinned.
- Finishing early doesn't free the rest of the block. Freed time only flows into work 2 hours or
  more ahead, so a cancelled 14:00 lecture stays empty.
- Through Claude, anything within 2 hours waits in Approve, so "I'm with a friend until 5" stalls.
- From the phone you can't check in, skip, start now or undo: kairos is not reachable through the
  claude.ai connector.
- A replan reports only counts, never which blocks moved where.
- Home refreshes every 60 s, so it lags the calendar by up to a minute.

**Reading mail:**
- The thread scrolls (space, PgDn) separately from a selection (`j`/`k`). The selection is a tint
  on one header line, which scrolls out of sight while you read. `⏎`, `z` and `r` then act on a
  message you can't see, and the view jumps back to it.
- `j` jumps past the rest of a long message.
- `r` always answers the account's latest message, not the one you're reading. In a thread merged
  across accounts, that can be an older message from another account.
- "Read" is stamped when a thread opens, so opening one and moving on loses its new messages.
- The "new" band sits above every message instead of between new and old.
- **Sending is not yet trustworthy:**
  - The confirm screen previews a `>` quote that is not sent, and never shows the quote that is.
  - It doesn't say which message you're replying to.
  - Its warnings are cut off and it can't scroll.
  - After a send you're back in the list, without seeing your message in the thread.
  - Claude's drafts are labelled "you".
- **Traps:**
  - The draft band says "g d opens it", but in a thread `d` files the thread.
  - With rows marked in the list, `d` or `a` inside a thread acts on the marked rows instead.

---

## Decisions for you (my recommendation first)

**D1. One set of keys for all three apps.**

| Action | Key | Today |
|---|---|---|
| back / quit | `esc` back; `q` quits only at the top | `q` is back in some places; `h` is back in keryx |
| undo | `u`, at once, with a line saying what was undone; on every screen | three behaviours |
| edit | `e` (opens the form; Tab reaches every field) | `e c r` / `e K L` |
| approve / refuse | `y` / `n` | same |
| new | `+` | same |
| done | `d` | same |
| remove / skip | `x`, asks once | same |
| mark several | `v` | kairos uses `v` for details: that moves to `⏎` |
| search / filter | `/` | same |
| refresh | `^r` | `r` in kairos, none in locus |
| go to | `g` then a letter (`g a` approve, `g t` tasks, `g m` mail, `g c` calendar, `g h` home, `g l` log) | capitals in kairos and keryx, letters on Home |
| help | `?`, generated from the same table the keys are read from | hand-written lists, wrong in places |

All three apps read the table from `locus-theme`. Each app's own keys stay only where nothing in
the table covers the action: day and week movement in kairos, reply in keryx. Section letters on
Home go; you move between sections with Tab or arrows, which Home already does.

**D2. Delete, don't hide.**
- The Project window and its sub-screens go, along with the screens that only ever showed sample
  data: Search (`/` becomes the Home chat's search), Review, InkConfirm,
  Prep, Decisions, Changes, Answered and Explore.
- Learning stays as a stub for L.
- The sample data goes from the live binary (`R`, `--fixtures`).
- Alt-2 becomes the calendar and Alt-3 mail.
- Nothing is lost: it is all in git. The Explore redesign is noted below as a todo.

**D3. The calendar's "live day" actions** (all in kairosd, so the TUI and Claude get the same):
1. **Slip.** One key, with the minutes typed or chosen (5, 10, 15, 30). It moves the rest of
   today's unstarted work blocks, and optionally your own one-off events, as one changeset that
   stays unpinned, so the plan can still move them.
2. **Stop here.** On a running block: trim it to now, record the minutes actually worked, and let
   the time that frees up be replanned from 15 minutes ahead, not 2 hours.
3. **Not today.** On a block or a task: it isn't planned before tomorrow. This makes "not before"
   editable on any task.
4. **Freed time fills.** A cancelled or "not going" event in the next 2 hours lets work flow in
   from 15 minutes ahead.
5. **One lecture, not the series.** Move, stretch, "not going" and delete act on one occurrence.
   Until that is built, they are refused on a recurring event with a message saying so. **This is
   the first step of the slice, because today it can move a whole series.**
6. **Moves you can see.** After a replan, the blocks that moved show their old time for a few
   seconds. A run of moves on one block within a minute is one changeset and one undo. ⇧ moves in
   15-minute steps, not 30.
7. **Home on the write.** Home refreshes when kairos writes, not once a minute.

Two calls that are yours, because they change the risk policy:
- **D3a.** Let Claude make, on your request, a one-off event within 2 hours that clashes with
  nothing, and same-day moves of your one-off events. Lectures, other people's events and
  anything recurring stay gated. *Recommended: yes*, since "if I ask for something it should just
  happen" (10 Nov).
- **D3b.** Put kairos's check-in, slip, stop-here and start-now tools on the locus connector, so
  the phone (claude.ai) can use them. *Recommended: yes.*

**D4. Read a thread like a document.**
- You scroll; there is no separate selection.
- The message at the top of the view is the one `⏎`, `z` and `r` act on, marked by a bar down its
  whole left edge.
- `j`/`k`/space scroll, and space runs on into the next message. `n`/`p` jump between messages.
- **On opening a thread:**
  - The newest unread message is at the top, open.
  - Up to two more new messages are open.
  - Older messages fold to one line each: who, when, first words.
  - One divider separates new from read.
  - The header shrinks to two lines while you scroll.
- "Read" is stamped when you leave the thread, and only for what was on screen.
- `r` replies to the message you're reading, across accounts, and says so: "replying to Milly,
  2 Nov 13:35".
- **The confirm screen shows exactly what will be sent:**
  - the body and the real quote;
  - your shown address;
  - wrapped warnings;
  - scrollable.
- **After a send:**
  - You stay in the thread and see your message, with "sending · c cancels · 8 s" until it says
    "sent".
  - Cancel gets its own key, so `u` is only undo.
- Claude's drafts say "Claude's draft", and `r` on a thread with one opens it rather than starting
  a second.

*How:* another chat is building keryx's thread view right now. This step goes to that chat
as a written brief, or waits until it is done. It is never done over its uncommitted work.

**D5. The library merge.** heuris searches the library through the old retrieval code, which is
why Q10 failed. *Recommended:* in C, cut the old arms that silently fail (the entity arm) and the
stray imports. Index the library into heuris itself together with the Canvas pull, which adds
library documents anyway and is the natural place to do it once.

**D6. A docs layout for every repo:** `README.md`, `CLAUDE.md` and `state.md` (where it stands,
what was decided, what is next, about 20 lines). The global CLAUDE.md gets one line: "at the end
of a session that changed direction, update `state.md`". *Recommended:* yes, as the last step of C.
Each existing repo gets its first `state.md` written by a chat in that repo.

---

## Steps

Each step ends with its check on real data, and is committed and deployed before the next.

**C1. The lecture-series hazard.** Refuse move, stretch, "not going" and delete on one occurrence
of a recurring event, from the TUI, CLI and MCP, with a message saying so. *Check:* an engine test
on a weekly series. On the live calendar, ⇧↓ on a B15 lecture is refused, and the series is
unchanged.

**C2. Cut the stray imports, then delete the dead Python.**
- Cut the three imports, then delete the dead CLI commands, modules, tests and config sections.
- Turn off and remove the `discover-pull` and `maintain` timers and the units already disabled.
- Remove the stale stdio `locus mcp` registration in the quast project.
- Remove the library entity arm, which has failed silently since 19 Oct.
- Fix the one live test that cannot run on macOS.
- Give pytest a `testpaths`.

*Check:*
- the full suite passes with 0 failures;
- an import-reachability script shows no module unreachable from a live root;
- `search`, `open`, `context`, capture and watch each run on the server;
- the H1 and H2 answers that cite library documents (Y3, Y9, 10, 21) still find them.

**C3. Delete the Project window and the sample-data screens** (D2); **System and Budget made live** from the server's real data (units, runs, the credit ledger, model calls). Window renumbering, the tmux
script and the fix chat's brief all change with it. *Check:* the windows open; every Home key
works; `--dump` of each remaining screen; no sample data reachable from a live binary.

**C4. One key table** (D1) in `locus-theme`, used by Home, kairos and keryx (keryx's part as in
D4's "how"). Help is generated from it. *Check:* a test that no app binds a table action to
another key, and that every help line names a key that works. Then a walk-through with you.

**C5. Parked (13 Nov), except D3a.** Claude may make, on your request, a one-off event within 2 hours that clashes with nothing, and same-day moves of your one-off events (`risk.rs`). The rest below waits for your rethink.

~~The live day~~ (D3 1 to 7). One occurrence of a series becomes
editable through a RECURRENCE-ID override. *Check:* on the test calendar first, then on today:
- slip 15 minutes;
- stop a block early and watch the freed time fill;
- cancel a lecture occurrence and watch work flow in;
- ask Claude from the phone to push a one-off event by an hour.

**C6. Reading and sending mail** (D4). *Check:* snapshots of a long thread, a merged thread and a
thread with a draft. Then you read three real threads and reply to one through the confirm screen.

**C7. Rebuilds reach the windows.** Each binary execs itself again when its file changes, while
nothing is being typed. *Check:* rebuild Home while it runs; it restarts in place, keeping the
screen.

**C8. CLAUDE.md and the docs layout** (D6). CLAUDE.md is rewritten to what is true: heuris, the
three apps, the windows; the engine's history stays in git. *Check:* every section names code that
exists (a script greps each path it mentions).

---

## Edge cases and uses not yet considered

**The day:**
- A lecture overruns.
- A tutorial moves by email. Does keryx's suggestion move the event, or add a second one?
- Two blocks are swapped.
- A block is worked during a lecture: the hour is counted twice today.
- A trip with travel time.
- An all-day event, such as a trip, that should clear the day's work.
- Illness: "nothing today, replan the week".
- A deadline extended by email.
- A task that turns out bigger halfway through ("longer", from the phone).
- An evening with a check-in you never answer. Today it is presumed done for planning at once,
  and recorded after 7 days.
- Daylight-saving week with recurring lectures.
- Working from the phone with no laptop for a weekend.

**Mail:**
- A reply-all on a list.
- A thread that forks (someone replies to an older message).
- The same message in two accounts.
- An email with only an image.
- A long newsletter opened by mistake.
- A forward.
- A thread where the question is in an older message than the newest.
- A send cancelled in the hold.
- A send that fails after the hold: today it says "not sent" once and nothing else.
- A draft started on the phone in Mail.

**Facts and memory:**
- A reschedule email should replace an interview fact; today it doesn't (38).
- A firm's name used as the subject (`facts("brevan-howard")`).
- A fact you said in the Home chat while hypothesising.
- Two people with the same first name (Q46).

**The windows:**
- The laptop is offline: each window says so and still shows the last data.
- The server is down.
- A window is too narrow.
- Someone else's window, such as the fix chat, is rebuilding the binary while you use it.

Each of these goes into the step that owns it as a test or a check, or into "Not in C" with a
reason.

---

## Not in C

- **The Canvas pull** (next): course sheets, notes and lectures appear in `~/files` and on the
  reMarkable by type. This closes the gap between the tablet and `~/files` (B18), and adds the Year
  1 and 2 syllabi and Q31.
- **L** (learning), after the Canvas pull.
- **Todo, not designed yet: Explore as a nightly reading digest** (your 13 Nov idea). About 30
  minutes of curated reading, chosen to:
  - probe a new idea or teach something;
  - suggest extensions to your projects and fixes to their shortfalls;
  - suggest plans in life;
  - be food for thought before sleep.

  You write notes on it. The ideas it raised and the ones you noted are saved and come back later.
  It is set aside until it gets its own spec. It will build on heuris (facts, records, briefs) and
  on the tablet loop (`to_remarkable`, `markups`).

## Risks

- **Deleting something live.** The reachability script, the full suite and a run of every live
  command on the server come before each delete. Every unit change goes through `deploy.sh`.
- **Two chats in one checkout.** Stage files by name; never stash; keryx's thread work goes to its
  own chat.
- **The key change is a habit change.** One walk-through with you before it ships. The help is
  generated, so it cannot drift.

---

## Progress (13 Nov, evening)

- **C1 done.** The kairos window refuses to move, stretch, delete or mark "not going" one lecture
  of a series (`SERIES_ONLY`, test `kairos-tui/tests/series.rs`), and a series change proposed by
  Claude says "(every occurrence)". **D3a done:** `risk::is_live_event_op` (policy table updated,
  MCP instructions and SPEC say so).
- **C2 done.** 33,701 lines deleted (Python under `locus/` 45,241 → 23,590); 29 CLI commands
  left; 0 modules unreachable from the live roots; the suite is 770 passed, 0 failed. The
  discover-pull, maintain, daily, daily-pull, discover-harvest and receiver units are removed from
  the server; five timers remain (backup, capture, watch, heuris, heuris-derive). The library's
  entity arm is gone: `search` now finds the GPTQ reading notes Q10 missed. Kept on purpose:
  `observe/gates` (heuris records to it), `vault`'s writer (capture's Related block),
  `enrich/related` (now on library search + rerank), `eval/metrics` (audit, inspect).
- **C3 done.** locus-tui lost the Project window and every sample-data screen (Project,
  Decisions, Changes, Answered, Application, Prep, Explore, Review, Search, InkConfirm): about
  13,900 lines from the two crates. System and Budget are live from `locus home-system` (timers
  with last result and next run, the credit this cycle, today's allowance, each day by job, this
  cycle's jobs). ⏎ on a project switches to its tmux session; ⏎ on an application starts a Home
  chat question; `/` opens the chat; `R` and `W` are gone. Windows are 1 home, 2 calendar,
  3 mail, 4 fix.
- **C7 done for Home** (68e3846): a rebuilt binary re-execs itself in place on the same screen
  once nothing is typed (checked in a scratch tmux session). kairos and keryx get the same when
  their chats are free.
- **C8 done** (463c88b, 258a6a5): CLAUDE.md 811 → 450 lines, every path and command it names
  checked by a script (174 of 174); the `state.md` template in `docs/templates/`, and the line in
  the global CLAUDE.md on both machines.
- **C4 done** (merged and deployed 13 Nov, late): Home, kairos and keryx all read
  `locus_theme::keys` first (esc back, q quits only at the top, u undo, e edit, y/n, +, d, x asks,
  v, /, ^r, g + a letter, ?); help is generated from the table; each app's tests check its own
  keys are free in the table and press every key its help names. Moved keys: kairos `v` details
  -> ⏎, start `n` -> `s`, share `s` -> `F`, `r` -> ^r, capitals -> `g`; keryx filing `d` -> `M`
  (`d` is done: files for the mark or keryx's guess, saying why), `h` is no longer back, next and
  previous message `] [`. The walk-through with Alec is still owed.
- **C4 adjusted after Alec tried it** (13 Nov, night): "the g - something keys feel weird,
  across the board, it only really made sense in mail" and "now navigating around home only
  works with tab and arrows". `g` left the shared table. Home's section letters and corner
  letters are back, on letters the table leaves free (system `y` -> `m`; on System recent
  `e` -> `h`, usage `u` -> `b`, and `b` opens the budget); `l` is the log again. The calendar's
  `g` places went; `A` `T` `P` (and `L` for the log) open Home's screens. keryx keeps `g` for its
  lists; `Action::GoTo` and `keys::PLACES` stay as a shim until keryx stops naming them.
- **C6 done** (D4): a thread is one scroll with a bar on the message acted on; it opens on the
  newest new message; read is stamped on leaving and only for what was seen; `r` answers the
  message read (`SendReq::in_reply_to`, keryxd threads to it); the confirm screen shows the real
  quote, the From address, wrapped warnings, and scrolls; after `y` you stay in the thread
  ("sending · c cancels"); Claude's drafts are labelled so. keryxd's `settle_send` now knows
  `Sent` and `Cancelled` (the line never said "sent" before). Still owed: Alec reads three real
  threads and replies to one through the confirm screen.
- **C7 done for all three** (kairos via `locus_theme::rebuilt`, keryx its own `rebuilt.rs`).
  Each window needs one manual restart to get onto a build that has it.
- **Leftovers done:** keryx/CLAUDE.md says Alt-3; the quast project's stale stdio `locus mcp`
  is removed; `locus status` shows spend against the credit's pace, not "$5.00 cap"; kairosd's
  "(every occurrence)" has an engine test; two keryxd tests stale since e26b8b6 (Admin and Travel
  became streams) updated; five keryx files rustfmt'd, so `scripts/ci.sh` exits 0.
- **Not done, kept:** `scripts/` holds ~32 analysis scripts that import deleted modules (evidence;
  they no longer run).
