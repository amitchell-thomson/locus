# Locus — capture, learning & idea-generation layer (implementation spec)

> Status: **implementation-ready** (2026-07). This rewrite supersedes the planning draft; the
> requirements-refinement decisions (previously §17, git `e266975`) are folded into the body.
> Development history and the earlier draft live in git.
>
> **This is part of Locus — one repo, one DB.** The ingest+retrieval engine is one component; the
> capture / structuring / surfacing components added here are others. The only boundary that matters
> is internal: **mutable agent state lives in its own tables and never mutates the ingest spine**
> (CLAUDE.md principles 7–9). The corpus stays immutable and regenerable as today.

---

## 1. What this is — and the end goal it serves

A **server-hosted, asynchronous, grounded, propose-never-mutate** layer that turns Locus from a RAG
engine into the owner's **master tool for learning, developing projects, preparing for quant
internships, and generating ideas**. The owner captures (primarily by handwriting on a reMarkable and
by talking to Claude); the server transcribes, lightly enriches, links, and structures that capture
*around* the owner's own words into searchable, linked corpus; the owner then **queries, critiques,
synthesises, and revises** through Claude, and reads a small daily page back on the reMarkable.

**The end goal (what every design choice serves):** help the owner *capture → recall → use* a
continuous stream of information (books, papers, lectures, project work, conversations) to **learn
deeply, sharpen his projects, prepare for interviews, and generate ideas** — and to be usable *daily
for years*. It is architected so it can extend toward **live-trading support** (market theses, trade
ideas, regime-conditioned recall) in ~4–5 years **without a rebuild** (§14).

**The four primary use modes** (owner-ranked; all lean on a well-linked corpus + the existing MCP
surface, which the shipped link-quality work already serves):

1. **Critique partner (co-priority).** Point it at a project or a piece of reasoning; it stress-tests
   against what the owner has read and previously concluded.
2. **Proactive surfacing (co-priority).** It pushes connections/tensions across recent capture that
   *spark* ideas.
3. **Synthesis on demand.** "What do I know/think about X?" → grounded, cited synthesis.
4. **Understanding/revision check.** It finds the gaps in the owner's grasp (interview/exam prep).

**Is not:** a change to the ingest/retrieval *engine*; a second retrieval implementation; an editor of
the owner's prose; a store of mutable state *inside the ingest spine*. Data flows one direction at each
boundary.

**The conditional the whole thing rests on:** value holds *only if* the separation holds — mutable
state out of the spine, generated content tagged and corpus-excluded, one direction per boundary,
propose-never-mutate. Blur any of these and the layer corrupts the invariants that make retrieval
trustworthy.

---

## 2. Locked decisions

| Dimension | Decision |
|---|---|
| **Purpose (now)** | Master **learning / project-development / interview-prep / idea-generation** tool for a quant-track student. Not yet trading. |
| **North star** | Frictionless capture — every thought in with zero ceremony; structure and link later, server-side. |
| **Primary capture** | **Handwriting on a reMarkable** (auto-pulled) + **Claude conversations** (captured on demand). Books/papers/lectures + code repos are the read/ingest inputs. |
| **Interaction surfaces** | **reMarkable** (capture · reading · the pushed daily page) and **Claude** (query · critique · synthesis, via the existing MCP server) are primary. **Obsidian is an optional, deferred visualization projection** (graph exploration). |
| **Structured objects** | First-class **Project · Concept · Question · Reading** over a shared extensible base — **built server-side from freeform capture, agent-proposed and human-blessed, never imposed at write time**. Designed so a **Thesis / Trade-idea** type slots in later. |
| **Headline capability** | **Understanding-evolution** — a dated trajectory of the owner's positions per concept/project, to learn and avoid repeating mistakes. |
| **News** | Enters **only via active reading/annotation** (Loop B on a news PDF). **No ambient news feed** (dilution; a trading-era feature). |
| **Privacy** | **No constraint now** — all material may transit to Claude. Local-only/VLM mode deferred to the trading era. |
| **Ingest model routing** | **Hybrid, per-pass.** Embeddings + math-OCR stay **local** (no Claude equivalent / safe-degrade). Durable LLM passes route to a **cheap Claude model** (Haiku default; Sonnet for judgment-heavy passes on judge-eval evidence). |
| **Hosting** | **Everything on the always-on server.** `claude -p` runs on the owner's subscription; bulk work uses the API Batch endpoint. Nothing pauses when the Mac sleeps. |
| **Codebase** | **One repo = Locus.** Agent components are subpackages; agent state in its own tables (same DB, `locus backup` covers it); ingest spine untouched. |
| **Sync** | reMarkable pull/push (rmapi). **Syncthing is off the critical path** (only needed if/when the optional Obsidian projection is used). |
| **Corpus entry** | All notes enter tagged `maturity` (`rough`\|`tidy`); retrieval **down-weights `rough`** (flag, never filter). Promotion re-ingests at full weight. |

