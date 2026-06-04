"""Pass hygiene (plan step 5, eval phase C): deterministic proposition/entity filters and
synthesis validation. Pure unit tests — no Ollama, no DB. Examples are taken verbatim from
the 2026-06-04 corpus evaluation findings.
"""

import pytest
from pydantic import ValidationError

from locus.ingest.entities import Entity, is_noise, merge_plural_variants, normalize_name
from locus.ingest.propositions import filter_propositions, rejection_reason
from locus.ingest.synthesis import DocSynthesis

# --- propositions -------------------------------------------------------------------------


def test_meta_propositions_are_rejected():
    assert rejection_reason("Diffusion in bounded media is discussed") == "meta"
    assert rejection_reason("Laplace transform methods are covered in this section") == "meta"
    assert rejection_reason("Partial Differential Equations are discussed in this document") == "meta"
    assert rejection_reason("This section covers the wave equation and its solutions") == "meta"


def test_dropped_formula_signatures_are_rejected():
    assert rejection_reason("The inner product of vectors and is given by .") == "dropped-formula"
    assert rejection_reason("The Laplacian operator is defined as ,") == "dropped-formula"
    assert rejection_reason("The general solution takes the form") == "dropped-formula"


def test_fragments_and_title_echoes_are_rejected():
    # Section 22's sole "proposition" was its truncated title, verbatim.
    t = "to satisfy any given initial condition on the problem."
    assert rejection_reason(t) == "fragment"  # lowercase start: not a sentence
    assert rejection_reason("Boundary conditions", "Boundary conditions") is not None
    assert (
        rejection_reason(
            "Separation of variables procedure",
            "Separation of variables procedure for bounded diffusion problems",
        )
        == "title-echo"
    )


def test_real_propositions_survive():
    keep = [
        "The Biot number is defined as Bi = hL/k.",
        "The Kalman filter is optimal for linear-Gaussian systems.",
        "Stability of an LTI system is determined by the poles of its transfer function.",
        # 'defined as' mid-sentence (content follows) is NOT the dropped-formula signature.
        "The Fourier transform is defined as an integral over all time.",
    ]
    for p in keep:
        assert rejection_reason(p, "Some Section Title") is None, p


def test_filter_propositions_splits_and_preserves_order():
    kept, rejected = filter_propositions(
        [
            "The wave equation governs the motion of a vibrating string.",
            "Travelling waves are discussed",
            "Dispersion relates frequency to wavenumber.",
        ],
        title="Waves",
    )
    assert kept == [
        "The wave equation governs the motion of a vibrating string.",
        "Dispersion relates frequency to wavenumber.",
    ]
    assert rejected == [("Travelling waves are discussed", "meta")]


# --- synthesis ----------------------------------------------------------------------------


def test_blank_synthesis_fields_fail_validation():
    with pytest.raises(ValidationError):
        DocSynthesis(thesis="", method="m", result="r", limitations="l")
    with pytest.raises(ValidationError):
        DocSynthesis(thesis="t", method="   ", result="r", limitations="l")


def test_valid_synthesis_is_stripped_and_kept():
    syn = DocSynthesis(thesis=" t ", method="m", result="r", limitations="l")
    assert syn.thesis == "t"


# --- entities -----------------------------------------------------------------------------


def test_normalize_name_strips_wrapping_and_articles():
    assert normalize_name("  the Laplace   transform, ") == "Laplace transform"
    assert normalize_name("'Kalman filter'") == "Kalman filter"
    assert normalize_name("LTI system") == "LTI system"  # case preserved


def test_noise_entities_are_detected():
    for bad in ("equation 1.36", "Equation 1.30", "fig. 3", "Table 2", "β", "γ", "f(t,x,y,z)", ""):
        assert is_noise(bad), bad
    for good in ("Laplace transform", "ML", "LTI", "Kalman filter", "Abel's formula", "H2"):
        assert not is_noise(good), good


def test_merge_plural_variants_is_evidence_based():
    sections = [
        [Entity(name="Laplace transforms", type="method")],
        [Entity(name="Laplace transform", type="method")],
        [Entity(name="Fourier series", type="concept")],  # 'Fourier serie' attested nowhere
        [Entity(name="Laplace transforms", type="concept")],  # different type: no merge
    ]
    merge_plural_variants(sections)
    assert sections[0][0].name == "Laplace transform"  # collapsed onto attested singular
    assert sections[2][0].name == "Fourier series"  # untouched
    assert sections[3][0].name == "Laplace transforms"  # type mismatch: untouched
