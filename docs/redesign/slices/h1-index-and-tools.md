# H1: the index and the tools

**State:** written, waiting for your approval · **Written:** 2026-11-10 · **Runs in:** `claude` (locus-dev)
**From:** heuris v2 (accepted 10 Nov), section I, row H1; sections C and D; ADR-011.
**Builds on:** Slice 0.6 (the one HTTP service), 0.9 (the scrubber), 0.10 (laptop chats on the server).

**Done when (from heuris):** every question in the test set that the data can answer is answered
well from a fresh chat, with no pasted context. "Well" is scored on three things: the right
current state, the right sources found, nothing invented.

**What H1 is not.** No records, facts, briefs, `remember` or `forget`: those are H2. So `context`
in H1 is the plain version (today, live projects, applications, what happened recently), and the
questions that need a fact ("am I still waiting on Jump?") are scored in H2, not here. H1 spends
no API credit: indexing runs on the server's GPU with the nomic model the library already uses.

---

## Where the data is today (measured 10 Nov)

| Kind | Where it lives | Size |
| --- | --- | --- |
| Mail | **the laptop**: keryx's mirror, `~/.local/share/keryx/keryx.db` (keryxd runs on the Mac only) | 7,677 messages: personal 3,532 · oxford 3,026 · oqts-shared 1,047 · oqts 72 |
| Calendar, tasks | the server: kairosd and its vdir (Family included) | about 380 series, 41 tasks |
| Claude chats | the server: `data/transcripts/laptop/` (pushed, scrubbed) and its own `~/.claude/projects/` | 358 laptop files, 1.1 GB |
| Repositories | GitHub; clones on both machines | the registry's repos |
| Sources, ink | the server: `locus.db` (the library) | 232+ documents, 752 marks |

keryx already has MCP tools that do what heuris section D asked of it: `list_inbox`,
`read_thread`, `tag_needs_reply`, `draft_reply`, `draft_email`, all behind its rule that Claude
never sends. They are bound to `127.0.0.1` on the Mac and registered nowhere.

---

## Decisions for you (my recommendation first)

- **D1, one new database for the new kinds; the library stays where it is** `[proposal]`.
  - `heuris.db` on the server holds mail, calendar, tasks, chats and repository items, each with
    its vectors and full-text index.
  - Sources and ink stay in `locus.db`. `search` asks both and reranks the results together with
    the library's cross-encoder.
  - Moving the library into `heuris.db` (heuris G) waits until the drop list is settled, which
    needs the propositions measurement. Doing it now doubles H1 for no gain in answers.
- **D2, mail reaches the server as a filtered, scrubbed copy, pushed every 15 minutes**
  `[proposal]`.
  - The same shape as 0.10: changed messages from keryx's mirror go through the scrubber, then
    rsync to the server.
  - The OQTS-shared mailbox is filtered out **on the laptop**, so it never leaves it (test
    question 47).
  - keryx's needs-reply tags and its filing links (which project a thread was filed under) go
    with the messages; the links are what let mail be found by project.
  - The alternative, indexing on the laptop, has no GPU and stops when the Mac sleeps.
- **D3, which mail is kept is set from your real mail, not guessed** `[proposal]`. This is what
  "decided in H1 by looking at real data" meant (you asked me to explain it better). I start
  from this rule:
  - **kept**: mail from a person; or anything you sent, replied to or flagged; or anything from a
    firm in your applications file, or from a recruiting system (Workday, Greenhouse, Lever and
    the like), because offer letters come from those;
  - **left out**: mailing lists and automated mail, read from the list headers and the sender.

  Then I show you 30 real messages the rule kept and 30 it left out, with the reason for each,
  and you mark the wrong ones. Five minutes of your time settles the rule on evidence. The same
  look settles how quoted replies and signatures are stripped from a thread (you see five threads
  before and after).
- **D4, the phone and claude.ai reach the service through Tailscale Funnel, with the login
  claude.ai accepts** `[proposal]`. Step H1.1 checks which login that is.
  - If it accepts a fixed token, the existing bearer token is used.
  - If it needs OAuth, which I expect, the service runs the single-user OAuth flow that the MCP
    library already supports. Its approval page asks for a passphrase that only you know, kept
    in the Keychain and on the server.
  - The alternative is to leave the phone until after H1, but Y-questions from the phone
    (test question 40) are part of what H1 is for.