---

## 3. Core capabilities (organised by the end goal)

The value the owner described lives in these capabilities. Capture feeds them; they are what he
actually interacts with. Phasing (§11) brings them forward ahead of the secondary loops.

**3.1 Capture — get everything in with zero ceremony.**
- **Handwriting** (reMarkable): margin notes in books/papers, lecture annotations, rough
  meeting/guest-speaker notes. The dominant stream. → Loop A (§8.1).
- **Claude conversations**: where much of the owner's reasoning and decisions happen. Captured on
  demand and turned into linked corpus. → Loop C (§8.3).
- **PDF annotation**: read + highlight + scribble a paper/FT article on the tablet; the source *and*
  the reactions become corpus. → Loop B (§8.2).

**3.2 Concept-spine — the backbone.** Concepts (canonical entities via `entity_aliases`) tracked
*across* courses, projects, papers, books, and conversations. Already largely built (entity linking +
code-concept extraction, shipped). The structuring and evolution layers hang off it.

**3.3 Structured objects — sharpen recall & track progress.** Agent-proposed, human-blessed overlays
on the immutable corpus (§6.2): **Project** (idea · approach · open threads · learnings), **Concept**
(mastery/engagement state over a canonical entity), **Question** (open, with what raised it / what
answers it), **Reading** (queued/ingested item + state + why). They make queries like "where did we
leave the regime project" and "what am I lacking to explain tanker-flow" first-class.

**3.4 Understanding-evolution — the headline.** A dated chain of the owner's positions per
concept/project ("first thought X → revised to Y after reading Z → now argue W"), plus contradiction
detection. Built on **propositions + concept-spine + timestamps** (§6.3), fed strongly by captured
conversations. Renders as a trajectory in synthesis/critique answers and warns when the owner is about
to repeat a past mistake.

**3.5 Critique / synthesis surface — the primary interaction.** An **enriched MCP surface** so Claude
can, grounded in the owner's corpus + objects + evolution: critique a project, synthesise "what I know
about X", draft/refine CV bullets, and stress-test a view. This is where modes 1–3 (§1) live. It reuses
the retrieval engine; the work is exposing objects/evolution to it (§8.4).

**3.6 Interview-prep aids — gap detection + practice questions.** Over the concept/project objects and
`gap_flags`: mark where the owner's grasp is thin, generate practice questions from his own
propositions, and drive a review schedule. Modes 4. Surfaced in the daily page (§9).

**3.7 The daily reMarkable page — dense, thought-provoking, small, two-way.** The single surface the
owner interacts with most (§9). **Annotatable + reingestable**: handwritten answers/reactions flow back
as graded recall, belief-updates, and flywheel signal.

