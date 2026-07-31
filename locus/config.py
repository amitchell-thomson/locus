"""Load and validate config.toml into typed, validated settings.

All tunables live in config.toml (never scattered constants). Secrets never live here:
the Claude API key is read from the ANTHROPIC_API_KEY env var, on demand, at generation time.
"""

from __future__ import annotations

import os
import tomllib
from functools import lru_cache
from pathlib import Path

from pydantic import BaseModel, Field

# Project root = the directory containing this package's parent (where config.toml lives).
PROJECT_ROOT = Path(__file__).resolve().parent.parent


def _load_dotenv() -> None:
    """Populate os.environ from a project-root .env (KEY=VALUE lines), if present.

    Real environment variables take precedence (we never overwrite an already-set key), so
    the .env is a convenience for local secrets like ANTHROPIC_API_KEY, not an override.
    """
    env_path = PROJECT_ROOT / ".env"
    if not env_path.exists():
        return
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        os.environ.setdefault(key.strip(), value.strip().strip("'\""))


_load_dotenv()


class OllamaConfig(BaseModel):
    host: str
    embed_model: str
    ingest_model: str
    # Ollama defaults to 4096. The ingest passes (summary/propositions/entities) feed full
    # section text, and num_ctx bounds prompt + generated output together: proposition-dense
    # sections cross 4096 once output is counted, sliding the section text out of the window
    # mid-generation. 8192 clears every measured section with margin (~0.47 GB extra VRAM).
    num_ctx: int = Field(8192, description="Ollama context window for ingest passes.")


class PathsConfig(BaseModel):
    db: Path
    raw_store: Path
    incoming: Path
    notes: Path


class EmbedConfig(BaseModel):
    # 768 is locked to nomic-embed-text. Changing it invalidates every stored vector.
    dim: int = Field(768, description="Embedding dimensionality; must match nomic-embed-text.")
    chunk_tokens: int = 512


