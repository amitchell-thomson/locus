# H1: the index and the tools

**State:** approved 10 Nov 2026, all eight decisions as recommended ("go with recommendations") · **Written:** 2026-11-10 · **Runs in:** `claude` (locus-dev)
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

**Finding (10 Nov), from Anthropic's connector docs** (claude.com/docs/connectors/custom/add-unlisted,
claude.com/docs/connectors/building/authentication, support article 11175166):
- A fixed header token (`static_headers`) is "Beta, for a limited set of organizations", so it
  cannot be relied on. OAuth is supported by default: Dynamic Client Registration (RFC 7591) or
  Claude's published Client ID Metadata Document. **So D4 takes the OAuth branch**: the service
  runs the single-user OAuth flow with a passphrase page.
- Claude's redirect is `https://claude.ai/api/mcp/auth_callback`; Claude Code uses a loopback
  redirect on any port.
- Claude connects from Anthropic's cloud, from `160.79.104.0/21`, so the server must be public:
  Tailscale Funnel. Funnel is enabled on compute-node; port 443 already serves something else on
  the tailnet, so locus takes Funnel port **8443**.
- The docs do not say outright that a connector added on the web appears in the phone app; the
  real connector in H1.10 checks it.
- The throwaway test connector is skipped: the docs settle the login, and the real connector in
  H1.10 is the live check.

### H1.2 Looking at real data (D3)
No code that stays. I sample your real mail through the starting rule, and show you items of
each kind with their proposed fields.
- **Your step:** mark the wrong ones; about 5 minutes.
- **Check:** your answers recorded in this spec, and the rule written as tests from them.

**Your answers (10 Nov).** The starting rule over keryx's mirror, OQTS-shared excluded, kept 3,440
of 6,630 messages: sent 1,177 · threads you replied to 969 · people you have written to 822 ·
other people 230 · firms and recruiting systems 198 · Oxford offices 30 · flagged 14. Left out:
list or automated headers 2,069 · automated senders 532 · companies with no person 459 · bulk
domains 130. On the samples:
- **Oxford mailing lists: keep all of them** (you chose this over admin only), societies included.
- **Receipts: travel only.** Flights, trains and bookings kept; shop orders and promotions not.
- **Societies outside Oxford's lists: the quant and careers ones** (Oxford Alpha Fund and the
  like); sport and social clubs not.
- Fixed without asking: Microsoft Family Safety reports were kept as "a person"; the careers
  service's daily alerts were kept because you once wrote to that address; Oxford Alpha Fund was
  kept once and left out once.
- Quoted text and signatures: shown to you on five real threads when the mail indexer exists
  (H1.4), since the cleaner is what is being judged.

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

