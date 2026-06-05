"""Math-aware page OCR with deterministic QC fallback (plan step 6, eval phase D).

Pages flagged by extract/pdf.py's damage/math detector (PageFlags.needs_ocr) are re-read by
an OCR-to-markup engine; the recovered markdown+LaTeX REPLACES that page's text-layer text.
Replacement is guarded by quality checks — if the OCR output fails them, the original text is
kept and the page is recorded in the audit trail (`OcrResult.fallbacks`). This gives
whole-page replacement the safety of keep-both without polluting chunks/embeddings with the
corrupted variant (decision record: PLAN.md step 6).

Engines (selected by config `[mathocr].engine`; benchmarked on flagged corpus pages —
see eval-artifacts/mathocr/report.md for the race that picked the default):
  - "qwen"  : qwen2.5vl via Ollama. No extra Python deps; the model swaps with the ingest
              model between passes (8 GB VRAM; ingest is time-unbounded, §2.4).
  - "got"   : GOT-OCR-2.0 via transformers (needs the `mathocr` extra).
  - "off"   : disable the pass (extraction behaves as before).
"""

from __future__ import annotations

import logging
import re
from collections import Counter

from locus.config import load

log = logging.getLogger(__name__)

# Rendering: ~144 DPI is enough for the OCR engines and keeps VLM image tokens bounded.
_RENDER_ZOOM = 2.0

PROMPT = (
    "Transcribe this page to GitHub-flavored Markdown.\n"
    "- Transcribe ALL text faithfully and completely; do not summarize, skip, or describe.\n"
    "- Typeset every mathematical expression in LaTeX: $...$ inline, $$...$$ for display "
    "equations.\n"
    "- Preserve the heading/list structure you see.\n"
    "- Output ONLY the transcription, no commentary."
)


# --- deterministic QC on the OCR output ----------------------------------------------------

# Same damage signatures the detector uses: OCR output must not still carry them.
from locus.extract.pdf import _BROKEN_LIGATURES, _SYMBOL_GARBAGE  # noqa: E402

_MIN_LENGTH_RATIO = 0.35  # OCR may legitimately compress layout noise, but not lose 65%+
_LOOP_NGRAM = 12  # words per shingle for the repetition detector
_LOOP_MAX_REPEAT = 4  # a 12-gram appearing 5+ times is a degeneration loop

# Char-level degeneration: a short unit (2-16 chars) repeated 10+ times consecutively, e.g.
# GOT emitting '|c|c|c|c...' (a runaway LaTeX tabular spec) or '[C@@H]3[C@@H]3...' for a
# chemical figure. Word-shingles miss these — the loop is one giant space-free "word"
# (observed live: a tabular loop swallowed the tail of a page, Biot definition included,
# and passed QC). The unit must contain >=2 distinct characters: runs of a single char
# ('0000...' binary listings, aligned whitespace, dotted rules) are legitimate content.
_CHAR_LOOP = re.compile(r"(\S{2,16})(?:\1){9,}")


def _has_repetition_loop(text: str) -> bool:
    """Detect autoregressive degeneration: repeated word shingles or repeated char units."""
    for m in _CHAR_LOOP.finditer(re.sub(r"\s+", " ", text)):
        if len(set(m.group(1))) >= 2:
            return True
    words = text.split()
    if len(words) < _LOOP_NGRAM * 2:
        return False
    shingles = Counter(
        " ".join(words[i : i + _LOOP_NGRAM]) for i in range(len(words) - _LOOP_NGRAM)
    )
    return shingles.most_common(1)[0][1] > _LOOP_MAX_REPEAT


def qc_reject_reason(ocr_text: str, original_text: str) -> str | None:
    """Why the OCR output must not replace the original page text, or None if it may.

    Checks are deliberately deterministic and cheap (no inference): empty/short output,
    degeneration loops, and residual damage signatures the pass exists to remove.
    """
    t = ocr_text.strip()
    if not t:
        return "empty"
    original = original_text.strip()
    # Only enforce the length floor when the original had substance: a page whose text
    # layer was nearly empty (image-math) is exactly where OCR adds the most.
    if len(original) > 400 and len(t) < _MIN_LENGTH_RATIO * len(original):
        return "too-short"
    if _has_repetition_loop(t):
        return "repetition-loop"
    if len(_BROKEN_LIGATURES.findall(t)) >= 2 or len(_SYMBOL_GARBAGE.findall(t)) >= 2:
        return "residual-corruption"
    return None


# --- engines -------------------------------------------------------------------------------


def _ocr_qwen(page) -> str:
    """qwen2.5vl via Ollama: render the page, one chat call with the image."""
    from ollama import Client

    cfg = load()
    png = page.get_pixmap(matrix=__import__("pymupdf").Matrix(_RENDER_ZOOM, _RENDER_ZOOM))
    client = Client(host=cfg.ollama.host)
    resp = client.chat(
        model=cfg.mathocr.model,
        messages=[{"role": "user", "content": PROMPT, "images": [png.tobytes("png")]}],
        options={"temperature": 0.0, "num_ctx": 8192},
    )
    return resp["message"]["content"]


