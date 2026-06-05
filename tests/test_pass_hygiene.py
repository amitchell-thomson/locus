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


def test_unbalanced_brackets_are_noise():
    # Truncated extractions from the 2026-06-05 evaluation ("SVD (", "MAP (").
    for bad in ("SVD (", "MAP (", "foo (bar", "matrix [A", "set {x"):
        assert is_noise(bad), bad
    # Balanced parentheticals are legitimate names.
    for good in ("LTI (linear time-invariant) system", "MAP (maximum a posteriori)"):
        assert not is_noise(good), good


def test_grounding_rejects_unattested_names():
    from locus.ingest.entities import is_grounded

    source = "The Kalman filter estimates the hidden state of an LTI system."
    assert is_grounded("Kalman filter", source)
    assert is_grounded("kalman", source)            # case-insensitive
    assert is_grounded("LTI", source)               # acronym: no 3+ letter token -> passes
    assert is_grounded("Kalman filtering bank", source)  # any-token match (loose on purpose)
    # Cross-document bleed: control-theory entities on an econometrics section.
    assert not is_grounded("transfer function", source.replace("LTI system", "VAR model"))
    assert not is_grounded("Fourier transform", source)


def test_extract_entities_filters_ungrounded_and_malformed(monkeypatch):
    from locus.ingest import entities as ent_mod

    raw = ent_mod._Entities(
        entities=[
            ent_mod.Entity(name="Kalman filter", type="method"),
            ent_mod.Entity(name="SVD (", type="method"),               # malformed -> dropped
            ent_mod.Entity(name="transfer function", type="concept"),  # unattested -> dropped
        ]
    )
    monkeypatch.setattr(ent_mod, "generate_structured", lambda *a, **kw: raw)
    out = ent_mod.extract_entities(
        "Sec", "The Kalman filter estimates the hidden state from noisy measurements."
    )
    assert [(e.name, e.type) for e in out] == [("Kalman filter", "method")]


# --- proposition extraction control flow (2026-06-05 evaluation) ----------------------------


def test_zero_raw_propositions_trigger_the_retry(monkeypatch, caplog):
    import logging

    from locus.ingest import propositions as props_mod

    calls = []

    def fake_generate(schema, user, **kw):
        calls.append(user)
        # First call: model returns nothing. Retry: it produces a real claim.
        if len(calls) == 1:
            return schema(propositions=[])
        return schema(propositions=["The Kalman filter is optimal for linear-Gaussian systems."])

    monkeypatch.setattr(props_mod, "generate_structured", fake_generate)
    with caplog.at_level(logging.WARNING):
        out = props_mod.extract_propositions("Methods", "some text")
    assert out == ["The Kalman filter is optimal for linear-Gaussian systems."]
    assert len(calls) == 2
    assert "returned no propositions" in calls[1]  # the zero-raw complaint, not the filter one
    assert any("zero propositions" in r.message for r in caplog.records)


def test_proposition_free_after_retry_is_logged(monkeypatch, caplog):
    import logging

    from locus.ingest import propositions as props_mod

    monkeypatch.setattr(
        props_mod, "generate_structured", lambda schema, user, **kw: schema(propositions=[])
    )
    with caplog.at_level(logging.WARNING):
        assert props_mod.extract_propositions("Methods", "some text") == []
    assert any("proposition-free after retry" in r.message for r in caplog.records)


def test_math_heavy_prompt_variant(monkeypatch):
    from locus.ingest import propositions as props_mod

    captured = {}

    def fake_generate(schema, user, **kw):
        captured.setdefault("user", user)
        return schema(propositions=["The Biot number is defined as Bi = hL/k."])

    monkeypatch.setattr(props_mod, "generate_structured", fake_generate)
    props_mod.extract_propositions("Methods", "x", math_heavy=True)
    assert "math-dense" in captured["user"]
    captured.clear()
    props_mod.extract_propositions("Methods", "x", math_heavy=False)
    assert "math-dense" not in captured["user"]


# --- gap flagging (2026-06-05 evaluation: the pass was inert) -------------------------------


def test_deferral_hints_extracts_explicit_phrases():
    from locus.ingest.gaps import deferral_hints

    texts = [
        "Root locus design is powerful. This method is not covered in this course. "
        "Bode plots are treated next.",
        "A full stability proof is beyond the scope of these notes. Basic familiarity with "
        "Laplace transforms is assumed familiar to the reader.",
        "Nothing deferred here at all.",
    ]
    hints = deferral_hints(texts)
    assert any("not covered in this course" in h for h in hints)
    assert any("beyond the scope" in h for h in hints)
    assert any("assumed familiar" in h.lower() for h in hints)
    assert len(hints) == len(set(hints))  # deduplicated


def test_flag_gaps_prompt_carries_summaries_and_hints(monkeypatch):
    from locus.ingest import gaps as gaps_mod

    captured = {}

    def fake_generate(schema, user, **kw):
        captured["user"] = user
        return schema(gaps=["Root locus rules are mentioned but not covered."])

    monkeypatch.setattr(gaps_mod, "generate_structured", fake_generate)
    out = gaps_mod.flag_gaps(
        "Control Notes",
        "Thesis: t\nMethod: m\nResult: r\nLimitations: l",
        sections=[
            ("Stability", "Poles determine stability.", "Stable iff poles are in the LHP."),
            (
                "Design",
                "Compensator design overview.",
                "Root locus design is not covered in this course.",
            ),
        ],
    )
    assert out == ["Root locus rules are mentioned but not covered."]
    user = captured["user"]
    assert "Section-by-section coverage:" in user
    assert "Poles determine stability." in user            # summaries ground the coverage map
    assert "not covered in this course" in user            # deferral hint fed back
    assert "CONFIRMED gap" in user


def test_flag_gaps_without_sections_still_works(monkeypatch):
    from locus.ingest import gaps as gaps_mod

    monkeypatch.setattr(
        gaps_mod, "generate_structured", lambda schema, user, **kw: schema(gaps=[])
    )
    assert gaps_mod.flag_gaps("T", "overview") == []


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
