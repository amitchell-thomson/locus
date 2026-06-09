"""Backfill documents.title via the synthesis-pass title arbitration (no re-ingest needed).

The extractor's title heuristic (metadata -> page-1 largest font -> filename) stored banners
("ENGINEERING SCIENCE" for a syllabus), browser-tab suffixes ("... - Colab"), fragments, and
slugs. Titles are not embedded in any vector — they live only in documents.title — so they
can be corrected in place from the stored section summaries, exactly the arbitration new
ingests now run inside synthesize_document (locus/ingest/synthesis.py).

Usage:
  uv run python scripts/backfill_titles.py            # dry run: show would-be changes
  uv run python scripts/backfill_titles.py --apply    # write the changes

Needs Ollama (one structured call per document). Respect the one-ingest-at-a-time rule:
don't run while an ingest is in flight.
"""

from __future__ import annotations

import argparse

from pydantic import BaseModel

from locus.config import load
from locus.db.connection import get_connection
from locus.extract.pdf import title_is_suspect
from locus.ingest.llm import generate_structured


class _Title(BaseModel):
    title: str


PROMPT = (
    "Extracted title candidate: {candidate}\n\n"
    "Section summaries of the document:\n{summaries}\n\n"
    "Return the document's title. The candidate above was extracted heuristically and may be "
    "a page banner, a browser-tab suffix, a fragment, or a filename. If it is the document's "
    "actual title, return it VERBATIM; otherwise give the true title, or a faithful "
    "descriptive one (at most 12 words) based on the summaries. Title only — no quotes, "
    "no trailing punctuation."
)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="write changes (default: dry run)")
    args = parser.parse_args()

    conn = get_connection(load().paths.db)
    try:
        docs = conn.execute("SELECT id, title, source_uri FROM documents ORDER BY id").fetchall()
        changed = 0
        for d in docs:
            # Deterministic guard: a trusted title is never sent to the model at all — an
            # LLM asked to confirm a long correct title tends to shorten it (observed on
            # the arXiv-metadata titles in the first dry run).
            if not title_is_suspect(d["title"]):
                print(f"[{d['id']:>2}] trusted {d['title'][:70]!r}")
                continue
            summaries = [
                r["summary"] for r in conn.execute(
                    "SELECT summary FROM sections WHERE doc_id=? ORDER BY position", (d["id"],)
                ) if r["summary"]
            ]
            joined = "\n".join(f"- {s[:300]}" for s in summaries) or "(no summaries)"
            user = PROMPT.format(candidate=d["title"] or "(none)", summaries=joined)
            new = generate_structured(_Title, user).title.strip().strip("\"'").rstrip(".")
            if not new or new == d["title"]:
                print(f"[{d['id']:>2}] keep    {d['title']!r}")
                continue
            changed += 1
            print(f"[{d['id']:>2}] retitle {d['title']!r}\n             -> {new!r}")
            if args.apply:
                with conn:
                    conn.execute("UPDATE documents SET title=? WHERE id=?", (new, d["id"]))
        verb = "updated" if args.apply else "would update (dry run; pass --apply)"
        print(f"\n{changed}/{len(docs)} titles {verb}")
    finally:
        conn.close()


if __name__ == "__main__":
    main()
