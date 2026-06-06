"""Figure-description pass: a local VLM makes figures findable (step 11, tier 2 — §15.1).

For each detected figure (extract/figures_detect.py, extract/pptx.py), one Ollama vision
call describes the image; `caption + description` is then embedded into figure_vectors and
searched as a first-class retrieval unit. The description only has to make the figure
FINDABLE — precise interpretation happens at generation, where the actual image goes to
(multimodal) Claude (tier 3). That asymmetry sets the quality bar: faithful and concrete
beats exhaustive.

Hygiene mirrors propositions.py: a faithfulness-first prompt, deterministic QC predicates
on the output (no inference), one bounded retry, then graceful degradation — a figure whose
description fails QC twice is stored caption-only (tier 1, preserve, still holds) and the
audit counts it. `rejection_reason` is re-applied by `locus audit` to stored rows.
"""

from __future__ import annotations

import logging
import re

from locus.config import load
from locus.extract.mathocr import _has_repetition_loop
from locus.ingest import llm

log = logging.getLogger(__name__)

# Cache-key component: bump when the prompt changes so cached descriptions regenerate.
PROMPT_VERSION = "fig-v2"  # v2 (2026-06-06 audit): diagram topology + blur honesty rules

_PROMPT = (
    "Describe this figure faithfully and concretely in 1-4 sentences, so it can be found "
    "by a text search.\n"
    "- Name the axes, labels, and components you can actually read in the image.\n"
    "- For a block or signal-flow diagram: name ONLY the blocks, arrows, and paths that "
    "are actually drawn. Do not add elements (delays, panels, comparisons) the drawing "
    "does not contain, and do not describe layouts (e.g. 'left panel / right panel') "
    "unless the image really has them.\n"
    "- For a plot or chart: state what is plotted against what, and the visible trend.\n"
    "- If a region is blurred or unreadable, say it is unreadable — never guess its "
    "contents.\n"
    "- Do NOT speculate about meaning the figure does not show, and do NOT invent numbers "
    "or labels that are not visible.\n"
    "- Output ONLY the description — no preamble, no markdown."
)

_MIN_WORDS = 8  # a usable description names at least a subject and some structure

# VLM input bound: Ollama 0.30.x runs the qwen2.5vl vision encoder on CPU ("clip_ctx: CLIP
# using CPU backend"), and encode cost scales with patch count — a 2x-zoom render (~1500-
# 2000px) costs ~30 s/figure of pure CPU. 1024px is ample to read labels for a 1-4 sentence
# findability description (precise reading happens at tier 3, where Claude gets the bigger
# image), and cuts encode ~3-4x. The pass cache stays keyed on the ORIGINAL bytes.
_VLM_MAX_EDGE_PX = 1024


def _vlm_input(image_bytes: bytes) -> bytes:
    """Downscale a PNG to the VLM input bound; original bytes on any failure."""
    try:
        from io import BytesIO

        from PIL import Image

        with Image.open(BytesIO(image_bytes)) as im:
            w, h = im.size
            if max(w, h) <= _VLM_MAX_EDGE_PX:
                return image_bytes
            scale = _VLM_MAX_EDGE_PX / max(w, h)
            im = im.resize((max(1, round(w * scale)), max(1, round(h * scale))))
            out = BytesIO()
            im.save(out, format="PNG", optimize=True)
            return out.getvalue()
    except Exception:  # corrupt PNG etc. — let the VLM see the original
        return image_bytes
_REFUSAL = re.compile(
    r"\b(?:i cannot|i can't|i am unable|i'm unable|as an ai|unable to (?:see|view)|"
    r"no (?:image|figure) (?:was |is )?(?:provided|attached|visible))\b",
    re.IGNORECASE,
)


def rejection_reason(description: str) -> str | None:
    """Why a description is unusable as retrieval text, or None if it is fine.

    Deterministic and cheap (no inference) — also re-applied to stored rows by the audit.
    """
    t = description.strip()
    if not t:
        return "empty"
    if len(t.split()) < _MIN_WORDS:
        return "too-short"
    if _REFUSAL.search(t):
        return "refusal"
    if _has_repetition_loop(t):
        return "repetition-loop"
    return None


def describe_figure(
    image_bytes: bytes,
    caption: str | None = None,
    *,
    client=None,
    model: str | None = None,
) -> str | None:
    """One VLM description of the figure, QC'd with one bounded retry.

    The caption (when present) is given as context — it names what the author thinks the
    figure shows, which anchors the description. Returns None when both attempts fail QC
    or the model call errors: the caller stores the figure caption-only and continues
    (per-figure graceful degradation; one stubborn figure must not quarantine the doc).
    """
    model = model or load().figures.model
    image_bytes = _vlm_input(image_bytes)  # bound the CPU-side vision-encode cost
    prompt = _PROMPT
    if caption:
        prompt += f"\n\nThe author's caption for this figure: {caption!r}"

    reason = None
    for attempt in range(2):
        attempt_prompt = prompt
        if attempt == 1:  # bounded repair: say what was wrong, demand a usable answer
            attempt_prompt += (
                f"\n\nYour previous answer was rejected ({reason}). Look at the image again "
                "and describe what is actually visible, in 1-4 plain sentences."
            )
        try:
            out = llm.vision_chat(
                attempt_prompt,
                image_bytes,
                model=model,
                client=client,
                temperature=0.0 if attempt == 0 else 0.3,
                # One image + a short prompt + <=512 out needs nowhere near 8k context —
                # and at num_ctx=8192 qwen2.5vl does NOT fit the 8 GB card (Ollama plans a
                # kept 74/26 CPU/GPU split and the whole figure batch runs several-fold
                # slow; observed live 2026-06-06, capacity-driven — re-split on a clean
                # card). 4096 shrinks the KV/vision buffers so the model fits fully.
                num_ctx=4096,
                num_predict=512,  # 1-4 sentences; a cap stops degeneration loops early
            )
        except llm.IngestExtractionError as exc:
            log.warning("figure description failed: %s", exc)
            return None
        out = " ".join(out.split())
        reason = rejection_reason(out)
        if reason is None:
            return out
        log.info("figure description rejected (%s), attempt %d", reason, attempt + 1)
    log.warning("figure description unusable after retry (%s); storing caption-only", reason)
    return None


def index_text(caption: str | None, description: str | None) -> str:
    """The text that is embedded and reranked for a figure: caption + description.

    The caption carries author intent ('Figure 4: closed-loop step response'), the
    description carries visual content — concatenated they maximise lexical and semantic
    recall. Either may be absent; the result may be "" (such a figure is stored for
    tier-1 preservation but gets no vector — nothing usable to search on).
    """
    parts = [p.strip() for p in (caption, description) if p and p.strip()]
    return "\n".join(parts)
