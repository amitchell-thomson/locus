"""Stage 4: ingest passes — integration smoke tests against a live qwen2.5 via Ollama.

Skipped when Ollama / the ingest model is unavailable. Assertions are structural (shape and
types), not exact wording, to stay robust to model nondeterminism.
"""

from typing import get_args

import pytest

from locus.ingest.entities import EntityType, extract_entities
from locus.ingest.gaps import flag_gaps
from locus.ingest.llm import IngestExtractionError, generate_structured
from locus.ingest.propositions import extract_propositions
from locus.ingest.summarize import SectionSummary, summarize_section
from locus.ingest.synthesis import synthesize_document

TEXT = (
    "Kalman filtering estimates the hidden state of a linear dynamical system from a series "
    "of noisy measurements. The algorithm was developed by Rudolf Kalman. It proceeds in a "
    "predict step and an update step, and is optimal for linear-Gaussian systems."
)


def _ollama_ready() -> bool:
    try:
        return bool(generate_structured(SectionSummary, "Summarize: hello.").summary)
    except IngestExtractionError:
        return False
    except Exception:
        return False


pytestmark = pytest.mark.skipif(not _ollama_ready(), reason="Ollama / qwen2.5 unavailable")


def test_summarize_returns_text_and_title():
    result = summarize_section("Kalman Filter", TEXT)
    assert isinstance(result.summary, str) and len(result.summary) > 0
    assert result.title is None or isinstance(result.title, str)


def test_propositions_are_a_list_of_strings():
    props = extract_propositions("Kalman Filter", TEXT)
    assert isinstance(props, list)
    assert all(isinstance(p, str) and p for p in props)
    assert len(props) >= 1  # this text clearly asserts claims


def test_entities_have_valid_types():
    allowed = set(get_args(EntityType))
    ents = extract_entities("Kalman Filter", TEXT)
    assert isinstance(ents, list)
    assert all(e.type in allowed for e in ents)
    assert len(ents) >= 1  # at least the Kalman filter / Rudolf Kalman


def test_synthesis_fills_all_four_fields():
    syn = synthesize_document("Kalman Filter", [TEXT])
    assert all(getattr(syn, f) for f in ("thesis", "method", "result", "limitations"))


def test_gaps_returns_a_list():
    gaps = flag_gaps("Kalman Filter", TEXT)
    assert isinstance(gaps, list)
    assert all(isinstance(g, str) for g in gaps)
