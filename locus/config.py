"""Load and validate config.toml into typed, validated settings.

All tunables live in config.toml (never scattered constants). Secrets never live here:
the Claude API key is read from the ANTHROPIC_API_KEY env var, on demand, at generation time.
"""

from __future__ import annotations

import os
import tomllib
from functools import lru_cache
from pathlib import Path

from pydantic import BaseModel, Field, field_validator

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
    # Confidence floor on cross-encoder (ms-marco-MiniLM) scores — raw logits, roughly -11..+11.
    # None disables it (ship default until calibrated against the live corpus + negative-control
    # queries; the 2026-06-05 evaluation). When set: the rerank refill never pads the
    # top-k with below-floor candidates, and a retrieval whose best survivor falls below the
    # floor is flagged low-confidence ("the corpus may not cover this") — flagged, not filtered.
    min_rerank_score: float | None = None


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


class ReadingConfig(BaseModel):
    """Server->device reading delivery (`locus read`, agent-layer plan §8.5, Phase 0.5).

    Renders a markdown doc to a device-tuned PDF and `rmapi put`s it to the tablet. Geometry
    defaults target the reMarkable Paper Pro screen (2160x1620 @229dpi => 7.07x9.43in) so text
    renders at its true point size; a different device is a config change, not a code change.
    """

    rmapi_binary: str = Field("rmapi", description="rmapi binary (PATH name or absolute path).")
    # `Daily`, not `Locus`, since the 2026-08 device reorganisation: the folder is named for the
    # ACTIVITY it holds rather than for the system that writes it, and it doubles as the unread
    # inbox (a page stays loose here until it has ink on it, then archives to Daily/YYYY-MM).
    target_folder: str = Field("Daily", description="Device folder that delivered docs land in.")
    # Ad-hoc markdown sent on a human ask (the MCP `to_remarkable` tool) gets its OWN device
    # folder, because every other candidate is already load-bearing: /Daily is the daily page's
    # ink inbox (a loose doc there reads as an unread page); Reading/{In-Progress,Finished} is
    # what loop_b reads as "he chose this" and auto-ingests from the bundle's bytes; and /Notes
    # is `capture.notes_root`, whose contents Loop A ingests as HIS handwriting — agent prose
    # landing there is invariant 5 violated by filing. See reading/send.py.
    send_folder: str = Field(
        "Inbox", description="Device folder for ad-hoc markdown sends (MCP `to_remarkable`)."
    )
    page_width_in: float = Field(7.07, description="PDF page width (in) — Paper Pro screen width.")
    page_height_in: float = Field(9.43, description="PDF page height (in) — Paper Pro screen height.")
    margin_in: float = Field(0.5, description="Page margin (in).")
    font_pt: float = Field(11.0, description="Body text size (pt).")
    # A document Claude WRITES for the tablet is authored in LaTeX (reading/tex2pdf), which needs
    # a system binary — unlike the markdown path, whose pandoc+typst toolchain installs from the
    # lockfile. `tectonic` is preferred because it fetches the packages a document asks for
    # instead of requiring a provisioned TeX distribution. An engine named here that is not on
    # PATH does not disable the feature: `available_engine` falls through to whatever IS present
    # and only errors when nothing is, since a stale config value should not be a hard failure.
    latex_engine: str = Field(
        "tectonic", description="LaTeX engine for agent-authored sends ('tectonic' | 'pdflatex')."
    )
    # MEASURED, and the reason this is configurable rather than the 180s constant it replaces:
    # the engine invocation pays TWO unrelated costs and only one of them is the document. A warm
    # compile of a four-figure TikZ/pgfplots brief takes ~1.3s (2026-09-08). The FIRST compile on
    # a machine pays a one-off package fetch instead — tectonic downloads the pgf/tikz tree from
    # its bundle server, and `_DIAGRAM_SETUP` loads tikz and pgfplots for EVERY document, so the
    # fetch is charged to a one-line fragment exactly as it is to a diagram-heavy one. MEASURED:
    # two cold runs took 201.5s and 149.7s, STRADDLING the old hardcoded 180s — and the slower of
    # the two was the one-line fragment, so the document is not the variable. That is why this
    # presented as an intermittent "the server is down" and why a full brief and the minimal
    # fragment sent to isolate it failed identically. The failure was never in the LaTeX.
    #
    # This is a CEILING on a pathological compile (a runaway `\loop`), not a budget for a normal
    # one. Setting it near the warm time would make a first send on a new machine, or after
    # `rm -rf ~/.cache/tectonic`, fail for a reason no error message could make actionable.
    latex_timeout_s: float = Field(
        900.0,
        gt=0,
        description="Ceiling on one LaTeX engine invocation (s). Sized for the one-off "
        "cold-cache package fetch, not the warm compile.",
    )
    # The LaTeX path carries its OWN body size and margin and deliberately does not inherit
    # `font_pt`/`margin_in`. Agent-authored documents are set one column, 10pt, with 0.95in
    # margins all round (Slice 0.4, 2026-10-19: the geometry of the redesign plan documents),
    # replacing two columns at 9pt. `font_pt` and `margin_in` still size the markdown path, and
    # through the shared geometry the daily page — changing one must not silently move the other.
    #
    # MEASURED, and the reason this is a CONSTRAINED set rather than a free float: `article`
    # accepts only 10/11/12pt as a class option and silently typesets anything else at 10pt.
    # `\documentclass[9.5pt]{article}` compiles clean and renders at 10pt, announcing itself
    # only as an "Unused global option" line buried in the log — the exact silent-substitution
    # class this codebase exists to resist. The preamble therefore builds on `extarticle`
    # (extsizes), whose supported sizes are these, and a value outside them fails HERE at config
    # load rather than becoming a document typeset at a size nobody chose.
    latex_font_pt: float = Field(
        10.0, description="Body text size (pt) for agent-authored LaTeX sends (extsizes set)."
    )
    latex_margin_in: float = Field(
        0.95, description="Page margin (in), all four sides, for agent-authored LaTeX sends."
    )

    @field_validator("latex_font_pt")
    @classmethod
    def _latex_font_pt_is_a_real_class_size(cls, value: float) -> float:
        # Imported here rather than at module scope: the set is a fact about LaTeX document
        # classes and belongs beside the preamble that relies on it, and `config` is imported by
        # nearly everything — it should not pull in a renderer to load a TOML file.
        from locus.reading.tex2pdf import CLASS_SIZES_PT

        if value not in CLASS_SIZES_PT:
            allowed = ", ".join(f"{s:g}" for s in sorted(CLASS_SIZES_PT))
            raise ValueError(
                f"[reading].latex_font_pt must be one of {allowed} (the sizes the extsizes "
                f"document class actually implements) — got {value:g}, which LaTeX would "
                "silently round to a different size."
            )
        return value


