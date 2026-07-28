"""Transcribe a device-rendered handwriting PDF to Markdown via a Claude VISION call.

A staged reMarkable render is a faithful *raster* of the owner's handwriting — no text layer — so
transcription is a vision task: each page is rasterised (pymupdf) and sent as a base64 image to an
Anthropic SDK request, reusing the multimodal pattern `query.py` already uses for figures.

This is DELIBERATELY a different channel from `agent/claude.py` (§10). Phase 0 measured that routing
transcription through `claude -p` spun an 8-turn agentic loop at **$0.23/page**; one SDK vision call
is **~$0.01/page** on Haiku. So Loop A uses the metered SDK for vision and subscription `claude -p`
for the text passes (fill-in, enrich). The SDK gives exact token usage but not a cost figure, so cost
here is an ESTIMATE from the model's per-token price (Phase-0 confirmed Haiku 4.5 = $1/$5 per M).

Fidelity is the whole point (failure mode #3): the prompt transcribes EXACTLY, never invents, and
marks genuinely-unreadable regions `[illegible]` for the later grounded fill-in pass to resolve. The
raw page raster is always kept upstream, so a transcription error is recoverable. The client is
injectable, so the transcription logic is unit-testable without the API.
"""

from __future__ import annotations

import base64
from dataclasses import dataclass, field
from pathlib import Path

from locus.config import Config, load

# Haiku 4.5 pricing (USD per million tokens), Phase-0 confirmed. Cost is an estimate — the SDK
# returns exact token usage, not a price; update alongside `[capture].transcribe_model`.
_INPUT_USD_PER_MTOK = 1.0
_OUTPUT_USD_PER_MTOK = 5.0

_SYSTEM = (
    "You transcribe a photo of the owner's handwritten notes into Markdown, EXACTLY as written. "
    "These are a quant researcher's dense working notes — full of finance/trading jargon, tickers, "
    "abbreviations, symbols, and personal shorthand you will often not recognise.\n"
    "Rules:\n"
    "- Transcribe VERBATIM. Do not summarise, paraphrase, correct spelling/grammar, translate, "
    "expand abbreviations, or 'fix' an unfamiliar term into a common English word (e.g. do not turn "
    "an unreadable domain word into 'camera'). Transcribe shorthand as written.\n"
    "- Preserve structure: headings, bullet/numbered lists, tables, arrows (→), and inline/display "
    "math as LaTeX ($…$ / $$…$$).\n"
    "- FLAG UNCERTAINTY, never hide it. This is the most important rule. If you are not confident "
    "you read a word correctly, write your best guess immediately followed by [?]. If you cannot "
    "read a word or region at all, write [illegible]. A confident WRONG word corrupts the notes and "
    "is far worse than a flag — when in doubt, flag. Expect to flag several words on a dense page.\n"
    "- For a diagram/sketch, write one line: [sketch: neutral one-line description].\n"
    "- If the page has no readable handwriting (blank, or only faint rule lines), output exactly "
    "[blank page] and NOTHING else. Never describe the image or add commentary.\n"
    "Output ONLY the transcription."
)
_USER = "Transcribe this handwritten page to Markdown, exactly as written. Flag every uncertain word."


@dataclass
class PageTranscript:
    page: int          # 1-based page number
    markdown: str
    illegible: int     # count of [illegible] markers (could not read at all)
    uncertain: int = 0  # count of [?] markers (best-guess, flagged low-confidence)

    @property
    def blank(self) -> bool:
        return self.markdown.strip() == "[blank page]"


@dataclass
class Transcript:
    pages: list[PageTranscript] = field(default_factory=list)
    input_tokens: int = 0
    output_tokens: int = 0

    @property
    def markdown(self) -> str:
        """All pages joined, each under a `<!-- page N -->` marker (provenance, invisible in render)."""
        return "\n\n".join(f"<!-- page {p.page} -->\n{p.markdown.strip()}" for p in self.pages)

    @property
    def illegible_total(self) -> int:
        return sum(p.illegible for p in self.pages)

    @property
    def uncertain_total(self) -> int:
        return sum(p.uncertain for p in self.pages)

    @property
    def cost_usd(self) -> float:
        return (
            self.input_tokens / 1_000_000 * _INPUT_USD_PER_MTOK
            + self.output_tokens / 1_000_000 * _OUTPUT_USD_PER_MTOK
        )


def render_pdf_pages(pdf_path: str | Path, *, dpi: int = 150) -> list[bytes]:
    """Rasterise each PDF page to PNG bytes (pymupdf). DPI trades legibility for image size."""
    import pymupdf

    doc = pymupdf.open(str(pdf_path))
    try:
        return [page.get_pixmap(dpi=dpi).tobytes("png") for page in doc]
    finally:
        doc.close()


def _transcribe_page(client, model: str, png: bytes, *, max_tokens: int) -> tuple[str, int, int]:
    resp = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        system=[{"type": "text", "text": _SYSTEM}],
        messages=[{"role": "user", "content": [
            {"type": "image", "source": {
                "type": "base64", "media_type": "image/png",
                "data": base64.standard_b64encode(png).decode("utf-8"),
            }},
            {"type": "text", "text": _USER},
        ]}],
    )
    text = "".join(b.text for b in resp.content if getattr(b, "type", None) == "text")
    usage = getattr(resp, "usage", None)
    return text.strip(), int(getattr(usage, "input_tokens", 0) or 0), int(getattr(usage, "output_tokens", 0) or 0)


def transcribe_pdf(
    pdf_path: str | Path,
    *,
    client=None,
    model: str | None = None,
    dpi: int | None = None,
    max_tokens: int = 4096,
) -> Transcript:
    """Transcribe every page of a handwriting PDF to Markdown. One vision call per page.

    `client` is injectable (tests pass a fake); the default builds an Anthropic client from
    ANTHROPIC_API_KEY. Model/DPI default to `[capture]`."""
    cfg = load().capture
    model = model or cfg.transcribe_model
    dpi = dpi if dpi is not None else cfg.transcribe_dpi
    if client is None:
        import anthropic

        client = anthropic.Anthropic(api_key=Config.anthropic_api_key())

    transcript = Transcript()
    for i, png in enumerate(render_pdf_pages(pdf_path, dpi=dpi), start=1):
        md, in_tok, out_tok = _transcribe_page(client, model, png, max_tokens=max_tokens)
        transcript.pages.append(PageTranscript(
            page=i, markdown=md, illegible=md.count("[illegible]"), uncertain=md.count("[?]"),
        ))
        transcript.input_tokens += in_tok
        transcript.output_tokens += out_tok
    return transcript
