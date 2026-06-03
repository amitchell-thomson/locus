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
    context_token_budget: int = 100_000


class GenerationConfig(BaseModel):
    model: str


class Config(BaseModel):
    ollama: OllamaConfig
    paths: PathsConfig
    embed: EmbedConfig
    retrieve: RetrieveConfig
    generation: GenerationConfig

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
