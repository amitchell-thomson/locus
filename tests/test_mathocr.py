"""Math-OCR QC fallback (plan step 6, eval phase D): deterministic checks that decide whether
an OCR output may replace a page's text-layer text. Pure unit tests — no models.
"""

from locus.extract.mathocr import _has_repetition_loop, _strip_fences, qc_reject_reason

GOOD = (
    "## Fourier Series\n\nThe transfer function $H(\\omega)$ relates input and output: "
    "$$Y(\\omega) = H(\\omega) X(\\omega)$$ where $x(t) = \\mathrm{Re}(X(\\omega) e^{i\\omega t})$ "
    "describes the temporal input for a linear system driven at a single frequency."
)
ORIGINAL = "the transfer function H(!) relates input and output via ei!t " * 8


def test_good_ocr_output_passes():
    assert qc_reject_reason(GOOD, ORIGINAL) is None


def test_empty_and_short_outputs_are_rejected():
    assert qc_reject_reason("", ORIGINAL) == "empty"
    assert qc_reject_reason("  \n ", ORIGINAL) == "too-short" or qc_reject_reason("  \n ", ORIGINAL) == "empty"
    # Massive content loss vs a substantial original is rejected...
    assert qc_reject_reason("tiny.", "x" * 1000) == "too-short"
    # ...but a near-empty original (image-math page) accepts short OCR output.
    assert qc_reject_reason("The matrix $A$ is symmetric positive definite here.", "") is None


def test_repetition_loop_is_rejected():
    loop = "the quick brown fox jumps over the lazy dog again and again " * 20
    assert _has_repetition_loop(loop)
    assert qc_reject_reason(loop, ORIGINAL) == "repetition-loop"
    assert not _has_repetition_loop(GOOD)


def test_char_level_loops_are_rejected():
    """Regression (live re-ingest): space-free degeneration loops escaped the word-shingle
    detector — a runaway tabular spec swallowed a page tail (Biot definition included)."""
    prose = "The temperature distribution depends on boundary conditions. "
    assert _has_repetition_loop(prose + "|c" * 200)  # GOT tabular loop
    assert _has_repetition_loop(prose + "[C@@H]3" * 40)  # SMILES-string loop
    # Single-char runs are legitimate content, not loops:
    assert not _has_repetition_loop("set $t0 to 0000000000001000 0000000000000000")  # binary
    assert not _has_repetition_loop("2019-2024" + " " * 40 + "A-Levels: Maths (A*)")  # padding
    # A short dotted rule is fine; a LONG one trips the word-shingle rule, which is
    # acceptable — pages dominated by dotted leaders are ToC noise, and a QC reject
    # only means the original text is kept.
    assert not _has_repetition_loop("General balance principle " + ". " * 8)


def test_residual_corruption_is_rejected():
    # OCR that still contains the damage signatures failed at its one job. (Padded past the
    # length floor so the specific reason, not 'too-short', is what fires.)
    bad = (
        "The rst condition denes the eld for the system under study. "
        "The response H(!) follows directly from ei!t analysis of the input. "
        + " ".join(f"Sentence number {i} adds distinct explanatory prose to the page." for i in range(8))
    )
    assert qc_reject_reason(bad, ORIGINAL) == "residual-corruption"


def test_fence_stripping():
    assert _strip_fences("```markdown\n# Title\nBody $x$\n```") == "# Title\nBody $x$"
    assert _strip_fences("# Title\nBody $x$") == "# Title\nBody $x$"
