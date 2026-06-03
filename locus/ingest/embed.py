"""Embed text with nomic-embed-text via Ollama.

Returns 768-dim vectors (the dimension is locked in config; nomic produces unit-normalised
vectors, so cosine similarity == dot product and sqlite-vec's L2 KNN ranks semantically).

Every embedding's dimensionality is checked against config: a silent dimension change would
poison the vector tables, and changing the embedding model is a full-re-embed event.
"""

from __future__ import annotations

from functools import lru_cache

from ollama import Client

from locus.config import load

# Embed in batches to bound request size on large documents.
_BATCH = 64


@lru_cache(maxsize=1)
def _client() -> Client:
    return Client(host=load().ollama.host)


def embedding_model() -> str:
    """The configured embedding model id (written into the *_vectors provenance columns)."""
    return load().ollama.embed_model


def embed_texts(texts: list[str]) -> list[list[float]]:
    """Embed a list of texts; returns one 768-dim vector per input, in order."""
    if not texts:
        return []
    cfg = load()
    model, dim, host = cfg.ollama.embed_model, cfg.embed.dim, cfg.ollama.host
    client = _client()

    out: list[list[float]] = []
    for start in range(0, len(texts), _BATCH):
        batch = texts[start : start + _BATCH]
        try:
            resp = client.embed(model=model, input=batch)
        except Exception as exc:  # network / model / server errors
            raise RuntimeError(
                f"Ollama embedding failed (model={model}, host={host}): {exc}"
            ) from exc
        vectors = getattr(resp, "embeddings", None)
        if vectors is None:
            vectors = resp["embeddings"]
        for v in vectors:
            if len(v) != dim:
                raise ValueError(
                    f"Embedding dim {len(v)} != configured {dim} (model={model}). "
                    "A model/dimension change requires a full re-embed of the corpus."
                )
            out.append(list(v))
    return out


def embed_text(text: str) -> list[float]:
    """Embed a single text; convenience wrapper over embed_texts."""
    return embed_texts([text])[0]