class RetrieveConfig(BaseModel):
    proposition_top_k: int = 10
    fine_top_k: int = 20
    section_top_k: int = 5
    # Figure descriptions are a 4th dense arm (figure_vectors, plan step 11) — sized between
    # sections (5) and propositions (10): figures are sparse but high-signal when they match.
    figure_top_k: int = 8
    rerank_top_k: int = 8
    # Diversity cap on rerank survivors: at most this many units per document in the top-k,
    # relaxed (fallback fill) only when honouring it would leave the top-k underfull. Stops a
    # single best-matching document from monopolising every slot, which breaks cross-domain
    # synthesis queries (the 2026-06-04 evaluation, build step 3).
    per_doc_cap: int = 3
    context_token_budget: int = 100_000
    # Confidence floor on cross-encoder (ms-marco-MiniLM) scores — raw logits, roughly -11..+11.
    # None disables it (ship default until calibrated against the live corpus + negative-control
    # queries; the 2026-06-05 evaluation). When set: the rerank refill never pads the
    # top-k with below-floor candidates, and a retrieval whose best survivor falls below the
    # floor is flagged low-confidence ("the corpus may not cover this") — flagged, not filtered.
    min_rerank_score: float | None = None
    # Documents (by exact source_uri) dropped from the candidate pool in production retrieval.
    # The locus repo self-ingests, so 1000+ chunks of its own RAG/test/README code compete
    # with and contaminate real queries (a finance query surfacing locus test files —
    # 2026-06-09 audit). Excluded by default; `locus retrieve --include-excluded` overrides.
    exclude_source_uris: list[str] = Field(default_factory=list)
    # Multi-query cross-domain expansion (H2 fix, §13). A query in one domain's vocabulary
    # doesn't surface another's docs — the cross-encoder demotes the semantically-distant
    # match below the floor even though the entity bridge fired. When a query is bridge-shaped
    # OR the first pass is low-confidence, rephrase it into `multi_query_k` other disciplinary
    # vocabularies (local qwen), retrieve each, and rerank every candidate against the variant
    # that found it (max) so the cross-domain match clears the floor. Off => single-pass only.
    multi_query_expansion: bool = Field(True, description="Enable multi-query cross-domain expansion.")
    multi_query_k: int = Field(2, description="Reformulations beyond the original (0 = off).")
    # Down-weight for `maturity=rough` capture notes (agent-layer §6.1). Subtracted from the
    # cross-encoder rerank_score of rough-doc candidates before the final sort. It is a PENALTY
    # in cross-encoder logit space, NOT the plan's "multiplier on the pre-rerank score": the
    # arms' pre-rerank scores are explicitly not cross-arm comparable and do not order results
    # (search.py) — only the rerank_score does — and a multiplier on signed logits is incoherent
    # (it would PROMOTE a negatively-scored unit). A subtractive penalty is a well-defined,
    # monotonic demotion that still lets a strongly-matching rough note surface (flag, never
    # filter — principle 8). Inert until rough notes exist (Phase 1 Loop A); eval-tune then (§18).
    #
    # EVAL-TUNED 2026-07-29 (that §18 tune), 1.5 -> 0.0 (OFF) — swept over 14 queries x 4 values
    # (scripts/analysis/rough_penalty_sweep.py) once Loop A had landed 12 real rough notes:
    #
    #   penalty | note queries recovered | non-note queries still correct
    #      1.5  |          2/6           |            8/8
    #     0.75  |          2/6           |            8/8
    #     0.35  |          2/6           |            8/8
    #      0.0  |          3/6           |            8/8
    #
    # Monotonic, and the penalty bought NOTHING: intrusion was 8/8 at every value, so no non-note
    # target was ever displaced by a rough note. At 1.5 it cost real recall — the EM-rates note was
    # entirely unreachable, and the internship<->offer query surfaced neither side. The premise was
    # wrong, not just the magnitude: `rough` measures POLISH, and retrieval must rank on VALUE. A
    # jotted idea is often the higher-value unit precisely because it is the owner's own thinking
    # and exists in no other document, whereas a polished paper usually has near-duplicates in the
    # corpus. Cost of switching off, stated honestly: two coursework queries lose one rank position
    # each (rank 1 -> 2, a rough note above a still-surfacing target) — an MRR cost, not a recall one.
    # The knob is KEPT (not deleted) so the demotion is one config edit away if capture volume grows
    # enough to genuinely crowd results.
    rough_penalty: float = Field(
        0.0, description="Cross-encoder-score penalty applied to maturity=rough candidates (0 = off)."
    )
    # Per-CATEGORY cross-encoder penalty, same mechanism and same rationale as rough_penalty.
    # Measured 2026-07-29: engineering coursework is 246/295 documents, 81% of propositions and
    # 97% of figures, and 87% of cross-document canonical entities are reachable ONLY through it.
    # That mass crowds the owner's quant material out of results for queries that are not about
    # coursework. This DOWN-WEIGHTS, never filters (principle 8) — a coursework document that
    # genuinely answers a coursework question still wins by out-scoring the penalty, which is why
    # it is preferred over deleting documents: nothing is lost and the knob is reversible.
    # Empty = no category is penalised (previous behaviour).
    category_penalty: dict[str, float] = Field(
        default_factory=dict,
        description="category -> cross-encoder-score penalty for its candidates (empty = off).",
    )


class GenerationConfig(BaseModel):
    model: str


class MathOCRConfig(BaseModel):
    """Math-aware page OCR (extract/mathocr.py): recovers LaTeX from pages whose text layer
    is damaged or math-dense. Engine chosen by benchmark (eval-artifacts/mathocr/report.md)."""

    engine: str = Field("got", description="'got' (transformers) | 'qwen' (Ollama VLM) | 'off'")
    model: str = Field("qwen2.5vl:7b", description="Ollama model for the 'qwen' engine.")


