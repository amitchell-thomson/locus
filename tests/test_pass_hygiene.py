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


def test_filter_gaps_drops_summarisation_artifacts():
    from locus.ingest.gaps import filter_gaps

    doc_text = (
        "We tune the confidence threshold θC=0.8 in the Stage C ablation studies. "
        "Preprocessing of the 14-variable panel applies 252-day z-scoring and a residual "
        "bootstrap for the likelihood-ratio tests over a 90-day window. "
        "Root locus design is not covered in this course."
    )
    hints = ["Root locus design is not covered in this course."]
    gaps = [
        # False absences (verbatim class from the 2026-06-05 second evaluation): the doc
        # discusses these at length — summarisation artifacts, must be dropped.
        "The study does not detail the exact confidence threshold used in ablation studies.",
        "The document does not explain the preprocessing steps applied to the panel data.",
        "No information is provided on how the likelihood-ratio tests are conducted.",
        # Hint-backed mentioned-but-not-covered: kept.
        "Root locus design rules are mentioned but not covered.",
        # Genuinely absent topic: kept.
        "The document does not cover option pricing or hedging strategies.",
    ]
    kept = filter_gaps(gaps, hints, doc_text)
    assert kept == [
        "Root locus design rules are mentioned but not covered.",
        "The document does not cover option pricing or hedging strategies.",
    ]


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


# --- summary grounding guard (round-3 evaluation: hallucinated code summaries) -------------


HMM_SOURCE = '''\
"""Gaussian HMM regime detector with multi-seed selection."""

import numpy as np
from hmmlearn.hmm import GaussianHMM


class HMMRegimeDetector:
    def fit_best_of_n_seeds(self, X, n_seeds=10):
        """Fit candidate models across seeds and keep the highest-likelihood one."""
        models = [self._fit_one(X, seed) for seed in range(n_seeds)]
        return max(models, key=lambda m: m.score(X))

    def _fit_one(self, X, seed):
        return GaussianHMM(n_components=3, random_state=seed).fit(X)
'''


def test_grounding_rejects_the_round3_hallucinations():
    from locus.ingest.summarize import is_grounded

    # Verbatim from the corpus: fluent, plausible, and about a different topic entirely.
    assert not is_grounded(
        "The document discusses basic electrical circuits and their components.", HMM_SOURCE
    )
    assert not is_grounded(
        "The document describes a new algorithm for image recognition with improved accuracy.",
        HMM_SOURCE,
    )
    # All-boilerplate filler that names nothing from the unit.
    assert not is_grounded(
        "Document contains information on system architecture and key components.", HMM_SOURCE
    )


def test_grounding_accepts_honest_summaries():
    from locus.ingest.summarize import is_grounded

    assert is_grounded(
        "Implements a Gaussian HMM regime detector; fit_best_of_n_seeds fits candidate "
        "models across random seeds and keeps the highest-likelihood one.",
        HMM_SOURCE,
    )
    # Morphology must not break matching ("detector" vs "detection" etc.).
    assert is_grounded(
        "Detects regimes with a Gaussian hidden Markov model, selecting among seeded fits "
        "by likelihood scoring.",
        HMM_SOURCE,
    )


def test_summarize_retries_then_returns_grounded(monkeypatch):
    from locus.ingest import summarize as sum_mod

    outputs = iter([
        sum_mod.SectionSummary(summary="The document discusses basic electrical circuits."),
        sum_mod.SectionSummary(summary="A Gaussian HMM regime detector selecting seeds by likelihood."),
    ])
    prompts: list[str] = []

    def fake(schema, user, **kw):
        prompts.append(user)
        return next(outputs)

    monkeypatch.setattr(sum_mod, "generate_structured", fake)
    out = sum_mod.summarize_section("src/regimes/hmm.py", HMM_SOURCE, code=True)
    assert out.grounded is True
    assert "regime detector" in out.summary
    assert len(prompts) == 2 and "MUST use the actual names" in prompts[1]


def test_summarize_falls_back_to_code_signature(monkeypatch):
    from locus.ingest import summarize as sum_mod

    bad = sum_mod.SectionSummary(summary="The document discusses basic electrical circuits.")
    monkeypatch.setattr(sum_mod, "generate_structured", lambda *a, **kw: bad)
    out = sum_mod.summarize_section("src/regimes/hmm.py", HMM_SOURCE, code=True)
    assert out.grounded is False
    # Deterministic signature summary: docstring first line + def/class names.
    assert "HMMRegimeDetector" in out.summary
    assert "fit_best_of_n_seeds" in out.summary
    assert "Gaussian HMM regime detector" in out.summary


def test_summarize_falls_back_to_leading_text_for_prose(monkeypatch):
    from locus.ingest import summarize as sum_mod

    bad = sum_mod.SectionSummary(summary="Discusses image recognition accuracy gains.")
    monkeypatch.setattr(sum_mod, "generate_structured", lambda *a, **kw: bad)
    text = "The Biot number compares conductive and convective resistance in a body. " * 4
    out = sum_mod.summarize_section("Transient conduction", text)
    assert out.grounded is False
    assert out.summary.startswith("Transient conduction: The Biot number compares")


# --- window-bounded summary input (round-5 audit: 12k-token CLAUDE.md section slid out of
# the 8192 num_ctx and was summarised as a non-existent 'image compression' paper) ---------


def test_fit_to_window_truncates_oversized_text():
    from locus.ingest.llm import fit_to_window

    small = "word " * 100
    assert fit_to_window(small) == small  # under budget: untouched
    huge = "alpha bravo charlie " * 5000  # ~100k chars >> any window budget
    bounded = fit_to_window(huge)
    assert len(bounded) < len(huge)
    assert bounded.endswith("[... source truncated to fit the model context window]")
    assert bounded.startswith("alpha bravo charlie")  # head kept — the model sees the source


def test_summarize_feeds_window_bounded_text_but_grounds_on_full(monkeypatch):
    from locus.ingest import summarize as sum_mod

    # Source far beyond the window; the distinctive term appears ONLY in the tail.
    head = "regime detector gaussian likelihood seeds " * 3000  # ~130k chars
    tail = " The zeta-functional quasinorm estimator concludes the module."
    source = head + tail
    prompts: list[str] = []

    def fake(schema, user, **kw):
        prompts.append(user)
        # Summary grounded only in the TAIL: passes only if the guard sees the full text.
        return sum_mod.SectionSummary(
            summary="Defines the zeta-functional quasinorm estimator for regime detection."
        )

    monkeypatch.setattr(sum_mod, "generate_structured", fake)
    out = sum_mod.summarize_section("big.md", source, code=False)
    # Prompt was bounded (the raw 130k source must not be sent verbatim)...
    assert len(prompts[0]) < len(source)
    assert "[... source truncated" in prompts[0]
    # ...but grounding ran against the FULL text, so the tail-grounded summary passes.
    assert out.grounded is True and "quasinorm" in out.summary
