# Locus — capture, annotation & learning layer (design plan)

> Status: **planning** (2026-06-23; restructured 2026-06-27). This designs the components that
> turn Locus from a RAG engine into the owner's full personal knowledge/learning/information
> system. **It is part of Locus, one repo** — the ingest+retrieval engine is one component; the
> capture/enrich/organise/reading/learn components added here are others. The only boundary that
> matters is internal: **mutable agent state lives in its own tables and never mutates the ingest
> spine** (CLAUDE.md principles 7–9); the corpus stays immutable/regenerable as today.

---

## 1. What it is (and is not)

A **server-hosted, asynchronous, grounded, propose-never-mutate** layer over an Obsidian vault.
The owner captures (primarily by handwriting on a reMarkable); headless `claude -p` runs on the
server transcribe, moderately fill-in, link, and organise the capture *around* the owner's words,
then file it into Locus as searchable, linked corpus. The owner reads/triages in **Obsidian** (Mac
+ phone, viewers of a synced vault) and studies pushed material back on the **reMarkable**.

**Why it belongs in Locus:** it operationalises the **Link** use case (CLAUDE.md §1, the
under-served primary — connections *pushed* to the owner at write-time instead of waiting to be
queried) and closes the corpus's biggest blind spot — the owner's **live idea-stream**, currently
absent (Locus ingests only past/external artifacts). It also makes the engine's value daily and
visible, so every retrieval-quality gain has daily payoff.

**Is not:** a change to the ingest/retrieval *engine*; a second retrieval implementation; an editor
of the owner's prose; a store of mutable state *inside the ingest spine* (review/queue/mastery
state lives in its own tables; tasks are vault markdown). The corpus tables stay immutable and data
flows one direction at each boundary.

**The conditional (the whole plan rests on this):** the value holds *only if* the separation holds
— mutable state out of the spine, generated content tagged & corpus-excluded, one direction per
boundary, propose-never-mutate. Blur any of these and the layer corrupts the invariants that make
retrieval trustworthy.

---

## 2. Primary target — build these two loops first

The product stands or falls on **two core loops**, both riding one shared foundation (reMarkable
push/pull + Claude-vision + ingest spine + grounded enrich):

1. **Rough-note enrichment & store loop** — handwrite → transcribe → moderate fill-in → grounded
   enrich → store as searchable, linked corpus.
2. **PDF annotate & store loop** — export any PDF (FT / quant paper / article) → read + annotate on
   the reMarkable → store the *source* **and** your highlights/margin-notes as linked corpus.

**Success criterion:** capture a handwritten note *and* read+annotate a PDF, and have both land as
enriched, linked, searchable corpus with provenance intact. Everything in §11 (organise, reading
list, learning, intake, output) is **downstream** and waits until both loops are trusted.

### Felt experience (accurate to the target)

You handwrite on the reMarkable — zero decisions, no titling/filing. Within the hour the server has
pulled it, transcribed it, expanded your shorthand into prose (**AI additions marked, raw
preserved**), added one `> [!ai] Related` block of *grounded* links + a followup, auto-filed it,
and made it searchable in Locus. When you read a pushed paper on the tablet and scribble in the
margins, those annotations come back the same way — the paper and your reactions both become
permanent, linked knowledge. You only ever do the part you enjoy (thinking on paper); filing,
linking, and remembering are ambient. **The system proposes; you dispose; every proposal is cheap
to ignore.**

---

## 3. Locked decisions