class FiguresConfig(BaseModel):
    """Figure extraction + VLM description (plan step 11, §15.1).

    Tier 1 (preserve): detect figure regions in PDFs (raster images + vector diagrams) and
    visual-bearing slides, render to PNG in the raw store with any paired caption.
    Tier 2 (make findable): describe each figure with a local VLM via Ollama and embed
    caption+description into figure_vectors. Detection thresholds lean STRICT: a missed
    figure is benign, a junk figure burns a VLM call and a retrieval slot (the same risk
    asymmetry as `_plausible_heading`).
    """

    enabled: bool = Field(True, description="Run figure extraction + description at ingest.")
    model: str = Field("qwen2.5vl:7b", description="Ollama VLM for figure descriptions.")
    # Engine for the description pass. "ollama" = the default Ollama path (its qwen2.5vl
    # vision encoder runs on CPU, ~25 s/figure); "llamacpp" = an on-demand llama-server
    # with the mmproj on GPU (~3-5 s/figure) — needs the llama.cpp binary installed
    # (optional system dep, like soffice); absent/failing it falls closed to "ollama".
    engine: str = Field("ollama", description="'ollama' | 'llamacpp' (GPU vision encode).")
    llamacpp_binary: str = Field(
        "llama-server", description="llama-server binary (PATH name or absolute path)."
    )
    llamacpp_model: str = Field(
        "ggml-org/Qwen2.5-VL-7B-Instruct-GGUF:Q4_K_M",
        description="-hf spec (repo:quant) or a local .gguf path.",
    )
    llamacpp_mmproj: str | None = Field(
        None, description="Explicit mmproj .gguf path; None = -hf auto-pair."
    )
    llamacpp_host: str = Field("127.0.0.1", description="llama-server bind host.")
    llamacpp_port: int = Field(8090, description="llama-server bind port.")
    llamacpp_ngl: int = Field(99, description="LLM layers to offload to GPU.")
    llamacpp_ctx: int = Field(4096, description="Context size (4096 fits the 8 GB card).")
    llamacpp_startup_timeout: float = Field(
        300.0, description="Seconds to wait for /health (first spawn downloads ~5.5 GB)."
    )
    render_slides: bool = Field(
        True, description="Render visual-bearing slides via LibreOffice (soffice) to PNG."
    )
    # Area band as a fraction of page area: below = decoration/icons, above = page background.
    min_area_frac: float = Field(0.03, description="Min figure area as a fraction of page area.")
    max_area_frac: float = Field(0.92, description="Max figure area as a fraction of page area.")
    # A real diagram has many strokes; a stray underline or rule does not.
    min_vector_paths: int = Field(8, description="Min drawing paths for a vector cluster to count.")
    # Text-layer chars per 1000pt² inside the region. Real diagrams carry sparse labels
    # (corpus-measured max 1.6); prose blocks with formula/decoration drawings and gridline
    # tables read 2.2+ — their content is already in the text layer, so they are junk here.
    max_text_density: float = Field(
        2.0, description="Max text-layer char density (chars/1000pt²) for a figure region."
    )
    caption_max_gap_pt: float = Field(
        60.0, description="Max vertical gap (pt) between a figure and its caption block."
    )
    max_per_page: int = Field(4, description="Cap on detected figures per page (junk guard).")
    # Bounds image tokens/cost at generation: top-N figure survivors attach as images.
    image_cap: int = Field(3, description="Max figure images attached per Claude call / MCP reply.")


