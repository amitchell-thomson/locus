"""Split section text into ~chunk_tokens-sized L3 chunks for embedding.

Token counts use tiktoken's cl100k_base as a stable proxy for nomic's tokenizer (exactness
is unnecessary — chunk size is a tuning knob, not a correctness constraint). Splitting
respects structure: paragraphs first, then sentences, then a hard token split as a last
resort, so a chunk never exceeds the budget and boundaries land at natural breaks where
possible.
"""

from __future__ import annotations

import re
from functools import lru_cache

import tiktoken

from locus.config import load

_PARAGRAPH = re.compile(r"\n\s*\n")
_SENTENCE = re.compile(r"(?<=[.!?])\s+")


@lru_cache(maxsize=1)
def _encoding():
    return tiktoken.get_encoding("cl100k_base")


def count_tokens(text: str) -> int:
    return len(_encoding().encode(text))


def chunk_text(text: str, max_tokens: int | None = None) -> list[str]:
    """Split `text` into chunks of at most `max_tokens` tokens (default: config.embed)."""
    max_tokens = max_tokens or load().embed.chunk_tokens
    enc = _encoding()

    chunks: list[str] = []
    current: list[str] = []
    current_tokens = 0

    def flush() -> None:
        nonlocal current, current_tokens
        if current:
            chunks.append("\n\n".join(current))
            current = []
            current_tokens = 0

    for para in _PARAGRAPH.split(text):
        para = para.strip()
        if not para:
            continue
        ptokens = len(enc.encode(para))
        if ptokens > max_tokens:
            # Paragraph alone overflows: emit what we have, then split it finer.
            flush()
            chunks.extend(_split_oversized(para, max_tokens, enc))
            continue
        if current_tokens + ptokens > max_tokens:
            flush()
        current.append(para)
        current_tokens += ptokens

    flush()

    # Final guarantee: token counts are estimated while packing (joins add a little, and
    # decode/encode is not a perfect round-trip), so enforce the hard budget on every chunk.
    enforced: list[str] = []
    for c in chunks:
        enforced.extend([c] if len(enc.encode(c)) <= max_tokens else _hard_split(c, max_tokens, enc))
    return enforced


def _split_oversized(text: str, max_tokens: int, enc) -> list[str]:
    """Split a too-large block by sentences, then by a hard token cut if a sentence overflows."""
    chunks: list[str] = []
    current: list[str] = []
    current_tokens = 0

    def flush() -> None:
        nonlocal current, current_tokens
        if current:
            chunks.append(" ".join(current))
            current = []
            current_tokens = 0

    for sentence in _SENTENCE.split(text):
        sentence = sentence.strip()
        if not sentence:
            continue
        stokens = len(enc.encode(sentence))
        if stokens > max_tokens:
            flush()
            chunks.extend(_hard_split(sentence, max_tokens, enc))
            continue
        if current_tokens + stokens > max_tokens:
            flush()
        current.append(sentence)
        current_tokens += stokens

    flush()
    return chunks


def _hard_split(text: str, max_tokens: int, enc) -> list[str]:
    """Last resort: split by token windows (e.g. an unbroken formula or URL run).

    A token window is decoded back to text and re-encoded to confirm it really fits the
    budget; if decode/encode round-tripping inflated it past max_tokens, the window shrinks
    until it fits. Guarantees every emitted piece is <= max_tokens (or a single token)."""
    tokens = enc.encode(text)
    pieces: list[str] = []
    i = 0
    while i < len(tokens):
        size = max_tokens
        while size > 1:
            piece = enc.decode(tokens[i : i + size])
            if len(enc.encode(piece)) <= max_tokens:
                break
            size = max(1, int(size * 0.9))
        else:
            piece = enc.decode(tokens[i : i + 1])
        pieces.append(piece)
        i += size
    return pieces
