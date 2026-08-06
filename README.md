# Locus

A self-hosted knowledge system that holds everything its owner has read, written and built,
and returns it when it is useful.

Locus does three things: it **answers questions** over a personal corpus with grounded citations,
it **surfaces connections** across that corpus, and it **feeds the owner's own material to Claude**
on demand. Corpus content never leaves the machine; the only runtime network egress is the
generation call.

It is not a note-taking application. There is no GUI. The product surfaces are a terminal, an MCP
server, and a sheet of e-paper.

---

## The daily loop

The system's centre of gravity is a PDF delivered to a reMarkable tablet each morning, and the
handwriting that comes back.

Each page is named for what he does on it, and an empty one is simply not printed.

| Page | What it offers |
| --- | --- |
| **Develop** | His own open threads — ideas and questions he has raised, each with what he has already written on it |
| **Consider** | Connections the system found, each written as a concrete proposal: how *this* paper's method bears on *that* project of his |
| **Answered** | Questions he wrote in margins while reading, answered from his own library, with the supporting passage cited |
| **Recall** | Spaced-repetition questions on concepts he has met; answers overleaf, never beside the prompt |

He writes on it. The next pull reads the ink back — geometry decides which region each stroke
belongs to, and a printed anchor routes it to the right record. A tick resolves; a cross drops;
prose **develops**, appending to a chain rather than overwriting it. Anything he develops is
written out as a note and re-ingested, so his own thinking becomes searchable and can return to
him later as a connection.

There is no reading page: it duplicated a folder he already browses, and what he actually wrote on
it was project thinking about the papers. The written reason each proposal is on the shelf now sits
*in* the shelf, as a document beside the papers themselves.

The page is composed from stored data only — no model call at composition time — so it renders
whether or not the previous night's work succeeded. Every piece of prose on it was written and
stored by an earlier pass.

### Reading

Marks made while reading are read from the tablet's stroke geometry, not from a screenshot. Shape
decides the gesture (underline, bracket, highlight, margin note) and position binds it to the
exact passage. A written note beside a mark is transcribed and classified into one of three
intents:

- **important** — stays retrievable; nothing is pushed at him
- **not understood** — answered on the Answered page from his own corpus
- **an idea** — becomes a tracked thread, linked to the project it concerns

Low-confidence classifications are not acted on; they become a decision in the terminal instead.

The whole chain runs unattended, every half hour. A book he added to the tablet himself is
recognised by the content hash of its own bytes, ingested the first time he moves it into
*In-Progress*, and his margin notes are written out as a note and re-ingested — so material he
chose reaches retrieval without a command being typed.

---

## Under the surface

**Ingest.** PDFs, DOCX, slides, notebooks, markdown and whole code repositories reduce to one
schema: documents → sections → chunks, plus atomic claims, typed entities and figures, all
embedded. Mathematics is recovered by OCR where the PDF text layer has lost it. Local models do
the extraction; ingest has no time budget, and quality is never traded for throughput.

**Retrieval.** Dense vector search across claims, sections, chunks and figures, combined with
keyword search and an entity-aware arm that understands when two names mean the same thing.
Candidates are reranked by a cross-encoder, filtered for diversity, expanded with parent context
and assembled coarse-to-fine into a single generation call. Queries that bridge two fields are
rephrased into each field's vocabulary, so a match written in unfamiliar terms still surfaces.

**Linking.** A derived alias layer canonicalises entity names — deterministic rules first, a
judged pass for the ambiguous remainder. This is what allows engineering coursework to connect to
quantitative research, and what lets separate threads of the owner's thinking find each other.
Documents are then ranked against each other by the concepts they share, weighted by how rare each
concept is corpus-wide, so a term half the library uses cannot pass for a connection. Whether a
pair is genuinely connected is a fact about shared concepts and stays deterministic; only the
explanation of *how* they connect is written by a model, from stored text, naming a concept it was
given.

**Discovery.** Weekly searches of arXiv and OpenAlex, using concepts drawn from what he has marked
and what his live projects implement. Candidates are ranked against stored profiles of his own
work; survivors are delivered to the tablet with a written explanation. Moving a paper out of the
proposed folder is the accept signal.

---

## Other surfaces

| Command | Purpose |
| --- | --- |
| `locus query` | A grounded, cited answer over the whole corpus |
| `locus mcp` | Serves the corpus to Claude over stdio; retrieval is local and free |
| `locus decide` | The single approval surface — the only place a status changes |
| `locus status` | One screen: corpus health, spend, timer state, staleness warnings |
| `locus gates` | What each internal threshold rejected, so a dead one becomes visible |
| `locus reading-why` | Delivers the shelf's own rationale document to the proposed-reading folder |
| `locus evolution` | The dated trajectory of a position, and where the corpus contradicts it |
| `locus backup` / `restore` | WAL-safe snapshots with a hard-linked raw store; restore is tested |
| `locus export-obsidian` | A read-only graph projection for visual exploration |

---

## Design commitments

1. **Local data ownership.** Corpus content stays on the server.
2. **Quality over speed at ingest.** Extraction loss is unrecoverable; latency is not.
3. **Grounded or silent.** Every claim, connection, answer and contradiction cites a real stored
   unit, or it is not shown. Silence is the better failure.
4. **Propose, never mutate.** The agent layer writes to its own tables and proposes; the owner
   decides. The ingested corpus is never edited by derived layers.
5. **Derived data is regenerable.** Aliases, caches and projections can be deleted and rebuilt;
   the ingested tables are the only source of truth.
6. **Nothing is shown twice**, and nothing is measured back at him. No unread counts, no streaks.

---

## Stack

Python 3.11+ with uv. SQLite with `sqlite-vec` for vectors and FTS5 for keyword search; Alembic
for forward-only migrations. Local models via Ollama for extraction and embeddings, a
cross-encoder on CPU for reranking, and the Claude API or `claude -p` for generation and
judgement.

The hardware ceiling — a single 8 GB GPU — is the constraint that shapes the design: local models
for ingest, hosted models only where an error would corrupt durable state.

---

## Usage

```bash
# ingest anything
locus ingest path/to/file.pdf
locus watch                       # continuous; category from the drop folder
locus sync                        # tracked code repositories

# ask
locus query "how do regime-switching models relate to state-space models?"
locus retrieve "covariance estimation" --json

# the daily loop
locus daily                       # compose and deliver today's page
locus daily-pull                  # read the ink back and route it
locus decide                      # approve what is pending
locus capture-sync                # pull handwriting and reading annotations off the tablet

# maintain
locus link                        # rebuild the alias substrate
locus status                      # health, spend, warnings
locus gates --days 7              # what the thresholds rejected
locus backup

# serve to Claude
locus mcp
```

Most of this runs unattended on systemd timers; the manual commands exist for when something
needs forcing.

---

## Status

In daily use. 232 documents, ~17,000 stored claims, ~25,000 typed entities, ~2,200 cross-document
concepts. Operating cost is well under a pound a day. The corpus is deliberately weighted toward
study material, because that is where the foundations bridging into current work are found.

Evaluation covers labelled retrieval recall, cross-domain bridging, link recall, mathematical
fidelity and a deterministic per-document audit. The test suite is about 1,050 tests, model-free
by default.

The recurring failure this system is built to resist is a path that *looks* wired and is not: a
threshold that admits nothing, a cached verdict that outlives the judge that produced it, a signal
that reaches no surface. Several such paths have been found and closed. `locus gates` exists so
that the next one is visible rather than silent.