**Design principle (non-negotiable, guards failure mode #6):** every capability must make the owner
**think MORE, not less** — capabilities *prompt* cognition (recall, critique, evolution), never replace
it.

---

## 4. Invariants (acceptance criteria)

1. **Asynchronous & non-blocking.** Capture is instant; structuring/enrichment catch up out-of-band.
   No user action ever blocks on a model call.
2. **Propose, never mutate.** Agents write only into clearly-owned `> [!ai] …` blocks inside human
   notes (§10) and into agent-owned notes/objects. They never edit the owner's prose. Structured
   objects are **proposed**; the owner blesses (`status: active`) or ignores.
3. **Grounded or silent.** Every suggested link / connection / critique claim / practice question /
   evolution pointer cites a real corpus unit from retrieval, or it does not appear.
4. **Provenance is structural & glanceable.** Everything an agent produces is callout-marked and
   carries frontmatter/row provenance (`author: agent`, `source_run`, `generated_at`).
5. **No feedback-loop contamination.** Agent-generated content ingested by Locus is tagged so
   retrieval can weight/exclude it; agents never treat their own generations as ground truth.
   `_generated/` is corpus-excluded.
6. **One direction per boundary.** reMarkable → pull → transcribe/enrich → capture inbox → ingest → DB
   → (compose) → pushed daily page. The reMarkable read-push is a *separate one-way delivery channel*;
   only fresh handwriting returns, via capture. The agent never round-trips the read-only Obsidian
   projection.

---

## 5. Architecture (server-hosted)

```
   SERVER (always-on; runs everything)
   ├─ rmapi  ◀── pull ── reMarkable (capture: handwriting + PDF annotations OUT)
   │          ── push ─▶ reMarkable (reading IN: papers · the daily page · docs → annotate)
   ├─ Locus (one system, one repo, one DB):
   │    • engine (existing):  ingest spine · retrieval · link layer · MCP server
   │    • new components:      agent/ capture/ structure/ evolve/ enrich/ reading/ vault/
   │    • agent-state tables:  objects · object_links · belief_positions · review_schedule
   │    │                      · acceptance_log · agent_runs   (same DB, separate tables)
   │    • orchestration:       claude -p for language tasks; grounding via the LOCAL retrieval
   │    │                      engine IN-PROCESS; systemd timers + on-demand CLI
   │    • MCP surface:         retrieve/query/inspect (existing) + capture + critique/synthesise
   │                           + object/evolution reads (new)
   └─ (optional, deferred) Obsidian projection ──Syncthing──▶ Mac + phone
```

**Surfaces.** **reMarkable** = capture + reading + the pushed daily page. **Claude** = query / critique
/ synthesis, via the MCP server the owner already connects to. **Obsidian** = optional visualization of
the graph, turned on later; the read-only exporter already exists, so it costs nothing to keep as an
option. **Syncthing is not on the critical path** — it appears only with the optional Obsidian layer.

**Consequences:** (1) nothing pauses when the Mac sleeps; (2) the agent grounds against the **local**
engine (no SSH in the agent layer — MCP-over-SSH stays only for the owner's interactive Claude
clients); (3) one codebase, one DB, one `locus backup`.

---

## 6. Data model

### 6.1 Engine change (the only spine-adjacent write)

- **`documents.maturity`** (`rough`\|`tidy`), migration **0009**, + `[retrieve].rough_weight`.
  Down-weight `maturity=rough` **at merge** (a multiplier on the pre-rerank score) — **flag/down-weight,
  never filter** (keeps the recoverable-class property, principle 8). This is the one genuinely new
  retrieval behaviour; gated by the retrieval eval (rough notes must neither drown signal nor be
  buried). Default `rough_weight` set in Phase 1 and eval-tuned.

### 6.2 Structured objects (agent-state; migration 0010)

Objects are **agent-owned overlays that reference the immutable corpus** — never copies of it. They
carry human decisions (blessing, mastery), so they are *not* purely regenerable; they are backed up
with the DB and versioned by `agent_runs`.

```
objects        id · type (project|concept|question|reading|…) · title · status (proposed|active|archived)
               · maturity · body (JSON, type-specific fields) · created_at · updated_at · source_run
object_links   object_id · target_kind (doc|entity|object) · target_key (source_uri | canonical (name,type) | object_id)
               · relation (implements|about|raised_by|answered_by|reads|relates)
```

- **Project** → links to a repo doc (`source_uri`); `body` holds `{approach, open_threads[], learnings[]}`.
- **Concept** → links to a canonical entity (`(name,type)`); `body` holds `{mastery, last_engaged, engagement:[read|linked|used|recalled]}` — the mastery map is a query over these.
- **Question** → free-standing; `object_links` capture `raised_by` / `answered_by` docs.
- **Reading** → links to a queued/ingested doc; `body` holds `{state: queued|reading|done, why}`.

Extensible: a new type is a new `type` value + a `body` shape; **Thesis/Trade-idea** is a future type
(`{claim, evidence[], catalysts[], risks[], invalidation, positioning}`) with no migration.

### 6.3 Understanding-evolution (agent-state; migration 0010)

```
belief_positions  id · subject_kind (concept|project) · subject_key (canonical (name,type) | object_id)
                  · stance (text — the owner's position, in his words) · source_doc_id · source_run · dated_at
```

`dated_at` is the *source note/conversation's* date, so the trajectory is chronological by when the
owner actually thought it. The evolution view is `ORDER BY dated_at` per subject; contradiction
detection embeds a new stance and flags near-neighbour stored propositions/positions of opposite
polarity (a `> [!ai] Tension` callout, advisory). Reuses the existing **propositions** layer for the
grounded claim substrate.

### 6.4 Learning/feedback state (agent-state; migration 0010)

```
review_schedule  id · prompt_ref (proposition_id | question object) · due · ease · interval   (SM-2)
acceptance_log   id · surface (link|connection|reading|recall) · candidate_key · verdict (kept|rejected) · at
agent_runs       id · kind · started_at · finished_at · status · stats (JSON)   (journal; crash-safe)
```

`acceptance_log` is the flywheel substrate (§12): keep/reject of a proposal is a free relevance label
that folds into `related_documents` ranking and grows `links_recall` labels — derived/regenerable,
never mutates ingested rows.

### 6.5 Annotations (agent-state; migration 0011, Phase 3)

```
annotations  id · source_doc_id · page · type (highlight|underline|margin) · text · anchor_chunk_id · owner_note_id
```

Anchors highlights/margins to source chunks so retrieval can surface "you highlighted this passage"
alongside the paper's own chunk. Append-only owner data; does not mutate the spine.

### 6.6 Vault layout (capture inbox + agent output; server-authoritative)

```
LocusVault/                         # server-authoritative
│  ── HUMAN-OWNED (you write; agents append only owned `> [!ai]` blocks) ──
├── daily/<YYYY-MM-DD>.md           # typed/quick capture inbox
├── notes/                          # atomic notes (handwriting→transcribed, or typed); maturity in frontmatter
├── annotations/                    # "Annotations on <paper>" notes (Loop B)
├── conversations/                  # captured Claude conversations (Loop C), maturity=rough
│  ── AGENT-OWNED (regenerable; corpus-excluded, invariant 5) ──
├── _generated/                     # surfaced-connections, critique outputs, trajectory notes
├── _home.md                        # the daily page (composed; also rendered → reMarkable)
│  ── OPTIONAL, deferred ──
└── _locus/                         # read-only Obsidian projection (exporter-owned; only if enabled)
```

The **capture inbox** (`notes/`, `annotations/`, `conversations/`) is a `note`-category ingest source
(§7). Agent output (`_generated/`, `_home.md`) is corpus-excluded and regenerated wholesale.

### 6.7 Authoring vault as an incremental `note` source

`note` category + `vault/notes/` input already exist. Ingest the inbox **incrementally** on the
`repo_sync.py` blob-manifest-diff template (notes churn — never full re-ingest per save):
**normalise whitespace before hashing** (CLAUDE.md §11 idempotency limit), reuse the `watch`
settle-window debounce, and **exclude `_generated/`** and `_home.md`.

---

## 7. Ingest model routing (hybrid, per-pass)

Not "ditch local" — **route each pass to the engine that fits**, configured in `[ingest].pass_routing`
and recorded per doc (`ingest_model` already exists). `agent/claude.py` (§10) is the Claude runner;
passes routed to `local` use the existing qwen path.

| Pass | Default engine | Rationale |
|---|---|---|
| embeddings (`nomic`, 768-dim) | **local (forced)** | No Claude embeddings model exists — non-negotiable. |
| math-OCR (GOT) | **local (forced)** | Chosen for safe-degrade; Claude vision can *invent* math (wrong failure mode). |
| section summaries | haiku | High volume, low stakes; Haiku ≫ 8B. |
| propositions · entities | haiku → **sonnet on eval evidence** | The §11.B ceiling; the durable structure belief-evolution/critique depend on. |
| synthesis · gaps · concepts | haiku | Judgment-ish; Haiku is a clear step up. |
| figure VLM | haiku-vision or local qwen2.5vl | Configurable; low volume. |

**Channels.** **Bulk re-ingest → API Batch** (−50%, no latency budget per CLAUDE.md principle 4;
~$15–50 one-time — *estimate; size in Phase 0*), avoiding subscription rate-limit/ToS pressure.
**Ongoing daily (low volume) → subscription `claude -p`**, arbitrated by the budget guard (§10), or
pay-as-you-go (~$15–25/mo). The **judge eval** measures per-pass quality; escalate a pass to Sonnet
only on that evidence (§11.B: "revisit per-pass API routing only on eval evidence").

---

## 8. The capture loops & the critique surface

### 8.1 Loop A — rough-note enrichment & store (Phase 1)

```
CAPTURE handwrite (zero decisions) → PULL rmapi → TRANSCRIBE claude-vision (MyScript fallback)
→ FILL-IN moderate, AI marked, raw preserved → ENRICH grounded > [!ai] Related → FILE to notes/
→ INGEST as note (maturity=rough) → STRUCTURE (propose objects, §8.4) → belief positions (§6.3)
```

New: `capture/{remarkable,transcribe,fillin}`, `enrich/related`, `vault/writer` (§10 protocol),
`agent/{claude,budget,run,journal}`. Gate: a real handwritten note appears enriched in the inbox *and*
searchable in Locus as `rough`.

### 8.2 Loop B — PDF annotate & store (Phase 3)

```
1 EXPORT  any PDF → reMarkable (rmapi put). Source: reading object, `locus read <pdf>`, an arXiv id, an FT PDF.
2 READ    highlight · underline · handwrite margins.
3 PULL    rmapi get → original + annotated PDF + stroke data.
4 EXTRACT highlights → map rects to the PDF text layer (pymupdf; region-OCR if no text layer);
          margins → region PNG → claude-vision transcribe; underlines/boxes → nearest-chunk markers.
5 STORE   source → normal spine (source_type=pdf, hash-idempotent); annotations → "Annotations on <paper>"
          note (maturity=rough) + annotations table (0011) anchored to source chunks.
6 ENRICH  margins get the normal treatment; highlights feed reading-rank + the flywheel.
```

News enters here (an FT/Economist PDF the owner actually reads), never as a feed. Reuse: rmapi (§8.5),
pymupdf, claude-vision, the PDF+note spine, enrich/link. Risks: `.rm` format drift (renderer behind an
adapter), no-text-layer highlight mapping (region-OCR fallback), ink error (confidence-flag, keep the
raster).

### 8.3 Loop C — Claude-conversation capture (Phase 1)

Where much of the owner's reasoning lives — captured on demand, curated.

```
CAPTURE (three entry points) → conversations/<slug>.md (maturity=rough) → INGEST → STRUCTURE + belief positions
```

- **MCP `capture` tool (core, cross-client).** On the existing Locus MCP server; works from Claude
  Code, claude.ai, phone — "save this to Locus" mid-conversation. Writes the exchange (or a
  decision-summary the tool asks Claude to produce) to `conversations/`. **Write-to-inbox only — never
  mutates the spine directly** (goes through normal ingest); invariant-clean.
- **Claude Code skill `/locus-capture` (ergonomic).** Thin wrapper: summarises the session's
  decisions, tags the project, calls the MCP tool.
- **Batch importer.** Pull a flagged Claude Code `.jsonl` transcript from disk retroactively.

Curation is the point — the owner picks what's worth keeping, which keeps noise out and matches
propose-never-mutate. These captures are prime fuel for belief-evolution ("in this session I decided X
because Y").

### 8.4 Structuring & the critique/synthesis surface (Phase 2)

- **`structure/propose.py`** — after a note/conversation/repo ingests, an agent pass proposes/updates
  **objects** (§6.2) and **belief positions** (§6.3), grounded in retrieval, `status: proposed`. The
  owner blesses via a lightweight action (a daily-page item or an MCP call).
- **`evolve/trajectory.py`** — renders the dated position chain per concept/project and runs
  contradiction detection (advisory `> [!ai] Tension`).
- **MCP surface (the primary interaction).** New tools alongside `retrieve/query/inspect`:
  `critique(target)` (project/reasoning → grounded stress-test), `synthesise(topic)` (grounded "what I
  know about X" incl. the trajectory), `objects(...)` / `evolution(...)` reads. Each **grounds
  in-process** (local retrieval) and passes candidates to `claude -p`. This is what makes the owner's
  four example queries work — CV bullets + interview gaps for a project, "where did we leave regime-ml
  given what I learned at Brevan Howard", "everything on portfolio construction + practice questions".
- **`learn/{gaps,practice,review}.py`** — gap detection over concept objects + `gap_flags`;
  practice-question generation from the owner's propositions; SM-2 `review_schedule`. Surfaced in the
  daily page.

Gate: the four example queries return grounded, cited, useful answers; a concept's trajectory renders.

### 8.5 Foundation — `locus read` (Phase 0.5)

`locus read <path|dir> [--format epub]` renders markdown → device-tuned PDF (pandoc; ~1404×1872,
~226 dpi) and `rmapi put`s it to a device folder. Useful day one (read any repo doc on the tablet) and
**proves the rmapi push channel** every loop and the daily page depend on. Deps: one-time rmapi auth;
a PDF engine (tectonic/weasyprint).

---

## 9. The daily reMarkable page (Phase 3)

The single surface the owner touches most. **Dense and thought-provoking, ruthlessly small, two-way.**
It **prompts thinking** — it is *not* a passive ingestion log.

**Compose** (`agent/compose_daily.py`, aggregates existing components — does not compute):
- **≤3 surfaced connections/tensions** (proactive surfacing over recent capture + concept-spine +
  trajectory) — each with a one-line grounded "why now".
- **≤5 recall / interview-practice questions** (due `review_schedule` items + practice generation).
- **≤3 read-next** (reading objects ranked by gaps).

Rendered to `_home.md` and pushed as a PDF to `Locus/Daily` with **stable per-anchor markers**
(numbered answer regions) so pull-back maps handwriting to the right item.

**Reingest the annotations** (the elegant part — makes "one surface" work):
```
PULL annotated daily page → extract handwriting per anchor (claude-vision) → route:
  recall answers      → grade against source (claude) → update review_schedule (SM-2)
  connection reactions → new belief_positions / notes; kept|ignored → acceptance_log (flywheel)
  new questions        → Question objects
```
Idempotent by `(daily date, anchor)` — re-pull updates, never duplicates.

**Longevity guardrails (outrank any feature; failure mode #7):** glanceable in ~10s; hard item caps;
**no guilt metrics** (no "N unread", no streaks); empty is a valid, calm state; degrades silently if an
agent didn't run; earns its place by *replacing hunting*, not adding a chore. Exact composition tuned
during build. **Open (decide from real usage):** read in Obsidian vs. the pushed page vs. both.

---

## 10. Orchestration, the `claude -p` contract & the owned-block protocol

**Grounding decision.** The Python orchestrator **grounds in-process** by calling the local retrieval
engine directly (deterministic, testable, free); **`claude -p` is used only for language tasks** —
transcription, fill-in, enrich phrasing, critique/synthesis, object proposal, trajectory summary,
recall grading — with retrieved candidates passed in the prompt. MCP-over-SSH is **not** used by the
agent layer.

**The `claude -p` task contract (`agent/claude.py`, reusing the shipped `link/adjudicate.py` pattern).**
One shared runner, one shape for every task:
- `run(prompt, *, model, schema) -> pydantic` — shells `claude -p <prompt> --output-format json
  --model <model>`, `cwd=` a neutral temp dir (no repo CLAUDE.md / project MCP), parses the envelope
  `.result`, then slices the first `{`…`}` and validates against a pydantic `schema` (tolerant of prose
  around the JSON).
- Each task = a function building `(prompt, grounding_context, schema)`; **bounded repair retries**,
  then **degrade** (keep raw, flag a gap) — never block capture, never abort a batch (mirrors ingest
  §7).
- Injectable runner so every task is unit-testable without a subprocess (as `adjudicate` already is).
- Bulk work uses the **API Batch** runner instead of `claude -p` (§7).

**Budget guard (`agent/budget.py`).** Wraps every subscription call; yields to foreground interactive
use (background work pauses, doesn't starve it); hard debounce + batching. Detection mechanism is a
Phase-0 spike (parse rate-limit errors / a local token ledger / off-peak scheduling) — a genuine
unknown to resolve before relying on it.

**Triggering.** Scheduled systemd timers (pull the reMarkable every 15–30 min; overnight batch
enrich/structure; periodic flywheel/mastery folds); on-demand `locus capture-sync`; the MCP `capture`
tool (event-driven). Each run is journaled (`agent_runs`); crash mid-run wastes no spend
(per-verdict/per-object commits, as `link`/`retitle` already do).

**Idempotent re-pull.** reMarkable docs keyed by device doc-id + **per-page content hash**
(re-transcribe only changed pages); PDF annotations keyed by `(doc, annotation-layer hash)` (re-pull
**updates** stored annotations, no duplicates).

**Owned-block write protocol (`vault/writer.py`) — the correctness linchpin.**
- **Markers.** Agent-owned blocks in a human note are delimited by HTML-comment sentinels keyed by
  `(note_path, block_kind)`:
  `<!-- locus:ai:<kind>:start run=<run_id> -->` … `<!-- locus:ai:<kind>:end -->`.
- **Idempotent regeneration.** A block is regenerated **wholesale** — locate the marker pair, replace
  the span (append a fresh block if absent). Re-runs replace, never accumulate.
- **Atomic write.** Read → hash → edit only the marked span → write to a temp file in the *same dir* →
  fsync → `os.replace` (atomic rename). Never touches bytes outside the markers.
- **Conflict handling (only relevant if Obsidian/Syncthing is enabled).** reMarkable- and
  conversation-originated notes are **server-authored** → no two-writer conflict. For a typed note, if
  the file changed since the read-hash, or a `*.sync-conflict-*` copy exists, **fall back to a sidecar
  enrichment note** (`<note>.locus.md`) rather than risk clobbering the owner's prose.
- **Provenance.** Generated notes carry frontmatter `author: agent`, `generated: true`, `source_run`,
  `generated_at`; owned blocks carry the run id in the marker (invariant 4).

---

## 11. Module layout, migrations & config (Phase 0–3 scope)

Only what Phases 0–3 touch is drawn here; deferred components (§13) are *not* pre-created.

```
locus/
├── (engine, existing)  extract/ ingest/ retrieve/ link/ export/ query.py mcp_server.py db/
├── agent/        claude.py (headless + Batch runners) · budget.py · run.py · journal.py · compose_daily.py
├── capture/      remarkable.py (rmapi pull+push) · transcribe.py · fillin.py · conversations.py · annotations.py
├── structure/    propose.py (objects, agent-proposed)
├── evolve/       trajectory.py (belief positions + contradiction)
├── enrich/       related.py (grounded > [!ai] blocks)
├── learn/        gaps.py · practice.py · review.py (SM-2)
├── reading/      md2pdf.py · deliver_remarkable.py   # locus read + daily-page push
└── vault/        writer.py (owned blocks, atomic, provenance) · markers.py · sidecar.py
```

**Migrations (forward-only; head 0008):**
- **0009** — `documents.maturity TEXT` (`rough|tidy`).
- **0010** — agent-state: `objects`, `object_links`, `belief_positions`, `review_schedule`,
  `acceptance_log`, `agent_runs` (same DB → `locus backup` covers them; separate from spine tables).
- **0011** — `annotations` (Phase 3).

**Config (all optional, default clean):** `[retrieve].rough_weight`, `[ingest].pass_routing`,
`[capture]` (rmapi target, schedule, transcription thresholds), `[learn]`, `[agent]` (budget
thresholds, schedules), `[daily]` (item caps).

**New CLI verbs:** `locus read` (§8.5) · `locus capture-sync` (manual pull→transcribe→enrich→structure)
· scheduled internal entrypoints (systemd timers). **New MCP tools:** `capture`, `critique`,
`synthesise`, `objects`, `evolution`.

---

## 12. Capability multipliers (build during/after the loops — highest leverage)

Each reuses a Locus layer generic tools lack; each serves the founding objective, not vault decoration.

1. **Acceptance-feedback flywheel.** Keep/reject of a suggested `[[link]]`, connection, reading, or
   recall answer is a free labelled relevance judgment (`acceptance_log`, §6.4). Periodically fold into
   `related_documents` ranking + grow `retrieval_eval.py` `links_recall` labels. *The system learns
   which connections the owner values.* Derived/regenerable — never mutates ingested rows.
2. **Contradiction / tension detection** (the evolution engine, §6.3). Advisory, never auto-acted.
3. **Gap-driven learning.** Drive practice + reading-rank from `gap_flags` + concept mastery, not FIFO.

---

## 13. Deferred (post-Phase-3) — the broader system

Additive on the same rails; none changes the spine. Ordering = leverage; all after the value core +
loops are trusted.

- **Organise + secondary capture.** Full-auto title/tag/file/split (reversible, journalled); daily-note
  hotkey + phone capture into the same pipeline.
- **Reading list — external.** Corpus + arXiv/citation-following suggestions; queue with auto-ingest;
  push delivery; promotion rough→tidy.
- **Obsidian visualization projection.** Turn on the existing read-only export + Syncthing for graph
  exploration (a "cool display", not load-bearing). Deferred by decision.
- **Learning layer extras.** Synthesis / teaching-back (Feynman prompts graded against the corpus);
  concept-mastery heatmap surface.
- **Intake breadth.** Web capture · newsletters/email (Gmail MCP) · lecture/podcast transcripts ·
  voice capture (same pipeline as handwriting). **No ambient news feed** (dilution; trading-era).
- **Output.** Grounded writing assistant (drafts citing the owner's corpus) · goal briefings · the
  existing global-news digest folded in after review · publish (tidy notes → digital garden).
- **The `_home.md` "Today" dashboard as a rich Dataview surface** (beyond the pushed daily page) — kept
  deliberately under-built; the longevity guardrails (§9) govern.

---

## 14. Trading-era extension seams (design now, build later)

The system is architected so the ~4–5-year pivot to live-trading support is additive, not a rebuild:
- **Objects (§6.2):** a **Thesis / Trade-idea** type slots in as a new `type` + `body` shape — no
  migration. Outcome-tracking is a `body` field + a `belief_positions`-style journal.
- **Evolution (§6.3):** the same dated-position machinery becomes "you held this view in a similar
  regime, and here's how it played out" once a **market-regime/date-context** dimension is added to the
  position rows.
- **News (§8.2):** the annotate loop already turns read FT/Economist PDFs into corpus — the news→thesis
  pipeline is the same path plus (later) an ambient feed.
- **Privacy (§2):** the deferred local-only/VLM mode returns as the routing gate for anything
  MNPI/firm-confidential; the per-pass routing (§7) already has the seam.

---

## 15. Testing & eval

- **Model-free by default** (CLAUDE.md §14): fake `claude` runner (canned transcription/fill-in/enrich/
  critique/structure), fake retrieval, seeded tmp DB + vault. Guarded integration tests where a
  model/device is unavoidable.
- **Engine changes gated by existing suites:** run `locus eval --suite retrieval` + `locus audit` after
  the `0009`/`0010`/`0011` migrations and the `rough_weight` knob; grow eval labels with the new
  corpus; **restart `locus mcp`** after retrieval/surface changes (stale servers run old code).
- **New gates:** a Phase-0 **transcription-accuracy** harness (WER threshold on the owner's real
  handwriting → go/no-go); an **enrich-grounding** check (every suggested link resolves to a real
  corpus unit — invariant 3); an **object-proposal precision** check (proposed objects are grounded and
  not duplicates); a **routing judge** run (per-pass Haiku-vs-local-vs-Sonnet quality delta — drives
  §7 escalation).
- **Ops rules carried over:** one ingest process at a time (flock); quarantines are bugs to triage.

---

## 16. Failure modes & weaknesses

The dangerous failures are the **silent** ones — drift, contamination, eroded trust, atrophied
thinking. The invariants (§4) fence these off and only work if the line is held.

| # | Failure mode | Why serious | Mitigation |
|---|---|---|---|
| 1 | **Voice drift / fill-in hallucination** | "Moderate fill-in" puts words in your mouth; you learn from the drift. | Marked AI additions + raw preserved; conservative fill-in; periodic raw-vs-filled review. |
| 2 | **Trust erosion** | The moment you can't tell your words from the AI's, you stop relying on the vault. | Airtight provenance; generated notes segregated; never edit human prose. |
| 3 | **Transcription-error propagation** | Bad OCR → wrong corpus → wrong recall/links/critique. | Confidence-flag + MyScript fallback; keep the source raster; Phase-0 WER gate. |
| 4 | **Feedback-loop contamination** | Agent text re-ingested as truth → the system learns from itself. | Tag + exclude `_generated/`; agents never cite their own generations. |
| 5 | **Capture density is make-or-break** | Every capability is downstream of what's captured and how well it's linked. | Phase-0 transcription proof is a real go/no-go; the two quick wins (`locus read`, conversation capture) pay off while the corpus accretes. |
| 6 | **Outsourcing the thinking** | Automate all organising/linking/summarising and you store more while understanding less. | §3 principle: capabilities *prompt* cognition (recall, critique, evolution), never replace it. |
| 7 | **Suggestion fatigue / dashboard abandonment** | Too many proposals or a busy daily page → ignored → noise. | Fewer, higher-confidence proposals; the daily-page guardrails (§9); the flywheel tunes toward what's kept. |
| 8 | **§11.B extraction ceiling** | Belief-evolution/critique are only as sharp as propositions/entities. | Hybrid routing (§7) lifts the durable passes off the 8B model; validation + grounding + raw co-assembly. |
| 9 | **Rate-limit / ToS pressure on the subscription** | Bulk ingest via `claude -p` competes with foreground + is a grey area. | Bulk → API Batch; ongoing → budget-guarded `claude -p`; the guard yields to foreground. |
| 10 | **Operational fragility / SPOF** | rmapi + Ollama + sqlite-vec + Claude + cron + one DB, one maintainer; firmware breaks rmapi; DB corruption is catastrophic. | `locus backup` is sacred; rmapi behind an adapter; `locus status`; email-export fallback. |
| 11 | **Two-writer conflicts** | Owner + agent on one typed file (only if Obsidian/Syncthing enabled). | reMarkable/conversation notes are server-authored (no conflict); typed-note enrichment is atomic owned-block → sidecar fallback (§10). |
| 12 | **Scaling** | Brute-force KNN degrades; more notes = more enrich cost. | ANN index when the count-warning fires (known-open); cost scales with the budget guard + Batch. |

---

## 17. Phasing

**Critical path: `0 → 0.5 → 1 → 2 → 3`.** Phase 1 (capture) unblocks Phase 2 (the value surfaces the
owner actually described wanting); Phase 3 adds the reMarkable loops on top. Everything in §13 waits.

- **Phase 0 — verify + size (go/no-go).** (a) rmapi pull *and* push server-side; (b) **transcription
  quality on the owner's real handwriting** (WER gate; MyScript fallback if poor); (c) **per-doc token
  cost + per-pass routing** spike (Haiku vs local vs Sonnet on the judge eval; bulk-reingest $); (d)
  **budget-guard detection** mechanism. Throwaway scripts in `scripts/`.
- **Phase 0.5 — `locus read`** (§8.5). Quick win + proves the push channel.
- **Phase 1 — capture foundation.** Loop A (rough note, §8.1) + Loop C (conversation capture, §8.3) +
  engine changes (0009 maturity, incremental note ingest §6.7, hybrid routing §7) + the owned-block
  protocol (§10). *Gate: handwriting and conversations land as enriched, linked, searchable corpus.*
- **Phase 2 — the value surfaces (the owner's stated priority).** Structured objects (§6.2), concept-
  spine consolidation, understanding-evolution (§6.3), the **critique/synthesise MCP surface** (§8.4),
  interview-prep aids (gap detection + practice generation). *Gate: the four example queries return
  grounded, useful answers; a concept's trajectory renders.*
- **Phase 3 — the reMarkable loops.** Loop B (PDF annotate, §8.2, 0011) + the **annotatable/reingestable
  daily page** (§9) + the acceptance flywheel (§12.1). *Gate: annotate a PDF and a daily page; both
  reingest with feedback; recall grading + flywheel signal recorded.*
- **Later — §13**, in leverage order, once the core is trusted.

---

## 18. Open items (specify in build / measure in Phase 0 — not blocking)

Named so nothing is dropped; none is a decision owed by the owner.
- **Phase-0 measurements:** transcription WER threshold; real per-doc token cost + bulk-reingest $;
  per-pass Haiku-vs-Sonnet routing; budget-guard detection mechanism.
- **Specify in the build:** exact `rough_weight` value + curve; highlight→chunk anchoring precision +
  region-OCR fallback; exact daily-page composition (against §9 guardrails); the object-proposal
  prompt/precision bar.
- **Decide from real usage:** daily page read in Obsidian vs. pushed to the reMarkable vs. both (§9).