class AliasConfig(BaseModel):
    """Cross-document entity-alias resolution (locus/link/aliases.py, plan step 12).

    Deterministic tiers (casefold/punct/acronym/plural) run first; remaining lookalike
    clusters — blocked by name-embedding cosine AND a token-overlap guard — are adjudicated
    by the Claude API (the judge.py pattern). Wrong merges corrupt the link substrate, so
    every threshold here leans conservative: a missed merge is fragmentation, a wrong merge
    is corruption.
    """

    # Cosine floor over nomic name embeddings for candidate edges. Below it, names are not
    # even considered lookalikes.
    block_threshold: float = Field(0.86, description="Cosine floor for candidate clustering.")
    # Jaccard overlap over >=4-char lowercase tokens. Backstops the embedder pulling
    # thematically-adjacent but DISTINCT names together ('Kalman filter' vs 'particle filter').
    min_token_overlap: float = Field(0.34, description="Token-Jaccard guard on candidate edges.")
    # Connected components larger than this skip the LLM and stay deterministic-only —
    # a giant component is almost always a blocking-threshold failure, not one real entity.
    max_cluster_size: int = Field(8, description="Max cluster size sent to the LLM (cost guard).")
    # Names shorter than this never merge into a different surface (homonym risk: 'var', 'P2').
    min_merge_len: int = Field(4, description="Min name length eligible for any merge.")
    use_llm: bool = Field(True, description="Adjudicate fuzzy clusters via the Claude API.")
    # Related-documents stop-entity guard (link/related.py): a canonical entity appearing in
    # more than this FRACTION of the corpus is too ubiquitous to indicate a real pairing
    # (every coursework doc shares 'ODE', 'Fourier') and is excluded from the related ranking.
    # Resolved to an absolute doc-frequency at query time (ratio x doc count). Auto-disabled
    # below a small-corpus floor; 0 disables entirely. Enabled post-pour (CLAUDE.md §9).
    stop_doc_freq_ratio: float = Field(
        0.4, description="Exclude related-doc entities appearing in >this fraction of the corpus (0 = off)."
    )
    # Minimum spacing between adjudication API calls. A full rebuild is hundreds of small
    # sequential calls (359 on the 33-doc corpus; more post-pour) — unthrottled, the burst
    # rides the account's per-minute rate limit and gets 429-throttled by the SDK anyway.
    # 1.5 s ≈ 40 requests/min, under the lowest tier's ceiling. 0 disables.
    api_call_interval: float = Field(
        1.5, description="Seconds between alias-adjudication API calls (0 = unthrottled)."
    )


class RetitleConfig(BaseModel):
    """Corpus-level document retitling (locus/retitle.py).

    Composes distinctive '[Module — ][Seq: ]Topic' titles after ingest batches, breaking
    same-title collisions across the corpus. The topic is distilled from the stored synthesis
    by the Claude API (judgement-quality, §11.B), cached by content hash; use_llm False falls
    back to a deterministic thesis-clause topic. Manual-only, like [alias] — billed, global
    view, run after the pour.
    """

    use_llm: bool = Field(True, description="Distil topics via the Claude API (else thesis-clause).")
    api_call_interval: float = Field(
        1.5, description="Seconds between topic-distillation API calls (0 = unthrottled)."
    )


class ReposConfig(BaseModel):
    """Tracked code repositories (build step 10). `locus watch` checks each repo's git
    HEAD every `check_interval` seconds and re-ingests only when new commits landed —
    the check itself is ~free (git rev-parse). Manual runs: `locus sync [--force]`."""

    paths: list[str] = Field(default_factory=list, description="Absolute repo paths to track.")
    check_interval: float = Field(3600.0, description="Seconds between HEAD checks in `locus watch`.")
    # Repo-relative fnmatch globs excluded from ingest. Exists for self-ingestion
    # contamination (round-5 audit): the locus repo indexes itself, and files carrying
    # the labelled eval queries verbatim ranked #1 on those very queries, displacing
    # real content. Exclude the answer keys, not the codebase.
    exclude_globs: list[str] = Field(
        default_factory=list,
        description="Repo-relative fnmatch globs to skip at repo ingest.",
    )


class ConceptsConfig(BaseModel):
    """Code concept-entity pass (ingest/concepts.py, CLAUDE.md §1.2 — Link).

    Extracts the domain concepts a repository implements/studies from its NARRATIVE (synthesis
    + README + file summaries, never raw code) into `concept`/`method`/… entities, so code
    projects link to the papers they're built on. A doc-level local-qwen pass run for
    `source_type='code'`; the durable merge decision still goes through `locus link`."""

    enabled: bool = Field(True, description="Run the code concept-entity pass at ingest.")
    # Bound on concepts written per repo — a runaway README is the failure mode (a junk concept
    # is a false cross-domain link, so cap + the grounding/stoplist guards lean conservative).
    max_per_doc: int = Field(40, description="Max concepts written per repository.")
    # Generic terms never written as concepts (they would link everything to everything).
    # Empty list => the built-in ingest/concepts.DEFAULT_STOPLIST.
    stoplist: list[str] = Field(default_factory=list, description="Override the generic-term stoplist (empty = built-in).")