### H1.12 Attachments and career files (added 10 Nov, your call: "add it to H1 first")
Found while you corrected the test set: the role preference form (Y2) is a PDF attachment you
marked up and returned, and the Millennium contract (24) is a file in `~/Downloads`. The index
held attachment names, not contents, and no files outside repos.
- **Attachments:** each PDF, Word or text attachment on mail the rule already keeps has its text
  read on the laptop (from Mail's own store; never fetched), and becomes an item of kind
  `attachment`, pointing at its message, with the message's people, thing and area.
- **Files:** kind `file`, read on the laptop from two places only:
  - iCloud `Career/` (applications, contracts, cover letters, your CVs, course material);
  - `~/Downloads`, only names that look like an offer, contract, offer pack or handbook.
- **Never read, by name:** passports, driving licences, P46 and tax forms, information and data
  collection forms, bank and account statements, payslips, visa and right-to-work papers. A CV
  that does not name you (someone else's) is skipped. Scanned pages with no text layer give no
  text and are skipped.
- **In whatever is read:** the secret scrubber, plus national insurance numbers, passport
  machine-readable lines, IBANs, sort codes with account numbers, and card numbers are redacted
  before anything leaves the laptop.
- Both are wrapped as untrusted data in the tools, like mail.
- **Check:** `search` finds the Millennium questionnaire's contents (Y2) and the contract's
  terms (24); what was skipped by name is counted in the push report; a test asserts each
  identifier kind is redacted.

- **Done 10 Nov:** 211 attachments and 48 files indexed. Skipped by name: 59 attachments and
  9 files (identity, tax and bank papers, mandates), 3 CVs that are not yours. 347 attachments
  Mail never downloaded are counted, not fetched. Crosses drawn over a form's checkboxes are read
  as the options chosen (the questionnaire: B, B, B, C, matching the page). Two things found on
  the way:
  - The push job (launchd) cannot read iCloud or Downloads (macOS privacy). A partial read now
    sends nothing, so the server keeps its last copy; the files refresh only when the push runs
    from a terminal until the job is given access.
  - The answer key leaked: this file, `h1-test-set.md` and the chat that wrote them were indexed.
    All three are held out while H1 is scored.

**Order:** H1.1 and H1.2 first, because they need you and they shape the rest. Then H1.3 to
H1.8, then H1.9, H1.10, H1.12 and H1.11.

---

## Not in H1
- Records, facts, briefs, `remember` and `forget`, and the Home chat's `y/n`: H2.
- Thread summaries and session records, the model steps from heuris C: H2.
- claude.ai chats, which live only in Anthropic's export: decided in H2.
- kairos behind the Funnel, which would allow scheduling from the phone: later, if you want it.
- Moving the library into `heuris.db`, and the propositions measurement: after H1.
- Drafting mail from the phone: later, because keryxd runs only on the Mac.

## Progress (10 Nov, end of the first day)

- **H1.1–H1.2:** done (above).
- **H1.3–H1.7:** the first full index was built on the server in under 4 minutes on the GPU:
  - mail 3,958 items (OQTS-shared: 0);
  - calendar and tasks 921;
  - chats 6,904 (both machines);
  - repos 2,328 (9 projects, 189 docs, 2,140 commits);
  - 32,715 chunks in all.

  The server re-indexes every 15 minutes (`locus-heuris.timer`); the laptop pushes on the same
  beat (`com.locus.heuris-push`).
- **Mail cleaner:** quoted text and signatures cut kept-reply text from 7.3M to 0.5M characters.
  A forward under a short note keeps the forwarded mail, unquoted.
- **H1.8–H1.9:** `context`, `search`, `open`, `timeline` are on the service, and the old tools
  are retired. Checks against the real index:
  - the BH offer email is found (Q44);
  - interviews are found among events;
  - Family events are present;
  - this week's locus commits are in order.

  A fresh laptop `claude -p` answered "when did I last hear from Brevan Howard" correctly from
  mail with pointers, in 29 s.
- **Fixed from the checks:**
  - `area=Family` let library papers through;
  - a firm ("Brevan Howard") was read as a person's name; it now resolves to its applications
    plus mail from its domain.
- **H1.10:**
  - the Home chat has the tools, and answered Q1 from them;
  - keryx is registered at user level on the laptop;
  - the CLAUDE.md line is on both machines;
  - the public listener runs behind Funnel at `https://compute-node.tailb449e5.ts.net/mcp` (moved from :8443, which claude.ai never reached; the nabla dashboard moved to tailnet-only :8444),
    with OAuth and a passphrase (Keychain `locus-public-passphrase`).

  **Waiting on you:**
  - add the connector on claude.ai;
  - sign Claude Code in again on the server (its login had expired, so the server check could
    not run).
- **H1.11:** answer lines drafted in [h1-test-set.md](h1-test-set.md), waiting for your
  corrections before scoring.
- **Found:** keryx's own tools return OQTS-shared mail to any chat that has them (the Home chat
  read one thread), which goes against integrity question 47. The index never holds it. This is
  a decision for you; keryx is unchanged.

## Report

### H1.11 scored (10 Nov)
48 H1 questions, each in a fresh `claude -p` on the laptop: locus, keryx and kairos tools only,
no built-in tools, no repo, run from a temp folder so the runs are never indexed. Four graders
marked each answer against your line, opened every cited pointer and searched for what an answer
missed. Answers, tool calls and grades: `data/evals/h1-2026-11-10/` (laptop, not committed: it
holds your mail).

**26 pass · 12 partial · 10 fail.** Q40 (phone) is yours to run.

| | Passed |
| --- | --- |
| All three marks | Y2 Y4 Y6 · 2 3 5 6 8 9 11 · 13 15 16 17 18 19 20 25 26 · 33 37 41 44 46 47 48 |

| # | State | Sources | Nothing invented | Overall | Cause and fix |
| --- | --- | --- | --- | --- | --- |
| Y1 | partial | partial | pass | partial | Read the Millennium letter, still gave the dates from the handwritten offer-call note; did not use your stated goal (QD/QR beside a systematic pod, 30 Oct mail). |
| Y3 | partial | partial | **fail** | fail | "Millennium accepted today": it was 2 Nov. Cited notes by section, not page; missed the BH "EM ideas" note. |
| Y5 | pass | partial | pass | partial | regime-ml is not in the registry, so it said it had no repo; the library holds it (`src:175`). |
| Y7 (BH contract: **signed**, your answer 10 Nov; a renege email to Sarah planned) | partial | partial | **fail** | fail | "BH fills places from its reserve list" stated as fact, unlabelled; said the Millennium letter is not in mail (it is a file). |
| Y9 | partial | pass | pass | partial | 2 Jun start from the handwritten note; the letter says 21 Jun. |
| 1 | partial | partial | partial | partial | Missed the passport renewal: your 5 Nov reply is indexed with no body (rich reply). Took the family calendar's Düsseldorf flight as yours. |
| 4 | pass | partial | pass | partial | Named no commits, though four from 10 Nov are indexed. |
| 7 | pass | partial | pass | partial | Lectures right; only one has a library pointer, none an event pointer. |
| 10 | **fail** | partial | pass | fail | "No margin notes for locus or GR AI/ML": the reading notes on the locus plan, screens and heuris v1, and on the Transformer primer, exist; `thing=` filters miss them. |
| 12 | pass | partial | pass | partial | BH quotes verbatim; repo docs named but not opened or cited. |
| 14 | pass | pass | partial | partial | Two bullets overstate the source (which bug rejected UK orders; "matching all 5,177 deals"). |
| 21 | partial | fail | **fail** | fail | Said the B14 notebook is untranscribed; it is indexed (`src:664`). The right answer: no 2D Fourier page exists. |
| 22 | partial | pass | **fail** | fail | Proposed 8pm Wed/Thu to Kristof: Wed clashes with MAO Drinks, Thu is a flight. Drafts made, none sent; the OQTS-shared ones were refused (read-only). **Missed Milly Parrott's 2 Nov Tower reply** (yours to answer): the thread is in the inbox but 248th of ~400, since keryx's scheduled runs (needs-reply tags, clearing automated mail) were never installed, and every listing the chat made was cut off before it (your correction, 10 Nov). |
| 23 | partial | pass | pass | partial | No 3YP email proposes a meeting (likely WhatsApp); it said so and scheduled nothing. |
| 24 | **fail** | partial | pass | fail | Found the offer letter but `open` returned all 55 KB and the client spilled it to a file it could not read. Missed 21 Jun–27 Aug, pay and stipend. |
| 34 | pass | partial | **fail** | fail | "OQTS, which Tower has sponsored from the start": no source. Repeated the CV's "3.4 s" tune-up; quast's results say 3.4 *minutes*. Checked 10 Nov: both are real. 3.4 s is detuning-only control on the L40S (§8bi); v2 with barrier control is 3.4 min learned, 0.8 told (§8ci). The CV line describes the older mode. |
| 35 | partial | pass | **fail** | fail | Treated family-calendar events as yours (Callum's personal training, the Düsseldorf flight). |
| P1 | partial | partial | **fail** | fail | Copied the stale mixer time (17:00) from the Home chat; the 10 Nov mail moved it to 16:00. The pointer it held had changed (thread ids move when a reply arrives). |
| P2 | **fail** | pass | pass | fail | Good plan, but kairos APPLIED its writes (an event, a task, a block) rather than leaving them pending: low-risk writes from MCP apply at once. The task (and its block) was also wrong in itself: the mixer is where teams are settled. Undone 10 Nov at your request; the mixer event stays. |
| 36 | partial | pass | pass | partial | kairos reports only the last 30 days, so "last month" became 11 Oct–10 Nov (said so). |
| 38 | partial | partial | pass | partial | Missed September's careers activity (Jump onsite, Millennium rounds); called September vacation (Quantum Device Lab internship). |
| 39 | partial | partial | partial | partial | Called regime-ml an unbuilt idea; it was built (`src:175`). |

**What the failures have in common, and the fix for each** (H1 fixes unless marked):
1. **Family calendar shown as yours** (1, 35, P1). Tool output should mark family-overlay events as such, with their guests.
2. **Offer terms not where answers look** (Y1, Y7, Y9, 24). The career files have no `thing` (a Downloads letter names no firm folder), and `open` does not page a long file. Fixes: file a file under a firm by its text; `open` pages long text.
3. **Library projects and notes not tied to projects** (Y5, 10, 39). regime-ml is a library project with no registry id, and reading notes carry no `thing`. Fix: register regime-ml; map each "Reading notes" note to its document's project.
4. **Index gaps**: a rich-text reply indexed with no body (1); `markups` does not point at a notebook's indexed transcription (21); mail thread ids change when a reply arrives, so older pointers stop opening (P1).
5. **Unlabelled claims and stale facts** (Y3, Y7, 22, 34, P1): the model's own inferences stated as fact, and an older chat trusted over newer mail. Partly H2 (facts with dates, newer replaces older), partly the CLAUDE.md line on labelling.
6. **kairos** (P2, 36): low-risk writes from MCP apply without approval, and project time covers only 30 days. **Your call, 10 Nov: keep it** ("if I ask for something it should just happen, small scheduling is fine and can always be changed back"), so P2's applied writes were right, not a fault. The 30 days stays a limit of the worked-time measure (and Home's 8-week chart is still sample data: H1 fix 6 below).
7. **OQTS-shared reaches the index indirectly** (47's grader): the 10 Nov Home chat quoted Kristof's OQTS-shared mail and is indexed. Fine for now, as you said; noted.

### Fixes made (10 Nov, evening), before the re-run

Each checked on live data through the locus tools after deploying, not only by tests.

| # | Fix | Live check |
|---|---|---|
| 1 | Family events say whose they are: `[family]` in the title, a first line saying so, and `context` lists them as "FAMILY calendar: not necessarily his". | `context`: the Düsseldorf flights and Callum's training are labelled. |
| 2 | `open` pages long things (a file by paragraphs, a thread by whole messages, never cutting an envelope). A career file whose folder names no firm is linked by its opening text, an offer document to the application at the offer stage. | `timeline millennium`: the offer letter and offer pack. The first try linked them to Meet Millennium 2026, and a book to Millennium; both fixed. |
| 3 | regime-ml registered (parked). A library document carries the project its title or path names; a project filter keeps those instead of dropping the library. | Tests; the library's own paths decide. |
| 4 | A reply sent all-quoted (or only an image placeholder and links above the quote) keeps its own text. `markups` points at a notebook's transcription. A mail pointer follows its message when the thread's id moves. | Search "passport renewed next month": the 5 Nov reply first. |
| 5 | `waiting`: who wrote last and asked, over all mail, lists and bulk mail left out. | `waiting on me`: 15 threads with Milly, Connor (B15), Isobel (B18), Kate; first cut gave 74. |
| 6 | The 8-week work chart is measured (it was sample data); kairos keeps 56 days. The laptop's worked-time scan had stopped silently on 9 Nov (a stale binary could not read the registry); fixed, and the sync now says when it cannot measure. | The chart shows quast 49 h, locus 47 h, nabla 19 h over eight weeks. |
| 7 | One line in the global CLAUDE.md and the same in the locus server's instructions: label guesses, newer sources win, family events are not his. | In the server's instructions. |

Also: one filing model (keryx's old folders retire into the taxonomy; Admin and Travel are Personal streams; JCR Treasurer is a registry role), and keryx's scheduled runs wait for keryxd (the 18:00 run had found no tools).


**Found and fixed during the run:** tools ran on the server's event loop, so one search stalled
everyone; with four chats at once, 4 of 48 could not connect. Tools now run in worker threads
(tested, deployed; the four re-ran under the same load and connected).

**Spot-check (yours):** Y2, Y3, Y7, 1, 10, 18, 24, 34, 46, 48: five passes and five failures
across every group. Each is `data/evals/h1-2026-11-10/answers/<id>.md` with its grade in
`grades/<id>.json`.