- **D5, keryx's tools are registered as they are, on the laptop** `[proposal]`.
  - keryxd's own MCP gets registered at user level on the laptop, so Claude Code there can list
    what needs a reply, read a thread and draft a reply.
  - It already refuses to send, approve, delete or reach kairos, and every mail it returns is
    wrapped as untrusted data.
  - From the server and the phone, mail is **read** through `search` and `open`. **Drafting**
    from the phone waits, because keryxd runs only on the Mac. Test question 22 is scored from
    the laptop.
- **D6, the old tools leave the service** `[proposal]`.
  - Retired: `retrieve` and `query` (replaced by `search`), `list_documents` and
    `inspect_document` (replaced by `open`), and `critique`, `synthesise`, `objects` and
    `evolution` (on heuris G's drop list).
  - Kept: `capture` (the locus-capture skill uses it), `markups` and `to_remarkable`.
  - The code and tables behind the retired tools stay until the drop list is decided. Only the
    tools go, so every chat sees one way to find things.
- **D7, one line in your global CLAUDE.md** `[proposal]`, from heuris D. It says:
  - call `context()` first when a question touches your life, projects, courses or
    applications;
  - then use `search`, `open` and `timeline`;
  - cite notes like references, as in heuris D.

  Your global CLAUDE.md is your file, so I change it only with this approval.
- **D8, how the test set is scored** `[proposal]`.
  - I sort the questions into three groups: answerable in H1, needs H2 or L, and no data.
  - For each answerable one, I draft the line on what a good answer contains, and you correct
    those lines.
  - Each question is then asked in a fresh `claude -p` with only the locus tools (on your
    subscription, not the credit). I score it on the three criteria and you spot-check ten.
  - The alternative is that you write every line yourself; that is about 35 lines.

---

## How it works

**An item** is one findable thing with a pointer that `open` can follow. The exact fields are
settled in H1.2 against real items; the starting set is:

| Field | What it holds |
| --- | --- |
| `kind` | mail, event, task, chat, commit, doc (a repo document), source, ink |
| `ref` | the pointer: `mail:personal/<thread>`, `chat:<session>#<turn>`, `commit:quast@<sha>`, `event:<uid>`, `src:<doc>#p12` |
| `at` | when it happened |
| `area`, `thing` | the area, and the registry id when it belongs to one (from keryx's filing links, a chat's folder, a repo) |
| `people` | names and addresses (mail senders and recipients, event attendees) |
| `title`, `text` | what is searched; mail cleaned of quoted text and signatures |
| `hash` | content hash, so re-indexing unchanged things costs nothing |

**What is indexed from each kind:**

| Kind | Indexed as | Refreshed |
| --- | --- | --- |
| Mail | one item per message, cleaned; grouped by thread for `open` | every 15 min (laptop push, D2) |
| Calendar, tasks | title, attendees, notes, dates | every 15 min, from kairosd |
| Chats | each of your turns whole; Claude's answers in chunks. The server's headless `-tmp` sessions are skipped: they are locus's own jobs | every 30 min, after the push |
| Repositories | READMEs, `docs/`, decision logs, commit messages. Code is not indexed: an agent reads it live | nightly, and on demand |
| Sources, ink | read where they are, in `locus.db` (D1) | as now |

**The four tools** (heuris D):

- `context(about?)`: today and the next seven days from kairos; your live projects with their
  last session or commit; your applications file; what needs a reply; with `about`, that
  thing's registry entry and its recent items. It says plainly that facts and briefs arrive in
  H2.
- `search(query, kind?, area?, thing?, person?, since?, until?)`: words and meaning together,
  filters applied before ranking, reranked. It returns dated snippets with pointers. A `person`
  that matches two people returns both and asks which (test question 46).
- `open(ref)`: the whole thing behind a pointer. That is a mail thread in order; the chat turns
  around a turn; a page of a source, as an image where it has maths; a notebook page; or a commit
  with its files.
- `timeline(thing or person, since?, until?)`: everything about that thing or person in date
  order, across every kind, one line each.

**Mail is untrusted.** Every piece of mail a tool returns is wrapped the way keryx wraps it, as
data with a per-reply marker, so an instruction inside an email is read as text, never followed.
Mail also passes the 0.9 scrubber before it leaves the laptop.

**Silent failure (CLAUDE.md §3).** Each source records when it last indexed and how many items
it added or skipped. `locus heuris status` prints that, and Home's system section shows a source
in red when it is stale. Every tool call goes through `mcp-log` as now. The mail filter records
what it left out in `gates`, so a filter that drops something it should not is visible.

---

## Steps

Each step has its check: the printed output that proves it done.

### H1.1 Which login claude.ai accepts
No code. Read Anthropic's current connector documentation. Then point a test connector at a
throwaway Funnel endpoint and see what claude.ai asks for.
- **Your hand step:** add the test connector on claude.ai (I give you the URL), about two
  minutes.
- **Check:** a written finding, and claude.ai lists the test server's one tool.

### H1.2 Looking at real data (D3)
No code that stays. I sample your real mail through the starting rule, and show you items of
each kind with their proposed fields.
- **Your step:** mark the wrong ones; about 5 minutes.
- **Check:** your answers recorded in this spec, and the rule written as tests from them.

### H1.3 heuris.db and the indexer
- The `items` table, its vectors (nomic, 768) and full-text index.
- One indexer framework, incremental by content hash.
- Per-source freshness, `locus heuris status`, and the Home system line.
- **Check:** `locus heuris status` prints every source with its count and last run; the tests
  pass; a second run over unchanged data adds 0 items.

### H1.4 Mail (D2, D3)
- Laptop side: `locus mail-push` and its launchd job every 15 minutes, built on the 0.10 code.
- Server side: the mail indexer.
- **Check:** the counts per account are printed, with 0 from OQTS-shared. The Brevan Howard
  offer email, which an automated HR system sent, is found by `search` (test question 44).
  `gates` shows what was left out, with samples.

### H1.5 Calendar and tasks
- **Check:** `search "interview" kind=event` finds the real interviews; Family events are
  present (test question 41).

### H1.6 Chats
- **Check:** a decision from a chat on each machine is found by `search`, and `open` shows the
  turns around it.

### H1.7 Repositories
- **Check:** `timeline locus since=<a week ago>` lists this week's commits and the docs they
  changed (test question 16).

### H1.8 Sources and ink through the same search
- **Check:** `search` with a date filter returns sources and margin notes alongside mail and
  chats, reranked as one list (test questions 20 and 21).

### H1.9 The tools on the service (D6)
- `context`, `search`, `open` and `timeline` are added; the old tools are retired; mail is
  wrapped as untrusted data.
- **Check:** a test per tool and per filter, including the ambiguous person. A fresh laptop
  `claude -p` lists exactly the new tool set, and `mcp-log` shows each call arriving and
  finishing.

### H1.10 Every caller reaches it (D4, D5, D7)
- The Funnel and the login from H1.1.
- keryx registered on the laptop.
- The Home chat has the four tools.
- The CLAUDE.md line.
- **Your hand step:** add the real connector on claude.ai.
- **Check:** one question each, answered with tool calls, from:
  - Claude Code on the laptop;
  - Claude Code on the server;
  - the Home chat;
  - claude.ai on your phone.

### H1.11 The test set (D8)
- **Check:** the scored table is in the report. Every question in the "answerable in H1" group
  passes, or its failure is named with its cause and the slice that fixes it.

**Order:** H1.1 and H1.2 first, because they need you and they shape the rest. Then H1.3 to
H1.8, then H1.9, H1.10 and H1.11.

---

## Not in H1
- Records, facts, briefs, `remember` and `forget`, and the Home chat's `y/n`: H2.
- Thread summaries and session records, the model steps from heuris C: H2.
- claude.ai chats, which live only in Anthropic's export: decided in H2.
- kairos behind the Funnel, which would allow scheduling from the phone: later, if you want it.
- Moving the library into `heuris.db`, and the propositions measurement: after H1.
- Drafting mail from the phone: later, because keryxd runs only on the Mac.

## Report
*(written when H1 closes)*