class AgentConfig(BaseModel):
    """Agent-layer orchestration. WHERE MODEL CALLS RUN (agent/claude.py, 3 Nov): on the API
    credit that comes with Max ($100 a month, Messages and Batch API, expires each cycle), and
    on the subscription through headless `claude -p` only when that credit refuses a call. His
    call, 3 Nov: "it should be on the $100 credit for now, with the ability to switch if that
    credit goes". Every call is counted (agent/usage.py, `locus budget`)."""

    backend: str = Field(
        "api", description="'api' (the Max API credit, falling back to the subscription when it "
        "refuses) or 'subscription' (headless claude -p only).",
    )
    # A model named by alias here and in the passes ("haiku") is pinned to an id for the API,
    # which has no aliases. These are the ids the code already used on the API before Slice 0.13.
    api_models: dict[str, str] = Field(
        default_factory=lambda: {
            "haiku": "claude-haiku-4-5-20251001",
            "sonnet": "claude-sonnet-5",
            "opus": "claude-opus-5-5",
            # heuris H2 (D8): facts, records and briefs on the current generation.
            "sonnet-5.5": "claude-sonnet-5-5",
            "haiku-5.5": "claude-haiku-5-5",
        },
        description="model alias -> API model id",
    )
    # USD per million tokens [input, output], used only to ESTIMATE the month's credit use for
    # `locus budget`; the console is the truth. Haiku 4.5 was confirmed in Phase 0; the others are
    # estimates to be replaced by the first month's measurement.
    api_prices: dict[str, list[float]] = Field(
        default_factory=lambda: {
            "claude-haiku-4-5-20251001": [1.0, 5.0],
            "claude-sonnet-5": [3.0, 15.0],
            "claude-opus-5-5": [5.0, 25.0],
            "claude-sonnet-5-5": [2.0, 10.0],
            "claude-haiku-5-5": [0.10, 0.50],
        },
        description="API model id -> [input, output] USD per million tokens (estimates)",
    )
    api_retry_hours: float = Field(
        24.0, description="After the credit refuses, hours on the subscription before trying it again."
    )
    credit_cycle_day: int = Field(
        1, description="Day of the month the Max API credit renews (for `locus budget`)."
    )

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
    """Which documents are HIS writing: read by `agent.state.owner_authored_sql`, which heuris
    search uses to tell his notes from sources. The proposal pass these were written for was
    deleted in Slice C (C2); the section name stays so the server's config.toml keeps meaning
    the same thing."""

    # Categories whose documents may yield BELIEF POSITIONS and IDEAS. A paper's claim is the
    # paper's position; attributing it to him corrupts the headline capability (§3.4).
    #
    # `project` and `career` were added on the owner's instruction (2026-08-03): "even if claude
    # authored them while I was writing the project they should still be treated as canonical
    # project documents, not agent generated lower weight material. The writeups are arguably the
    # most important documents in the project." A write-up records decisions HE made; who typed it
    # is not the question. Excluding them meant his OXDAQ architecture note and every future note
    # in `/Notes/projects` or `/Notes/careers` could never yield an idea — while `folder_category`
    # files his handwriting into exactly those categories.
    belief_source_categories: list[str] = Field(
        default_factory=lambda: ["note", "project", "career"],
        description="Categories whose docs may yield belief positions/ideas (owner-authored).",
    )
    # ...but a category alone cannot say whose words they are. `category='project'` also holds
    # third-party material he keeps for reference — live, the two Optibook vendor manuals — and a
    # belief extracted from a vendor PDF and attributed to him is precisely the §3.4 corruption.
    # The discriminator is the FORMAT: what he and his agent write is markdown/text/notebook;
    # a published manual or paper arrives as a PDF or a slide deck. Empty = no restriction.
    belief_source_types: list[str] = Field(
        default_factory=lambda: ["markdown", "text", "docx", "notebook"],
        description="Source types that count as his own writing (empty = any).",
    )


