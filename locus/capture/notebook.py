"""Transcribe a notebook one changed PAGE at a time (Slice 0, 28 Oct).

Loop A used to re-transcribe a whole notebook whenever any ink in it changed. That was tolerable
for a notebook he fills once, and wrong for Jot, which only grows: one new line on page 40
re-read pages 1-39 too, at one vision call per page, every time. Now each inked page has a key (a
hash of its strokes) and its transcription is kept in a small cache beside the bundles
(`data/cache/transcripts/<uuid>.json`); only pages whose key changed are sent, and the note is
assembled from all pages in order, so it still reads as the whole notebook.

The full notebook is still rendered every time (local, about a second): its pages are what the
note's `source_pdf` points at, and the changed pages are cut from that render, so a page is drawn
exactly as it is in the full PDF.

A notebook captured before this existed has no cache, so its first change after deploy
transcribes it whole once; every change after that costs only the pages it touched. A page he
deletes simply drops out of the note.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


def page_key(page) -> str:
    """A fingerprint of one page's strokes (see `rmdoc.ink_hash`, which does the same per doc)."""
    h = hashlib.sha256(page.page_uuid.encode())
    for stroke in page.strokes:
        h.update(b";")
        for x, y in stroke.points:
            h.update(f"{x:.2f},{y:.2f}|".encode())
    return h.hexdigest()


def _load(cache_file: Path) -> dict:
    try:
        return dict(json.loads(cache_file.read_text()))
    except (OSError, ValueError):
        return {}


def transcribe_notebook(rmdoc, *, out_pdf: Path, cache_file: Path, transcribe_pdf, composite):
    """Render `rmdoc` to `out_pdf` and transcribe only its changed pages. Returns
    `(Transcript, pages_transcribed)`, or `(None, 0)` when no page carries a stroke.

    `transcribe_pdf(path) -> Transcript` and `composite(rmdoc, out) -> path` are injected (the
    real ones are `capture.transcribe.transcribe_pdf` and `rmdoc.composite_pdf`).
    """
    import pymupdf

    from locus.capture.transcribe import PageTranscript, Transcript

    inked = sorted(rmdoc.pages, key=lambda p: p.pdf_page)
    if not inked:
        return None, 0
    composite(rmdoc, out_pdf)
    cache = _load(cache_file)
    keys = {p.page_uuid: page_key(p) for p in inked}
    changed = [i for i, p in enumerate(inked)
               if (cache.get(p.page_uuid) or {}).get("key") != keys[p.page_uuid]]

    fresh = Transcript()
    if changed:
        part = out_pdf.with_name(out_pdf.stem + ".changed.pdf")
        src = pymupdf.open(str(out_pdf))
        try:
            # A notebook's render holds exactly its inked pages, in document order, so render
            # page i is inked[i] (composite_pdf draws each inserted page at its position).
            if src.page_count != len(inked):
                changed = list(range(len(inked)))     # cannot map pages: read it all
            src.select(changed)
            src.save(str(part))
        finally:
            src.close()
        try:
            fresh = transcribe_pdf(part)
        finally:
            part.unlink(missing_ok=True)
        if len(fresh.pages) != len(changed):
            raise RuntimeError(
                f"transcribed {len(fresh.pages)} page(s) for {len(changed)} changed: not stored"
            )
        for i, pt in zip(changed, fresh.pages):
            page = inked[i]
            cache[page.page_uuid] = {"key": keys[page.page_uuid], "markdown": pt.markdown,
                                     "illegible": pt.illegible, "uncertain": pt.uncertain}

    whole = Transcript(input_tokens=fresh.input_tokens, output_tokens=fresh.output_tokens,
                       reported_cost_usd=fresh.reported_cost_usd)
    for i, page in enumerate(inked):
        c = cache[page.page_uuid]
        whole.pages.append(PageTranscript(page=page.pdf_page + 1, markdown=c["markdown"],
                                          illegible=c["illegible"], uncertain=c["uncertain"]))
    # Only pages that still exist are kept, so a deleted page does not linger in the cache.
    live = {p.page_uuid for p in inked}
    cache_file.parent.mkdir(parents=True, exist_ok=True)
    cache_file.write_text(json.dumps({k: v for k, v in cache.items() if k in live}, indent=1))
    return whole, len(changed)