_GOT_CACHE: dict = {}


def _ocr_got(page) -> str:
    """GOT-OCR-2.0 via transformers (the `mathocr` extra). Loaded once per process."""
    import io

    import pymupdf
    from PIL import Image

    import torch

    if "model" not in _GOT_CACHE:
        from transformers import (
            AutoTokenizer,
            GotOcr2ForConditionalGeneration,
            GotOcr2ImageProcessor,
            GotOcr2Processor,
        )

        name = "stepfun-ai/GOT-OCR-2.0-hf"
        # Explicit classes: transformers 5.x's Auto* registry no longer resolves GOT.
        _GOT_CACHE["processor"] = GotOcr2Processor(
            image_processor=GotOcr2ImageProcessor.from_pretrained(name),
            tokenizer=AutoTokenizer.from_pretrained(name),
        )
        device = "cuda" if torch.cuda.is_available() else "cpu"
        _GOT_CACHE["model"] = (
            GotOcr2ForConditionalGeneration.from_pretrained(name, dtype=torch.float16)
            .to(device)
            .eval()
        )
    proc, model = _GOT_CACHE["processor"], _GOT_CACHE["model"]
    # Re-promote to GPU if a previous release_gpu() parked the model on the CPU.
    if torch.cuda.is_available() and model.device.type == "cpu":
        model = _GOT_CACHE["model"] = model.to("cuda")
    png = page.get_pixmap(matrix=pymupdf.Matrix(_RENDER_ZOOM, _RENDER_ZOOM))
    image = Image.open(io.BytesIO(png.tobytes("png"))).convert("RGB")
    inputs = proc(image, return_tensors="pt", format=True).to(model.device)
    import torch

    with torch.no_grad():
        out = model.generate(
            **inputs, max_new_tokens=4096, do_sample=False,
            stop_strings="<|im_end|>", tokenizer=proc.tokenizer,
        )
    return proc.decode(out[0, inputs["input_ids"].shape[1]:], skip_special_tokens=True)


_ENGINES = {"qwen": _ocr_qwen, "got": _ocr_got}


def release_gpu() -> None:
    """Park the cached GOT model on the CPU and free its VRAM.

    Called after a document's OCR pass: the LLM passes that follow need the 8 GB card for
    Ollama — with GOT's ~3.5 GB still resident, qwen gets split across CPU/GPU and generation
    slows several-fold (observed live). The hop back to GPU on the next document costs ~1 s.
    """
    model = _GOT_CACHE.get("model")
    if model is not None and model.device.type == "cuda":
        import torch

        _GOT_CACHE["model"] = model.to("cpu")
        torch.cuda.empty_cache()


# --- the pass ------------------------------------------------------------------------------


class OcrOutcome:
    """Audit trail for one document's OCR pass."""

    def __init__(self) -> None:
        self.replaced: list[int] = []  # 1-based pages whose text was replaced
        self.fallbacks: list[tuple[int, str]] = []  # (page, qc reason) kept original


def ocr_pages(doc, page_texts: list[str], flagged: list[int]) -> tuple[list[str], OcrOutcome]:
    """OCR the flagged (0-based) pages of an open pymupdf doc; return new texts + audit.

    Failures never abort extraction: an engine error on one page falls back to that page's
    original text and is recorded, consistent with §6's quarantine-not-crash rule.
    """
    cfg = load().mathocr
    outcome = OcrOutcome()
    if cfg.engine == "off" or not flagged:
        return page_texts, outcome
    if cfg.engine == "got":
        # Evict the (6 GB) Ollama ingest model BEFORE GOT takes the card: against a resident
        # LLM, GOT's allocation OOMs and every page silently falls back un-OCR'd. The LLM
        # passes reload it after release_gpu() below frees the card again.
        from locus.ingest.llm import unload as _unload_llm

        _unload_llm()
    engine = _ENGINES[cfg.engine]
    out = list(page_texts)
    try:
        for i in flagged:
            try:
                ocr_text = engine(doc[i])
            except Exception as exc:  # engine/server error: keep original, keep going
                log.warning("math-OCR failed on page %d: %s", i + 1, exc)
                outcome.fallbacks.append((i + 1, f"engine-error: {exc}"))
                continue
            ocr_text = _strip_fences(ocr_text)
            reason = qc_reject_reason(ocr_text, page_texts[i])
            if reason is None:
                out[i] = ocr_text
                outcome.replaced.append(i + 1)
            else:
                log.info("math-OCR QC fallback on page %d (%s)", i + 1, reason)
                outcome.fallbacks.append((i + 1, reason))
    finally:
        release_gpu()  # the LLM passes that follow need the whole card for Ollama
    return out, outcome


_FENCE = re.compile(r"\A```(?:markdown|md)?\s*\n(.*)\n```\s*\Z", re.DOTALL)


def _strip_fences(text: str) -> str:
    """VLMs sometimes wrap the whole transcription in a markdown fence; unwrap it."""
    m = _FENCE.match(text.strip())
    return m.group(1) if m else text