class ObsidianConfig(BaseModel):
    """Read-only Obsidian projection of the corpus (locus/export/obsidian.py, §13).

    One-way, regenerable render of the SQLite corpus as a browsable vault — never read back,
    never in the retrieval path. Joins-only (no LLM), canonical entities only (joins through
    entity_aliases). The exporter owns only its docs/ and entities/ subtrees and never touches
    `.obsidian/` (the user's per-machine layout/plugin config). See the plan in
    docs/obsidian-projection-plan.md.
    """

    out_dir: Path = Field(Path("vault/obsidian"), description="Exporter-owned vault root.")
    # 10 (not 5): code projects share rarer concepts with each other than with papers, so the
    # papers a project is built on land at rank ~6-15 (the generic shared concept, e.g. 'Markov
    # model', is low-IDF). A top-10 list surfaces them without the cost of Phase-2 fuzzy linking.
    top_related: int = Field(10, description="doc<->doc related edges per note.")
    # A canonical entity becomes a note only if it spans >= this many EXPORTED docs. A
    # single-doc canonical is noise (no cross-doc link to draw), mirroring related.py.
    min_cluster_docs: int = Field(2, description="Emit a canonical entity note only if it spans >= this many docs.")
    emit_entity_notes: bool = Field(True, description="Emit canonical entity notes (false = docs-only graph).")


class ReadingConfig(BaseModel):
    """Server->device reading delivery (`locus read`, agent-layer plan §8.5, Phase 0.5).

    Renders a markdown doc to a device-tuned PDF and `rmapi put`s it to the tablet. Geometry
    defaults target the reMarkable Paper Pro screen (2160x1620 @229dpi => 7.07x9.43in) so text
    renders at its true point size; a different device is a config change, not a code change.
    """

    rmapi_binary: str = Field("rmapi", description="rmapi binary (PATH name or absolute path).")
    target_folder: str = Field("Locus", description="Device folder that delivered docs land in.")
    page_width_in: float = Field(7.07, description="PDF page width (in) — Paper Pro screen width.")
    page_height_in: float = Field(9.43, description="PDF page height (in) — Paper Pro screen height.")
    margin_in: float = Field(0.5, description="Page margin (in).")
    font_pt: float = Field(11.0, description="Body text size (pt).")


class DiscoveryConfig(BaseModel):
    """Proposed reading and the accept loop (docs/reading-discovery-plan.md, Phase 4).

    `Locus/Reading/{Proposed,In-Progress,Finished}` on the device: moving a file OUT of `Proposed`
    is the accept signal and triggers ingest; leaving it there is a rejection.

    The caps are on the STOCK sitting in `Proposed`, not on a rate. A weekly quota keeps topping up
    a folder he has not cleared, which is how a reading list turns into a guilt metric; a stock cap
    means an untouched folder proposes nothing, and empty stays a valid state.
    """

    root_folder: str = Field(
        "Locus/Reading", description="Device folder holding the three reading folders."
    )
    rmapi_binary: str = Field("rmapi", description="rmapi binary (PATH name or absolute path).")
    paper_cap: int = Field(
        10,
        description="Max papers held in Proposed at once (a stock, not a rate). 3 -> 10 on "
                    "2026-07-31: a proposal is a menu, not a demand, and rejecting one is the "
                    "signal the flywheel tunes on — three at a time starved it.",
    )
    book_cap: int = Field(
        1, description="Max books held in Proposed at once — one considered suggestion, not a feed."
    )
    ttl_days: int = Field(
        21,
        description="Days in Proposed before it reads as a no. A WEAK negative: silence is as "
                    "likely to mean a busy fortnight as a bad suggestion.",
    )
    drop_folder: str = Field(
        "paper", description="Folder under vault/incoming/ that accepted readings are ingested via."
    )

    # --- the discovery engine (step 2) ---
    # ONLY these category tokens ever leave the machine. Not concepts, not project names — the
    # relevance judgement happens locally against his own embeddings (locus/discover/arxiv.py).
    arxiv_categories: list[str] = Field(
        default_factory=lambda: [
            "q-fin.PM", "q-fin.ST", "q-fin.TR", "q-fin.RM", "q-fin.CP",
            "econ.EM", "stat.ML", "stat.AP", "eess.SY", "cs.RO",
        ],
        description="arXiv categories to harvest. Category tokens only — validated on send.",
    )
    per_category: int = Field(
        25,
        description="Papers pulled from EACH category. A quota, not a shared pool: q-fin is "
                    "outpublished ~10x by stat.ML/cs.LG, so one OR-query returned 2 q-fin.PM "
                    "papers out of 200 (measured 2026-07-31).",
    )
    harvest_limit: int = Field(400, description="Overall cap across all categories.")
    familiarity_weight: float = Field(
        0.25,
        description="Penalty on a candidate resembling material he already has. SWEPT 2026-07-31, "
                    "1.0 -> 0.25: at 1.0 it pushed the harvest's two best portfolio papers out of "
                    "the top 10 in favour of building-energy M&V. A tiebreaker, not a driver, "
                    "until the corpus is dense enough for 'already have this' to be often true.",
    )
    gap_weight: float = Field(
        0.6, description="Weight of a gap match relative to a project match (projects win)."
    )

    @property
    def caps(self) -> dict[str, int]:
        return {"paper": self.paper_cap, "book": self.book_cap}