class IngestConfig(BaseModel):
    """Per-pass ingest LLM routing (agent-layer plan §7). Maps a durable text pass to an engine:
    `local` = the local qwen model (Ollama, the default) · any other value = a Claude model
    alias/id run on the SUBSCRIPTION through a lean headless `claude -p` (Slice 0.13). Phase 0
    sent these to the metered SDK because a `claude -p` call carried its harness's tool and server
    definitions, which dominated a small pass; the lean flags remove them (~23,500 -> ~750 tokens).

    Embeddings and math-OCR are ALWAYS local (no Claude equivalent / safe-degrade) and are not
    routed here. Absent/empty `pass_routing` => every pass runs local (current behaviour, no
    change). Pass names: summarize · propositions · entities · synthesis · gaps · concepts."""

    pass_routing: dict[str, str] = Field(
        default_factory=dict,
        description="ingest pass -> 'local' | claude model alias/id (haiku/sonnet). Empty = all local.",
    )


class CaptureConfig(BaseModel):
    """Capture from the reMarkable cloud (Slice 0.12; capture/device.py).

    What is read, and how, follows the folders and the kind of document (a notebook is his
    writing, a PDF Locus sent is Claude's, any other PDF in an area folder is a source), so there
    is no folder map or exclusion list to configure: an unknown folder is not read at all. The
    old `folder_category`, `excluded_folders`, `default_category` and `notes_root` settings were
    retired with the tablet agent; pydantic ignores unknown keys, so a config that still sets
    them would LOOK live, which is why they were also removed from the server's config.toml."""

    staging_dir: Path = Field(
        Path("/home/alec/remarkable-import"),
        description="Where notebook renders are written as <uuid>.pdf for transcription.",
    )
    rmapi_tree_cache: Path = Field(
        Path("~/.cache/rmapi/tree.cache"),
        description="rmapi's own tree cache, read after one `rmapi ls /` (capture/device.py).",
    )
    max_notebooks_per_run: int = Field(
        4, description="Notebooks rendered and transcribed per capture run (billed vision)."
    )
    annotate_max_fetch: int = Field(
        15, description="PDF bundles fetched per capture run; the rest wait a tick (cloud 429s)."
    )
    rmapi_binary: str = Field("rmapi", description="rmapi binary (PATH name or absolute path).")
    # Transcription is a VISION call, made on the subscription with the page image inside the message
    # (Slice 0.13; Phase 0's $0.23/page came from handing `claude -p` a FILE to open over eight tool
    # turns, which the inline image avoids). Default SONNET, not Haiku: a 2026-07-28 A/B on a real dense
    # notebook (Jargon Sheet) showed Haiku produced confident NONSENSE on hard quant-jargon pages
    # while Sonnet transcribed coherently and flagged uncertainty — and transcription seeds the
    # corpus (errors propagate, failure mode #3). Subscription quota, not money; the text passes stay
    # on cheap Haiku (agent.model). This is the plan's "escalate to Sonnet on eval evidence".
    transcribe_model: str = Field(
        "claude-sonnet-5", description="Anthropic vision model id for transcription (seeds the corpus)."
    )
    transcribe_dpi: int = Field(150, description="Render DPI for handwriting pages (legible, not huge).")
    # Loop B on the capture timer (capture/loop_b.py): reading annotations captured and their
    # handwriting transcribed automatically. Transcription was the ONE manual link in the
    # ink→answer chain — 105 marks sat wordless on a paper because `locus annotate --transcribe`
    # was never run for it (2026-08-06). The per-run bound caps billed vision spend; ink beyond
    # it is picked up next tick, because transcription skips marks that already carry a note.
    annotate_transcribe: bool = Field(
        True, description="Transcribe new reading-annotation handwriting on the capture timer."
    )
    annotate_max_transcribe: int = Field(
        8, description="Max billed vision transcriptions per capture run (rest wait a tick)."
    )


