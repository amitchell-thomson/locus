"""Stage 3: embedding via Ollama.

These are integration tests — they need a running Ollama with nomic-embed-text. They skip
cleanly when the server/model is unavailable so the suite stays green elsewhere.
"""

import math

import pytest

from locus.config import load
from locus.ingest.embed import embed_text, embed_texts, embedding_model


def _ollama_ready() -> bool:
    try:
        return bool(embed_texts(["ping"]))
    except Exception:
        return False


pytestmark = pytest.mark.skipif(not _ollama_ready(), reason="Ollama / nomic-embed-text unavailable")


def test_embed_returns_configured_dim():
    dim = load().embed.dim
    vecs = embed_texts(["control theory", "signal processing"])
    assert len(vecs) == 2
    assert all(len(v) == dim for v in vecs)


def test_embed_single_text():
    assert len(embed_text("a single string")) == load().embed.dim


def test_empty_input_returns_empty():
    assert embed_texts([]) == []


def test_vectors_are_unit_normalised():
    (v,) = embed_texts(["nomic vectors are normalised"])
    norm = math.sqrt(sum(x * x for x in v))
    assert abs(norm - 1.0) < 1e-3


def test_embedding_model_matches_config():
    assert embedding_model() == load().ollama.embed_model