class AgentConfig(BaseModel):
    """Agent-layer orchestration (agent-layer plan §10, Phase 1). The capture/enrichment loops
    run language tasks through headless `claude -p` (the owner's subscription), metered by a
    cost ledger (agent/budget.py). Bulk work uses API Batch instead (added with hybrid routing)."""

    # `claude -p --model` for durable agent passes. Haiku is the Phase-0 routing choice (it beat
    # local AND Sonnet on the durable passes at 3x less cost); a CLI alias so it tracks the
    # current Haiku without pinning a dated id. Judgement-heavy passes may override per-call.
    model: str = Field("haiku", description="claude -p model alias/id for agent language tasks.")
    # Soft daily spend ceiling (USD) for agent-layer `claude -p`, summed from agent_runs across
    # the day (agent/budget.spent_today). None disables the cap. The foreground-yield guard is
    # not yet built (needs the live-throttle error shape, Phase-0 finding d); this cost cap is
    # the working guard until then.
    daily_cost_cap_usd: float | None = Field(
        5.0, description="Daily agent-layer claude -p spend cap in USD (None = no cap)."
    )


class StructureConfig(BaseModel):
    """Object/belief proposal (locus/structure/propose.py, agent-layer plan §6.2-6.3, Phase 2).

    Every default here is a PRECISION knob, and precision is the whole game: an agent that
    proposes six plausible objects per note trains the owner to ignore it (failure mode #7), and
    a belief trajectory containing one stance he did not actually take is worse than no
    trajectory. Loosen deliberately, on evidence from a `--dry-run`."""

    # Hard cap on objects proposed per document. The model is asked for its most consequential
    # first, so the cap truncates the tail rather than sampling.
    max_objects_per_doc: int = Field(3, description="Max objects proposed per document.")
    # A concept must already span this many documents to be proposable. >=2 is the substantive
    # bar: a "concept" appearing in exactly one document is that document's vocabulary, not a
    # thread through the corpus, and the Link use case (§1.2) is about threads.
    min_concept_docs: int = Field(2, description="Min documents a canonical concept must span.")
    # Extra cross-document `relates` links per object, taken only from retrieval citations that
    # clear [retrieve].min_rerank_score.
    max_support_links: int = Field(3, description="Max floor-clearing support links per object.")
    # Entity TYPES that cannot become a Concept object. A Concept is an IDEA the owner engages
    # with; a language, a platform, a person and a company are none of those, however often they
    # co-occur with his work. The first live run proposed `Python` (tool) and `Optibook` (tool)
    # as concepts to track — both cleared every other gate, because the problem is not their
    # frequency but their kind. Types come from the ingest entity taxonomy (§6).
    concept_exclude_entity_types: list[str] = Field(
        default_factory=lambda: ["tool", "author", "organization", "ticker", "other"],
        description="Entity types that may not become Concept objects (a concept is an idea).",
    )
    # Categories whose documents may yield BELIEF POSITIONS. Default `note` only: handwriting and
    # captured conversations are the owner's own words. A paper's claim is the paper's position;
    # attributing it to him corrupts the headline capability (§3.4).
    belief_source_categories: list[str] = Field(
        default_factory=lambda: ["note"],
        description="Categories whose docs may yield belief positions (owner-authored only).",
    )
    # A Concept object must touch at least one document in these categories. Measured 2026-07-29:
    # 87% of cross-document canonical entities are reachable ONLY through engineering coursework,
    # so without this the concept space is overwhelmingly Ampere's-law-linking-two-lecture-notes
    # rather than the owner's quant work. The 89 concepts that BRIDGE coursework into papers/
    # projects/career/notes (LTI system, transfer function, Markov model, regime shift detection)
    # are kept by construction — they touch a listed category. Empty = no requirement.
    concept_require_categories: list[str] = Field(
        default_factory=lambda: ["paper", "project", "career", "note"],
        description="A concept must appear in >=1 doc of these categories (empty = no requirement).",
    )
    # Override the agent model for this pass (None = [agent].model, Haiku).
    model: str | None = Field(None, description="claude -p model for the proposal pass.")