class MCPConfig(BaseModel):
    # The MCP `query` tool makes a billed Claude API call server-side; `retrieve` and the read
    # tools are local-only (free). Default OFF so the server never exposes a billable tool unless
    # the owner opts in — the client model cannot trigger spend on a tool that isn't advertised.
    enable_query: bool = Field(
        False, description="Expose the server-side-generating `query` MCP tool (costs API spend)."
    )


class Config(BaseModel):
    ollama: OllamaConfig
    paths: PathsConfig
    embed: EmbedConfig
    retrieve: RetrieveConfig
    # Optional: absent [mcp] in config.toml falls back to defaults (query disabled).
    mcp: MCPConfig = Field(default_factory=MCPConfig)
    # Optional: absent [mathocr] falls back to defaults (qwen engine via Ollama).
    mathocr: MathOCRConfig = Field(default_factory=MathOCRConfig)
    # Optional: absent [figures] falls back to defaults (enabled, qwen2.5vl via Ollama).
    figures: FiguresConfig = Field(default_factory=FiguresConfig)
    # Optional: absent [repos] means no tracked repos (sync is a no-op).
    repos: ReposConfig = Field(default_factory=ReposConfig)
    # Optional: absent [concepts] falls back to defaults (code concept pass on).
    concepts: ConceptsConfig = Field(default_factory=ConceptsConfig)
    # Optional: absent [reading] falls back to defaults (Paper Pro geometry, folder "Locus").
    reading: ReadingConfig = Field(default_factory=ReadingConfig)
    # Optional: absent [agent] falls back to defaults (claude -p model 'haiku', $5/day cap).
    agent: AgentConfig = Field(default_factory=AgentConfig)
    # Optional: absent [capture] falls back to defaults (staging dir, built-in folder→category).
    capture: CaptureConfig = Field(default_factory=CaptureConfig)
    # Optional: absent [ingest] falls back to defaults (pass_routing empty = every pass local).
    ingest: IngestConfig = Field(default_factory=IngestConfig)
    # Optional: absent [structure] falls back to the default definition of his writing.
    structure: StructureConfig = Field(default_factory=StructureConfig)

    def resolve_paths(self) -> "Config":
        """Make all configured paths absolute, relative to the project root."""
        base = PROJECT_ROOT
        self.paths.db = (base / self.paths.db).resolve()
        self.paths.raw_store = (base / self.paths.raw_store).resolve()
        self.paths.incoming = (base / self.paths.incoming).resolve()
        self.paths.notes = (base / self.paths.notes).resolve()
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
