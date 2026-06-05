"""Corpus math-fidelity metric (PLAN.md step 7) — the gate metric for the bulk pour.

"Fraction of math-bearing pages whose formulas survived into the stored text." Math lost at
extraction is NOT Claude-recoverable (§15.0), so this number gates everything else.

Method: re-derive the page damage/math flags from the raw PDFs (cheap, no OCR — `has_math`
is not persisted), sample flagged pages, render each, and have the Claude judge
(locus/eval/math_fidelity.py — the same harness that picked the OCR engine) compare the page
image against the STORED text of the sections overlapping that page.

Caveat: the stored text of an overlapping section usually spans more than the sampled page,
so the `equations_hallucinated` count is meaningless in this mode (neighbouring pages' math
looks "hallucinated") — only fidelity (correct/partial/missing) is reported.
"""

from __future__ import annotations

import json
import random
import tempfile
from dataclasses import dataclass
from pathlib import Path

import pymupdf

from locus.config import Config, load
from locus.eval.math_fidelity import PageJudgement, judge_transcription
from locus.extract.pdf import _page_flags


@dataclass
class PageFidelity:
    doc_id: int
    title: str
    page: int  # 1-based
    judgement: PageJudgement


def _flagged_pages(conn, doc_id: int) -> list[int]:
    """0-based pages of a stored doc that the damage/math detector flags, from its raw PDF."""
    row = conn.execute("SELECT raw_path FROM documents WHERE id=?", (doc_id,)).fetchone()
    pdf = pymupdf.open(str(load().paths.raw_store / row["raw_path"]))
    try:
        return [
            i for i, page in enumerate(pdf)
            if _page_flags(page, i, page.get_text("text")).needs_ocr
        ]
    finally:
        pdf.close()


def _stored_text_for_page(conn, doc_id: int, page_1based: int) -> str:
    """Stored chunk text of every section whose page range covers the page (via section_map)."""
    row = conn.execute("SELECT section_map FROM documents WHERE id=?", (doc_id,)).fetchone()
    positions = [
        m["position"] for m in json.loads(row["section_map"] or "[]")
        if (m.get("page_start") or 0) <= page_1based <= (m.get("page_end") or 0)
    ]
    if not positions:
        return ""
    qmarks = ",".join("?" for _ in positions)
    rows = conn.execute(
        f"SELECT c.raw_text FROM chunks c JOIN sections s ON s.id = c.section_id "
        f"WHERE s.doc_id=? AND s.position IN ({qmarks}) ORDER BY s.position, c.position",
        (doc_id, *positions),
    ).fetchall()
    return "\n".join(r["raw_text"] for r in rows)


def evaluate_math_fidelity(
    conn, *, sample: int = 10, seed: int = 0, judge_model: str | None = None
) -> tuple[list[PageFidelity], dict[str, float]]:
    """Judge a deterministic sample of flagged pages across the corpus."""
    import anthropic

    client = anthropic.Anthropic(api_key=Config.anthropic_api_key())
    model = judge_model or load().generation.model

    docs = conn.execute("SELECT id, title, raw_path FROM documents ORDER BY id").fetchall()
    pool: list[tuple[int, str, int]] = []  # (doc_id, title, page_1based)
    for d in docs:
        pool.extend((d["id"], d["title"], p + 1) for p in _flagged_pages(conn, d["id"]))
    if not pool:
        return [], {}
    picked = pool if sample >= len(pool) else sorted(
        random.Random(seed).sample(pool, sample)
    )

    results: list[PageFidelity] = []
    for doc_id, title, page in picked:
        stored = _stored_text_for_page(conn, doc_id, page)
        row = conn.execute("SELECT raw_path FROM documents WHERE id=?", (doc_id,)).fetchone()
        pdf = pymupdf.open(str(load().paths.raw_store / row["raw_path"]))
        try:
            with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
                png = Path(tmp.name)
            pdf[page - 1].get_pixmap(matrix=pymupdf.Matrix(2.0, 2.0)).save(png)
            j = judge_transcription(client, model, png, stored or "(no stored text)")
            png.unlink(missing_ok=True)
        finally:
            pdf.close()
        results.append(PageFidelity(doc_id=doc_id, title=title, page=page, judgement=j))
    return results, aggregate(results)


def aggregate(results: list[PageFidelity]) -> dict[str, float]:
    if not results:
        return {}
    fidelities = [r.judgement.math_fidelity for r in results]
    return {
        "math_fidelity_mean": sum(fidelities) / len(fidelities),
        "pages_above_0.8": sum(1 for f in fidelities if f >= 0.8) / len(fidelities),
        "pages_judged": float(len(results)),
    }


def format_results(results: list[PageFidelity], agg: dict[str, float]) -> str:
    lines = []
    for r in results:
        j = r.judgement
        lines.append(
            f"  {j.math_fidelity:.2f}  doc {r.doc_id:>2} p{r.page:<4} "
            f"(eq {j.equations_correct}+{j.equations_partial}p/{j.equations_on_page})  "
            f"{r.title[:44]}"
        )
    lines.append("")
    for k, v in agg.items():
        lines.append(f"  {k:<24} {v:.3f}")
    return "\n".join(lines)
