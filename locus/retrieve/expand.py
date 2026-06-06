"""Hierarchical expansion (§7): attach each candidate's parent context via plain SQL joins.

Free, no inference. For every survivor we fetch its section summary/title/page-range and its
document synthesis (thesis/method/result/limitations) + title, plus chunk provenance
(file_path:line / video ?t=). This is the whole reason L1/L2 exist: a precise hit arrives with
the surrounding context Claude needs.
"""

from __future__ import annotations

import json
from dataclasses import dataclass

from locus.retrieve.search import Candidate


@dataclass
class Expanded:
    candidate: Candidate
    doc_id: int
    doc_title: str | None
    thesis: str | None
    method: str | None
    result: str | None
    limitations: str | None
    section_id: int | None
    section_title: str | None
    section_summary: str | None
    page_start: int | None
    page_end: int | None
    # chunk-only provenance
    file_path: str | None = None
    line_start: int | None = None
    line_end: int | None = None
    video_timestamp: int | None = None
    # figure-only provenance (step 11): the raw-store PNG + caption for generation/MCP
    figure_path: str | None = None
    figure_caption: str | None = None
    figure_kind: str | None = None  # 'raster' | 'vector' | 'slide'
    figure_page: int | None = None  # 1-based page / slide number


def expand(conn, candidates: list[Candidate]) -> list[Expanded]:
    out: list[Expanded] = []
    for c in candidates:
        doc = conn.execute(
            "SELECT title, thesis, method, result, limitations, section_map "
            "FROM documents WHERE id=?",
            (c.doc_id,),
        ).fetchone()
        section_title = section_summary = None
        page_start = page_end = None
        position = None
        if c.section_id is not None:
            s = conn.execute(
                "SELECT title, summary, position FROM sections WHERE id=?", (c.section_id,)
            ).fetchone()
            if s is not None:
                section_title, section_summary, position = s["title"], s["summary"], s["position"]

        if position is not None and doc and doc["section_map"]:
            for m in json.loads(doc["section_map"]):
                if m.get("position") == position:
                    page_start, page_end = m.get("page_start"), m.get("page_end")
                    break

        file_path = line_start = line_end = video_timestamp = None
        if c.kind == "chunk":
            ch = conn.execute(
                "SELECT file_path, line_start, line_end, video_timestamp FROM chunks WHERE id=?",
                (c.id,),
            ).fetchone()
            if ch is not None:
                file_path, line_start = ch["file_path"], ch["line_start"]
                line_end, video_timestamp = ch["line_end"], ch["video_timestamp"]

        figure_path = figure_caption = figure_kind = figure_page = None
        if c.kind == "figure":
            fig = conn.execute(
                "SELECT raw_path, caption, kind, page FROM figures WHERE id=?", (c.id,)
            ).fetchone()
            if fig is not None:
                figure_path, figure_caption = fig["raw_path"], fig["caption"]
                figure_kind, figure_page = fig["kind"], fig["page"]
                # A figure's page beats the section's page range for citation precision.
                page_start = page_end = fig["page"]

        out.append(
            Expanded(
                candidate=c,
                doc_id=c.doc_id,
                doc_title=doc["title"] if doc else None,
                thesis=doc["thesis"] if doc else None,
                method=doc["method"] if doc else None,
                result=doc["result"] if doc else None,
                limitations=doc["limitations"] if doc else None,
                section_id=c.section_id,
                section_title=section_title,
                section_summary=section_summary,
                page_start=page_start,
                page_end=page_end,
                file_path=file_path,
                line_start=line_start,
                line_end=line_end,
                video_timestamp=video_timestamp,
                figure_path=figure_path,
                figure_caption=figure_caption,
                figure_kind=figure_kind,
                figure_page=figure_page,
            )
        )
    return out