| Dimension | Decision |
|---|---|
| **North star** | Frictionless capture — every thought in with zero ceremony; organise/link later. |
| **Primary capture** | **Handwritten notes on a reMarkable tablet**, auto-pulled server-side. |
| **Secondary capture** | Desktop Obsidian (daily-note hotkey) + phone (Obsidian mobile). *Phase 3.* |
| **Transcription** | **Claude-vision** on the pulled raster (no per-note manual trigger, no API cost on the subscription); reMarkable built-in MyScript as the low-confidence fallback. See §3.1. |
| **Fill-in depth** | **Moderate** — expand shorthand into prose, preserve meaning, **mark AI additions**, raw always preserved. |
| **Autonomy** | **Hybrid** — scheduled batch enrich + on-demand live enrich. |
| **Interaction layer** | **Obsidian on the Mac is THE front-end** (markdown + community-plugin visualisations; phone is a secondary viewer). No TUI. The digest (later) renders into Obsidian. |
| **reMarkable role** | Asymmetric bidirectional: handwriting **out** (capture); papers / extension notes / gap comments pushed **in** to read + annotate; annotations return via the capture path. |
| **Hosting** | **Everything on the always-on server** (capture, agents, Locus). `claude -p` runs server-side on the server's subscription — nothing pauses when the Mac sleeps. In-terms; only rate limits monitored. |
| **Codebase** | **One repo = Locus.** Agent components are subpackages; mutable agent state in its own tables (same DB, `locus backup` covers it); ingest spine untouched. |
| **Vault** | **Authoritative on the server**; **Syncthing** (self-hosted, bidirectional, mobile) mirrors to Mac + phone viewers — no third-party cloud (local-data-ownership). |
| **Corpus entry** | All notes enter, tagged `maturity` (`rough`\|`tidy`); retrieval **down-weights `rough`** (flag, never filter) so scrawl never drowns signal. Promotion re-ingests at full weight. |
| **Auto-organise** | **Full auto, reversible** — title/tag/file/split, every action logged and undoable. |
| **Reading list** | Corpus **+ external** (arXiv/academic, follow citations); queued items **auto-ingested** so they become searchable. *Phase 4.* |

### 3.1 Transcription path (the one thing to confirm in Phase 0)

```
rmapi pulls the raw notebook → render pages to PNG → claude -p (vision) transcribes
→ moderate fill-in → enrich → organise → file → ingest
```
Claude's vision reads cursive well, runs on the subscription (no API cost), and needs no manual
convert step. **Fallback:** for pages Claude flags low-confidence, use the reMarkable's built-in
MyScript conversion (emailed in, read via the Gmail MCP). Confirm quality on a sample of the
owner's actual handwriting before building (Phase 0).

---

## 4. Architecture (server-hosted)

```
   SERVER (always-on; runs everything)
   ├─ rmapi  ◀── pull ── reMarkable notebooks (capture: handwriting OUT)
   │          ── push ─▶ reMarkable (reading IN: papers / extension notes / gap comments → annotate)
   ├─ Locus (one system, one repo, one DB):
   │    • engine (existing):  ingest spine · retrieval · link layer · MCP server
   │    • new components:      capture/ enrich/ organise/ reading/ learn/ intelligence/ vault/ agent/
   │    • agent-state tables:  review / queue / mastery / acceptance / runs  (same DB, separate tables)
   │    • orchestration:       headless `claude -p` for language tasks; grounding via the LOCAL
   │                           retrieval engine in-process (§9); systemd timers + on-demand CLI
   └─ authoritative vault ──Syncthing──▶ Mac + phone (Obsidian viewers)

   reMarkable: handwriting OUT, curated reading IN (no note-sync back) ·
   vault ↔ Mac/phone (Syncthing) · MCP-over-SSH stays ONLY for the owner's interactive Claude clients
```

Consequences of server-hosting: (1) nothing pauses when the Mac sleeps; (2) the agent grounds
against the **local** engine — no SSH in the agent layer; (3) one codebase, one DB, one
`locus backup`. **Two-writer conflicts** (owner + agent on one hand-typed file via Syncthing) are
avoided for reMarkable notes (they originate server-side) and mitigated for typed notes by atomic
owned-block writes, falling back to **sidecar enrichment notes** if conflict copies appear.

---

## 5. Invariants (acceptance criteria)

1. **Asynchronous & non-blocking.** Capture is instant; enrichment catches up out-of-band. No user
   action ever blocks on a model call.
2. **Propose, never mutate.** Agents write only into clearly-owned, collapsible `> [!ai] …` blocks
   inside human notes, and into agent-owned generated notes. They never edit the owner's prose.
3. **Grounded or silent.** Every suggested link / reading item / cross-time pointer / claim cites a
   real corpus unit from retrieval, or it does not appear. No ungrounded filler.