class IngestConfig(BaseModel):
    """Per-pass ingest LLM routing (agent-layer plan §7). Maps a durable text pass to an engine:
    `local` = the local qwen model (Ollama, the default) · any other value = a Claude model
    alias/id run via the METERED Anthropic SDK (cheap Haiku — the right channel for high-volume
    small passes, not `claude -p` whose per-call harness prompt dominates small-task cost, Phase-0).

    Embeddings and math-OCR are ALWAYS local (no Claude equivalent / safe-degrade) and are not
    routed here. Absent/empty `pass_routing` => every pass runs local (current behaviour, no
    change). Pass names: summarize · propositions · entities · synthesis · gaps · concepts."""

    pass_routing: dict[str, str] = Field(
        default_factory=dict,
        description="ingest pass -> 'local' | claude model alias/id (haiku/sonnet). Empty = all local.",
    )


class CaptureConfig(BaseModel):
    """Loop A — reMarkable handwriting capture (agent-layer plan §8.1, Phase 1 ④).

    The device pushes each changed notebook to `staging_dir` as `<uuid>.pdf`; capture identifies,
    transcribes, enriches, and files it as a `rough` note. Folder→category maps a reMarkable top
    folder to a Locus category (empty dict = the built-in map in capture/remarkable.py)."""

    staging_dir: Path = Field(
        Path("/home/alec/remarkable-import"),
        description="Where the device-push receiver lands <uuid>.pdf renders (absolute).",
    )
    rmapi_binary: str = Field("rmapi", description="rmapi binary (PATH name or absolute path).")
    # Transcription is a VISION call — an Anthropic SDK request with the page image, NOT `claude -p`
    # (Phase-0: claude -p spun an 8-turn loop at $0.23/page; the SDK image call is cents/page). It is
    # METERED (needs ANTHROPIC_API_KEY). Default SONNET, not Haiku: a 2026-07-28 A/B on a real dense
    # notebook (Jargon Sheet) showed Haiku produced confident NONSENSE on hard quant-jargon pages
    # while Sonnet transcribed coherently and flagged uncertainty — and transcription seeds the
    # corpus (errors propagate, failure mode #3). ~$0.015/page; the text passes (fill-in/enrich) stay
    # on cheap Haiku (agent.model). This is the plan's "escalate to Sonnet on eval evidence".
    transcribe_model: str = Field(
        "claude-sonnet-5", description="Anthropic vision model id for transcription (seeds the corpus)."
    )
    transcribe_dpi: int = Field(150, description="Render DPI for handwriting pages (legible, not huge).")
    folder_category: dict[str, str] = Field(
        default_factory=dict, description="reMarkable folder -> Locus category (empty = built-in)."
    )
    excluded_folders: list[str] = Field(
        default_factory=lambda: ["trash", "Locus", "admin"],
        description="reMarkable folders whose docs are not Loop-A capture.",
    )
    default_category: str = Field("note", description="Category for folders not in the map.")


class MCPConfig(BaseModel):
    # The MCP `query` tool makes a billed Claude API call server-side; `retrieve` and the read
    # tools are local-only (free). Default OFF so the server never exposes a billable tool unless
    # the owner opts in — the client model cannot trigger spend on a tool that isn't advertised.
    enable_query: bool = Field(
        False, description="Expose the server-side-generating `query` MCP tool (costs API spend)."
    )
    include_figure_images: bool = Field(
        True, description="Attach retrieved figure images as MCP image content blocks."
    )


