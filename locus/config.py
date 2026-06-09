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