4. **Provenance is structural & glanceable.** Everything an agent produces is callout-marked and
   carries frontmatter provenance (`author: agent`, `generated`, `source_run`).
5. **No feedback-loop contamination.** Agent-generated content ingested by Locus is tagged so
   retrieval can weight/exclude it; agents never treat their own generations as ground truth.
   `_generated/` is corpus-excluded by default.
6. **One direction per boundary.** reMarkable → pull → transcribe/fill-in → vault (owned blocks) →
   ingest → DB → (export) → read-only projection. The reMarkable read-push is a *separate one-way
   delivery channel* (material out; only fresh handwriting comes back, via capture). The agent
   never round-trips the read-only projection (CLAUDE.md §13).

---

## 6. Data flow, folders & provenance

### End-to-end (rough-note loop)

```
CAPTURE handwrite (zero decisions) → PULL rmapi (scheduled) → TRANSCRIBE claude-vision (MyScript
fallback) → FILL-IN moderate, AI marked, raw preserved → ENRICH grounded > [!ai] Related →
ORGANISE auto title/tag/file/split (reversible) → FILE to vault → INGEST as note, maturity=rough
→ PROMOTE later: agent proposes tidy, owner blesses → re-ingest maturity=tidy (full weight)
```

### Vault layout (one unified Obsidian vault, zoned by ownership)

**All vault mutation happens on the server** (capture, enrich, organise, digests, resurfacing, the
learning surfaces). The Mac/phone are viewers; Syncthing propagates the server's result to them.
The Mac being off never pauses anything.