class Config(BaseModel):
    ollama: OllamaConfig
    paths: PathsConfig
    embed: EmbedConfig
    retrieve: RetrieveConfig
    generation: GenerationConfig
    # Optional: absent [mcp] in config.toml falls back to defaults (query disabled).
    mcp: MCPConfig = Field(default_factory=MCPConfig)
    # Optional: absent [mathocr] falls back to defaults (qwen engine via Ollama).
    mathocr: MathOCRConfig = Field(default_factory=MathOCRConfig)
    # Optional: absent [figures] falls back to defaults (enabled, qwen2.5vl via Ollama).
    figures: FiguresConfig = Field(default_factory=FiguresConfig)
    # Optional: absent [repos] means no tracked repos (sync is a no-op).
    repos: ReposConfig = Field(default_factory=ReposConfig)
    # Optional: absent [alias] falls back to defaults (LLM adjudication on).
    alias: AliasConfig = Field(default_factory=AliasConfig)
    # Optional: absent [retitle] falls back to defaults (LLM topic distillation on).
    retitle: RetitleConfig = Field(default_factory=RetitleConfig)
    # Optional: absent [concepts] falls back to defaults (code concept pass on).
    concepts: ConceptsConfig = Field(default_factory=ConceptsConfig)
    # Optional: absent [obsidian] falls back to defaults (vault/obsidian, entity notes on).
    obsidian: ObsidianConfig = Field(default_factory=ObsidianConfig)
    # Optional: absent [reading] falls back to defaults (Paper Pro geometry, folder "Locus").
    reading: ReadingConfig = Field(default_factory=ReadingConfig)
    # Optional: absent [discovery] falls back to defaults (Locus/Reading, 3 papers / 1 book, 21d).
    discovery: DiscoveryConfig = Field(default_factory=DiscoveryConfig)
    # Optional: absent [agent] falls back to defaults (claude -p model 'haiku', $5/day cap).
    agent: AgentConfig = Field(default_factory=AgentConfig)
    # Optional: absent [capture] falls back to defaults (staging dir, built-in folder→category).
    capture: CaptureConfig = Field(default_factory=CaptureConfig)
    # Optional: absent [ingest] falls back to defaults (pass_routing empty = every pass local).
    ingest: IngestConfig = Field(default_factory=IngestConfig)
    # Optional: absent [structure] falls back to the conservative proposal precision bar.
    structure: StructureConfig = Field(default_factory=StructureConfig)

    def resolve_paths(self) -> "Config":
        """Make all configured paths absolute, relative to the project root."""
        base = PROJECT_ROOT
        self.paths.db = (base / self.paths.db).resolve()
        self.paths.raw_store = (base / self.paths.raw_store).resolve()
        self.paths.incoming = (base / self.paths.incoming).resolve()
        self.paths.notes = (base / self.paths.notes).resolve()
        self.obsidian.out_dir = (base / self.obsidian.out_dir).resolve()
        return self

    @staticmethod
    def anthropic_api_key() -> str:
        """Read the Claude API key from the environment. Raises if unset.

        Called only when a generation request is about to be made, so the rest of the
        pipeline (ingest, retrieval) works without a key present.
        """
        key = os.environ.get("ANTHROPIC_API_KEY")
        if not key:
            raise RuntimeError(
                "ANTHROPIC_API_KEY is not set. Export it before running a query: "
                "export ANTHROPIC_API_KEY=sk-..."
            )
        return key


@lru_cache(maxsize=1)
def load(config_path: Path | None = None) -> Config:
    """Load, validate, and path-resolve config.toml. Cached for the process lifetime."""
    path = config_path or (PROJECT_ROOT / "config.toml")
    if not path.exists():
        raise FileNotFoundError(
            f"config.toml not found at {path}. Copy config.example.toml to config.toml."
        )
    with path.open("rb") as f:
        data = tomllib.load(f)
    return Config.model_validate(data).resolve_paths()
