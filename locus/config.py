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
    rerank_top_k: int = 8
    # Diversity cap on rerank survivors: at most this many units per document in the top-k,
    # relaxed (fallback fill) only when honouring it would leave the top-k underfull. Stops a
    # single best-matching document from monopolising every slot, which breaks cross-domain
    # synthesis queries (the 2026-06-04 evaluation, PLAN.md step 3).
    per_doc_cap: int = 3
    context_token_budget: int = 100_000
    # Confidence floor on cross-encoder (ms-marco-MiniLM) scores — raw logits, roughly -11..+11.
    # None disables it (ship default until calibrated against the live corpus + negative-control
    # queries; the 2026-06-05 evaluation, PLAN.md). When set: the rerank refill never pads the
    # top-k with below-floor candidates, and a retrieval whose best survivor falls below the
    # floor is flagged low-confidence ("the corpus may not cover this") — flagged, not filtered.
    min_rerank_score: float | None = None


class GenerationConfig(BaseModel):
    model: str


class MathOCRConfig(BaseModel):
    """Math-aware page OCR (extract/mathocr.py): recovers LaTeX from pages whose text layer
    is damaged or math-dense. Engine chosen by benchmark (eval-artifacts/mathocr/report.md)."""

    engine: str = Field("got", description="'got' (transformers) | 'qwen' (Ollama VLM) | 'off'")
    model: str = Field("qwen2.5vl:7b", description="Ollama model for the 'qwen' engine.")


class ReposConfig(BaseModel):
    """Tracked code repositories (PLAN.md step 10). `locus watch` checks each repo's git
    HEAD every `check_interval` seconds and re-ingests only when new commits landed —
    the check itself is ~free (git rev-parse). Manual runs: `locus sync [--force]`."""

    paths: list[str] = Field(default_factory=list, description="Absolute repo paths to track.")
    check_interval: float = Field(3600.0, description="Seconds between HEAD checks in `locus watch`.")


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
    generation: GenerationConfig
    # Optional: absent [mcp] in config.toml falls back to defaults (query disabled).
    mcp: MCPConfig = Field(default_factory=MCPConfig)
    # Optional: absent [mathocr] falls back to defaults (qwen engine via Ollama).
    mathocr: MathOCRConfig = Field(default_factory=MathOCRConfig)
    # Optional: absent [repos] means no tracked repos (sync is a no-op).
    repos: ReposConfig = Field(default_factory=ReposConfig)

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