One Obsidian vault is THE front-end; the Locus **read-only projection** (CLAUDE.md §13) lives inside
it as an exporter-owned subtree, so a **single graph spans authored notes + doc/entity hubs**. The
exporter still owns only that subtree and never touches `.obsidian/`, so unifying is safe (this
supersedes §13's separate-vault transport *for this system*). The **organising axis is
ownership/provenance, not topic** — retrieval is Locus's job, folders are for the human and for
agent-ownership boundaries (invariant 4).

```
LocusVault/                         # server-authoritative · Syncthing → Mac + phone · the single front-end
│
│  ── HUMAN-OWNED (you write; agents may only append owned `> [!ai]` blocks) ──
├── daily/<YYYY-MM-DD>.md           # quick/typed capture inbox + dated log
├── notes/                          # atomic notes (handwriting→transcribed+filled, or typed); maturity in frontmatter
├── annotations/                    # "Annotations on <paper>" notes from the PDF loop
│
│  ── AGENT-OWNED (regenerable; overwritten freely; corpus-excluded, invariant 5) ──
├── _generated/
│   ├── digests/<date>.md           # daily update / news digest
│   ├── resurfaced/<date>.md        # the day's old-note resurfacing (grounded "why relevant")
│   └── briefings/<topic>.md        # goal briefings
├── reading/queue.md                # reading list + state (Dataview); agent proposes in owned blocks
├── learn/
│   ├── recall/                     # due recall prompts (also pushable to the reMarkable as a warm-up)
│   ├── mastery.md                  # competence heatmap (Dataview over the mastery table)
│   └── synthesis/                  # teaching-back / synthesis prompts
│
│  ── DASHBOARDS (Dataview control surfaces) ──
├── _home.md                        # "Today": digest · due reading · resurfaced notes · due recall · open followups · triage
│
│  ── LOCUS PROJECTION (read-only, exporter-owned subtree; regenerable) ──
├── _locus/
│   ├── docs/<category>/<slug>.md
│   └── entities/<type>/<slug>.md
│
└── .obsidian/                      # Mac-owned: plugins, theme, graph layout (agents never touch it)
```

- **Provenance:** agent-owned blocks use stable markers and are **regenerated wholesale**
  (idempotent — re-run replaces, never accumulates); generated notes carry frontmatter provenance.
- **`_home.md` is the daily landing surface** — a Dataview "Today" note aggregating every
  agent-produced surface in one place, so the owner opens one note and sees the day.
- **Two-axis safety:** human zones are never overwritten (only appended-to in owned blocks); the
  `_generated/`, `_locus/`, and dashboard zones are fully regenerable.

---

## 7. Locus engine changes (the only spine-adjacent work)

Everything else is new components that *consume* the engine. The engine itself changes only here:

1. **`maturity` column + retrieval weight** (migration `0009`; `[retrieve].rough_weight`).
   Down-weight `maturity=rough` at merge — **flag/down-weight, never filter** (keeps the
   recoverable-class property, principle 8). The one genuinely new retrieval behaviour.
2. **Authoring vault as a `note` ingest source.** `note` category + `vault/notes/` input already
   exist. Ingest the vault **incrementally** on the `repo_sync.py` blob-manifest-diff template
   (notes churn — never full re-ingest per save); normalise whitespace before hashing (CLAUDE.md
   §11 limit); reuse the `watch` settle-window debounce; exclude `_generated/`.
3. **External-paper producer** (reading list): queued paper → dropped into `incoming/` for the
   normal spine. *A new producer, not a new ingest path.*
4. **Annotations table** (migration `0011`, optional, Phase 2): anchors highlights to source
   chunks (§10.2). Append-only owner data; does not mutate the spine.

Eval labels grow with the new corpus (CLAUDE.md §11/§14 standing rule).

---

## 8. Module layout, migrations & config

```
locus/
├── (engine, existing)  extract/ ingest/ retrieve/ link/ export/ query.py mcp_server.py db/
├── agent/        claude.py (headless runner) · budget.py (rate-limit guard) · run.py · journal.py
├── capture/      remarkable.py (rmapi pull+push) · transcribe.py · fillin.py · annotations.py
│                 daily_note.py · mobile.py · voice.py · web.py · email.py · feeds.py
├── enrich/       related.py (grounded > [!ai] blocks)
├── organise/     classify.py (title/tag/file) · split.py · undo.py (reversible journal)
├── reading/      suggest.py · queue.py · ingest_paper.py · deliver_remarkable.py · md2pdf.py
├── learn/        recall.py · curriculum.py · mastery.py · synthesis.py
├── intelligence/ flywheel.py · contradiction.py        # gap_reading lives in reading/
└── vault/        writer.py (owned blocks, provenance, atomic) · markers.py · sidecar.py
```

**Migrations (forward-only; current head 0008):**
- `0009` — `documents.maturity TEXT` (`rough|tidy`).
- `0010` — agent-state tables: `review_schedule`, `reading_queue`, `mastery`, `acceptance_log`,
  `agent_runs`. Same DB (so `locus backup` covers them); separate from spine tables.
- `0011` — `annotations` (optional, Phase 2): `(source_doc_id, page, type, text, anchor_chunk_id, owner_note_id)`.

**Config (all optional, default clean per §14):** `[retrieve].rough_weight`, `[capture]` (rmapi
target, schedule, transcription thresholds), `[reading]`, `[learn]`, `[agent]` (budget thresholds,
schedules).

**New CLI verbs:** `locus read` (§10.0) · `locus capture-sync` (manual pull/transcribe/enrich run)
· scheduled internal entrypoints driven by systemd timers.

---

## 9. Orchestration — triggering, grounding, scheduling

**Grounding decision (resolves the MCP-vs-in-process ambiguity):** the **Python orchestrator
grounds in-process** by calling the local retrieval engine directly (deterministic, testable,
free); **`claude -p` is used only for language tasks** — vision transcription, fill-in, phrasing
the enrich block, judgement — with the grounding context (retrieved candidates) passed in the
prompt. MCP-over-SSH is **not** used by the agent layer; it remains the surface for the owner's
interactive Claude clients only. This keeps link-selection grounded and unit-testable, and uses
Claude for what only Claude can do.

**Triggering:**
- **Scheduled (systemd timers):** pull the reMarkable on an interval (e.g. 15–30 min); overnight
  batch enrich; periodic flywheel/mastery folds.
- **On-demand:** `locus capture-sync` for an immediate pull→enrich run.
- **Typed notes (Phase 3):** a debounced file-save watcher on `vault/notes/`.

**Budget guard (`agent/budget.py`, built in Phase 1, used everywhere):** wraps every `claude -p`
call, tracks subscription usage, and yields to foreground interactive use (background work pauses
rather than starves it). Hard debounce + batching throughout.

**Idempotent re-pull (closes a real gap):** reMarkable documents are keyed by reMarkable doc id +
**per-page content hash**; a re-pull re-transcribes only changed/new pages. PDF annotations are
keyed by `(doc, annotation-layer hash)` so re-pulling an evolving annotated PDF **updates** the
stored annotations rather than duplicating them. Each run is journaled (`agent/journal.py`).

---

## 10. The two primary loops (and the foundation) in detail

### 10.0 Foundation — `locus read` (markdown → reMarkable)

A small standalone utility (the `reading/md2pdf.py` + `capture/remarkable.py` push side, usable on
its own). Renders any markdown to a device-tuned document and pushes it. **Useful day one** (read
any repo doc on the tablet) and it **proves the rmapi push channel** both loops depend on.

```
locus read docs/obsidian-agent-layer-plan.md      # render → push one doc
locus read docs/                                  # push a folder
locus read CLAUDE.md --format epub                # EPUB for reflowable prose
```
- **Render:** `pandoc` md → PDF (default), page geometry tuned to the reMarkable (~1404×1872
  portrait, ~226 dpi) so headers/tables/code read cleanly. PDF is faithful for docs (fixed layout);
  EPUB is the reflowable option for prose (at the cost of mangling code/tables).
- **Upload:** `rmapi put` into a device folder (e.g. `Locus/Docs`); appears after cloud sync.
- **Deps/caveats:** one-time rmapi auth; a PDF engine (tectonic/xelatex) or an HTML→PDF path
  (weasyprint) to avoid LaTeX; a few seconds of cloud-sync latency.

### 10.1 Loop A — rough-note enrichment & store

The §6 end-to-end flow. New components: `capture/{remarkable,transcribe,fillin}`, `enrich/related`,
`vault/writer`, `agent/{claude,budget,run,journal}`. Engine: §7.1 (maturity) + §7.2 (note ingest).
Gate: a real handwritten note appears enriched in Obsidian *and* searchable in Locus as `rough`.

### 10.2 Loop B — PDF annotate & store

The reMarkable becomes a reading-and-annotation surface whose output flows back as first-class
corpus: **the document AND your reactions both become searchable, linked knowledge — your
highlights a "what mattered to me" signal.**

```
1. EXPORT    any PDF → reMarkable (rmapi put). Source: reading queue, `locus read <pdf>`,
             web-capture, an arXiv id, an FT article saved as PDF.
2. READ      highlight, underline, handwrite margins, draw. reMarkable stores ink/highlights as a
             separate annotation layer over the original PDF.
3. PULL      rmapi get → original PDF + annotated/flattened PDF + raw stroke data.
4. EXTRACT   • highlights → map rectangles to the PDF text layer (pymupdf) → exact sentences marked
               (no text layer ⇒ OCR the region).
             • margin ink → render region to PNG → claude-vision transcribe (the capture path) →
               text anchored to page/location.
             • underlines/boxes → positional markers anchored to nearest chunk.
5. STORE     • source PDF → normal ingest spine (source_type=pdf). Idempotent by content hash (a
               paper already in the corpus dedupes; an arXiv id won't double-ingest).
             • annotations → a linked `note` doc ("Annotations on <paper>", maturity=rough):
               transcribed margins + quoted highlight excerpts with page anchors, linked to source.
             • optional `annotations` table (0011): anchor highlights to source chunks so retrieval
               surfaces "you highlighted this passage" alongside the paper's own chunk.
6. ENRICH    margins get the normal treatment (fill-in, grounded > [!ai] Related to source + corpus,
             followups). Highlights feed mastery, reading-queue ranking, and the acceptance flywheel.
```

New: `capture/annotations.py` + optional `0011`. **Reuse:** rmapi transport (10.0), `pymupdf`
(already in stack), claude-vision (10.1), the PDF+note spine, enrich/link. Mostly composition.
Loop-specific risks: reMarkable `.rm` format drift across firmware (pin the renderer behind an
adapter); highlight mapping needs a text layer (else region OCR); ink transcription error
(confidence-flag, keep the raster).
Gate: a read+annotated PDF stores source + annotations as linked, searchable corpus; re-annotating
updates rather than duplicates (§9 idempotency).

---

## 11. Phasing

### Foundation (shared)
- **Phase 0 — verify + size.** Confirm (a) **rmapi pull *and* push** server-side; (b)
  **Claude-vision transcription quality** on real handwriting (MyScript fallback if poor); (c)
  per-run token cost → set the budget-guard thresholds. Throwaway scripts in `scripts/`; go/no-go.
- **Phase 0.5 — `locus read`** (§10.0). Quick win + proves the push channel.

### The two primary loops
- **Phase 1 — rough-note loop** (§10.1). The end-to-end thought→corpus loop. *Live on it before extending.*
- **Phase 2 — PDF annotate loop** (§10.2). Reuses Phase 0.5 transport + Phase 1 transcription/enrich.

### Then — the broader system (only once both loops are trusted)
- **Phase 3 — organise + secondary capture.** Full-auto title/tag/file/split (reversible);
  daily-note hotkey + phone capture into the same pipeline; Dataview triage dashboard.
- **Phase 4 — reading list.** Corpus + external (arXiv, citation-following) suggestions; queue with
  state; auto-ingest (§7.3); push delivery; promotion rough→tidy.
- **Phase 5 — capability multipliers** (§12).
- **Phase 6+ — learning / intake / output** (§13). Plus deferred: digest rendered into Obsidian,
  cross-time serendipity, tasks/calendar, conversation capture.

Critical path: `0 → 0.5 → 1 → 2`, then `{3,4}` parallel → `5` → `6+`. Phase 1 unblocks everything.

---

## 12. Capability multipliers (build during/after the loops — highest leverage)

Each reuses a Locus layer generic tools lack and serves the founding objective (maximise retrieval
quality), not just vault decoration.

1. **Acceptance-feedback flywheel.** Keep/delete of a suggested `[[link]]` is a free labelled
   relevance judgment. Log `(note, candidate, kept|rejected)` to `acceptance_log`; periodically fold
   into `related_documents` ranking + grow `retrieval_eval.py` `links_recall` labels. The system
   *learns which connections the owner values.* Derived/regenerable — never mutates ingested rows.
2. **Contradiction / tension detection.** Embed a new note's claims → nearest stored **propositions**
   → judge conflict → `> [!ai] Tension` callout (*"you now argue X; in March (doc 412) you concluded
   not-X"*). Novel; the proposition layer uniquely enables it. Advisory, never auto-acted.
3. **Gap-driven reading.** Drive the reading list from `gap_flags` (*"your notes on X flag a gap in
   Y; doc 318 covers Y"*) instead of FIFO.

Further (lower priority): cross-time serendipity (resurface old notes via cross-domain bridges);
question backlog (resurface unanswered questions when a new doc answers them); conversation capture
(ingest selected Claude Code sessions — transcript ingest is a known-open).

---

## 13. Beyond the target — full learning & information system

Additive on the same rails; none changes the spine. Ordering = leverage; all **post-target**.

**13.A Learning layer (highest leverage — exploits propositions / gap_flags / entity graph):**
1. **Active recall from your own propositions** — spaced-repetition prompts from propositions tied
   to engaged material; surfaced in the digest or as a reMarkable warm-up page; answered in prose,
   graded against the source. *The feature that makes this a learning engine.* SM-2 schedule in
   `review_schedule` (agent state, not the spine).
2. **Gap-driven curriculum** — `gap_flags` → an ordered learning path (corpus + 1 external).
3. **Concept-mastery map** — per-canonical-entity engagement depth (written/linked/recalled vs.
   merely filed); a competence heatmap; drives (2).
4. **Synthesis & teaching-back** — periodic Feynman/synthesis prompts graded against the corpus;
   output becomes new corpus.

**13.B Intake breadth (each a new producer feeding the watcher — no engine change):** web capture ·
newsletters/email (Gmail MCP) · lecture/podcast transcripts (known-open) · arXiv/RSS feed
monitoring · voice capture (same pipeline as handwriting).

**13.C Output (renders into Obsidian or pushes to the reMarkable — never a TUI):** grounded writing
assistant (drafts citing your own corpus) · goal briefings ("prep me for X") · the existing
global-news **digest** as an Obsidian note/dashboard · publish (tidy notes → digital garden).

**13.D `_home.md` — the "Today" dashboard (ROUGH DESIGN — deliberately under-built)**

> **Status: rough, and intentionally so.** The biggest risk here is *not* technical — it is that a
> busy dashboard becomes a chore and gets abandoned within a month. This must survive *years* of
> daily use, which means the governing constraint is **restraint, not completeness**. Treat the
> shape below as a starting sketch to prune, not a target to fill. When in doubt, show less.

**Longevity guardrails (these outrank any feature):**
- **Glanceable in ~10 seconds or it dies.** One screen, no scrolling to the "important" part.
- **Hard item caps, ruthlessly.** e.g. ≤3 resurfaced notes, ≤3 reading items, ≤5 recall prompts,
  ≤1 digest line per section. Overflow is *hidden*, not listed — never a wall of backlog.
- **No guilt metrics.** No "47 unread", no streaks, no growing counts. A backlog you can't clear is
  the #1 reason people stop opening a dashboard. Surface *the next thing*, not *everything owed*.
- **Empty is a valid, good state.** A quiet section renders as a single calm line ("nothing due"),
  not an alarm. The dashboard should feel restful, not nagging.
- **Degrades gracefully.** If an agent didn't run or a query returns nothing, the section silently
  collapses — a broken/empty section never breaks the page.
- **It earns its place by replacing hunting**, not by adding a place to look. If it ever becomes a
  *second* chore on top of the vault, cut it back until it's a relief.

**Rough section sketch (prune freely):**

```markdown
# Today — <date>

> [!ai] Digest            # 1–3 lines, the day's update (collapsible, default open)

## Surfaced for you       # ≤3 — the agent's best cross-time / contradiction picks, grounded
- [[old note]] — why it's relevant now (one line)

## Read next              # ≤3 from reading/queue.md, ranked by relevance/gaps — NOT the whole queue
- [[paper]] — one-line why

## Recall                 # ≤5 due prompts (or "nothing due") — also pushable to the reMarkable
- prompt … (answer in-line or on the tablet)

## Loose ends             # ≤3 open followups the agent thinks are answerable now
- …

---
[Triage queue](_dashboard) · [Reading list](reading/queue) · [Mastery](learn/mastery)   # links, not contents
```

**Mechanics (rough):** plain Dataview/Tasks queries over frontmatter the agents already write
(`status: proposed`, `due`, `relevance`), plus the day's `_generated/digests/<date>.md` and
`_generated/resurfaced/<date>.md` transcluded. Everything it shows is produced by components that
already exist in the plan — the dashboard *aggregates*, it does not compute. The heavy surfaces
(full reading list, full triage, mastery map) are **linked, not inlined**, so the home stays a
launchpad, not a control panel.

**Open question (decide when you reach it):** whether the morning "Today" is read in Obsidian, or
pushed to the reMarkable as a one-page warm-up, or both. The layout supports all three; pick by what
you actually open each morning, and let real usage prune the sections.

**Finished shape:** `CAPTURE (reMarkable + web + voice + feeds + email) → grounded linked CORPUS
→ LEARNING (recall · gaps · mastery · synthesis) → OUTPUT (writing · briefings · publishing)`, with
all mutable learning/review/queue state in agent tables and the engine as the brain in the middle.

---

## 14. Failure modes & weaknesses

The dangerous failures are not crashes (loud, fixable) but the **silent** ones: drift,
contamination, eroded trust, atrophied thinking. The invariants (§5) exist to fence these off and
only work if the line is held.

| # | Failure mode | Why it's serious | Mitigation |
|---|---|---|---|
| 1 | **Privacy / egress shift** | Principle 1 was *corpus content never leaves the server except final generation*. This sends **every captured note** to Claude (transcribe/fill-in/enrich) — the most personal layer now transits to Anthropic. | A deliberate, eyes-open trade. Keep corpus + embeddings local; route only what needs a model; optional local-VLM transcription mode for sensitive notes; decide explicitly, not by drift. |
| 2 | **Voice drift / fill-in hallucination** | "Moderate fill-in" puts words in your mouth; read the expansion, forget the raw, and your recorded thinking drifts from what you thought — then you *learn* from the drift. | Marked AI additions + raw preserved (inv. 2/4); periodic raw-vs-filled review; keep fill-in conservative; never let fill-in be the *only* representation. |
| 3 | **Trust erosion** | The moment you can't tell your words from the AI's, you stop relying on the vault. | Airtight provenance; generated notes segregated; never edit human prose. |
| 4 | **Transcription error propagation** | Bad OCR → wrong corpus → wrong retrieval/links/recall. Compounds downstream. | Confidence flagging + MyScript fallback; surface low-confidence spans; keep the source raster for re-transcribe. |
| 5 | **Feedback-loop contamination** | Agent text re-ingested as truth → the system learns from itself; corpus inflates. | Inv. 5: tag + exclude generated content; agents never cite their own generations. |
| 6 | **Outsourcing the thinking** | Organising/linking/summarising is itself where understanding forms; automate it all and you store more while understanding less. | The learning layer must *prompt* cognition (recall, synthesis, teaching-back), not just perform it. |
| 7 | **Suggestion fatigue** | Too many proposals → you ignore all → the triage surface becomes noise. | Fewer, higher-confidence suggestions; the flywheel (§12.1) tunes toward what you keep. |
| 8 | **Operational fragility / SPOF** | rmapi + Syncthing + Ollama + sqlite-vec + Claude CLI + cron + one SQLite DB, one maintainer. A firmware update *will* break rmapi; DB corruption is catastrophic. | `locus backup` is sacred; abstract rmapi behind an adapter; `locus status` health-check; budget for maintenance. |
| 9 | **Third-party dependence** | reMarkable cloud, Anthropic terms/limits, community rmapi — all outside your control. | Thin adapters; keep the email-export transport as a manual fallback. |
| 10 | **Rate-limit / quota starvation** | Background agents consume the quota you rely on interactively. | Budget guard yields to foreground (§9); debounce; batch; monitor, err cautious. |
| 11 | **Syncthing conflicts** | Two-writer edits (owner + agent on one typed file) → conflict copies / lost edits. | reMarkable notes originate server-side (no conflict); typed-note enrichment writes its delimited block atomically → sidecar fallback. |
| 12 | **Scaling** | Brute-force KNN degrades as the corpus grows; more notes = more enrich cost. | ANN index when the count-warning fires (known-open); cost scales with the budget guard. |

---

## 15. Testing & eval

- **Model-free by default** (CLAUDE.md §14): fake `claude` client (canned transcription/fill-in/
  enrich), fake retrieval, seeded tmp DB + vault. Guarded integration tests where a model/device is
  unavoidable.
- **Engine changes gated by existing suites:** run `locus eval --suite retrieval` + `locus audit`
  after the `0009`/`0010`/`0011` migrations and the `rough_weight` knob; grow eval labels;
  **restart `locus mcp`** after retrieval changes (memory: stale servers run old code).
- **New gates:** a Phase-0 transcription-accuracy harness (sample of real handwriting); an
  enrich-grounding check (every suggested link resolves to a real corpus unit — invariant 3).
- **Ops rules carried over:** one ingest process at a time (flock); quarantines are bugs to triage.

---

## 16. Open questions — confirm in Phase 0

1. **Transcription quality** on the owner's actual handwriting (else MyScript-via-email fallback).
2. **rmapi** reliability for pull *and* push server-side (auth, firmware compatibility).
3. **Budget-guard thresholds** — measure per-run token cost; set foreground-yield limits.
4. **Existing digest project** — review what it does before folding it in (§13.C) rather than rebuilding.
5. **Local-VLM transcription mode** — worth offering for sensitive notes (failure mode #1)? Decide
   when privacy posture is set.
