"""Throwaway measurement for plan step 4 (eval phase B): run the current extractor over the
corpus raw PDFs and report sectioning stats, ToC/dotted-leader incidence, and heading shapes.

Usage: uv run python scripts/measure_sectioning.py
"""

from __future__ import annotations

import re
import statistics

from locus.config import load
from locus.db.connection import get_connection
from locus.extract.pdf import extract_pdf

# A ToC line: text, dotted leader (consecutive or spaced dots), page number.
DOTTED = re.compile(r"(?:\.[ \t]*){4,}[ \t]*\d{1,4}[ \t]*$", re.MULTILINE)


def title_shape(t: str | None) -> str:
    if t is None:
        return "none"
    words = t.split()
    flags = []
    if len(words) > 12:
        flags.append(">12w")
    if t[:1].islower():
        flags.append("lower-start")
    if t.rstrip().endswith(("-", ",", ";")):
        flags.append("dangling-end")
    # sentence-shaped: contains a sentence-final period followed by more prose
    if re.search(r"[a-z]\.\s+[A-Z]", t):
        flags.append("multi-sentence")
    return ",".join(flags) or "ok"


def main() -> None:
    conn = get_connection(load().paths.db)
    rows = conn.execute("SELECT id, title, raw_path FROM documents ORDER BY id").fetchall()
    raw_store = load().paths.raw_store

    for r in rows:
        d = extract_pdf(raw_store / r["raw_path"])
        sizes = [len(s.text) for s in d.sections]
        toc_secs = [s for s in d.sections if len(DOTTED.findall(s.text)) >= 3]
        shapes: dict[str, int] = {}
        for s in d.sections:
            k = title_shape(s.title)
            shapes[k] = shapes.get(k, 0) + 1
        print(f"doc {r['id']:>2}  {r['title'][:48]:<48} pages={d.page_count:>3} "
              f"strategy={d.section_strategy:<9} sections={len(d.sections):>3}")
        print(f"        chars min/med/max = {min(sizes)}/{int(statistics.median(sizes))}/{max(sizes)}"
              f"   <1500c: {sum(1 for x in sizes if x < 1500):>3}"
              f"   <2500c: {sum(1 for x in sizes if x < 2500):>3}")
        print(f"        toc-like sections (>=3 dotted-leader lines): {len(toc_secs)} "
              f"{[s.position for s in toc_secs][:12]}")
        bad = {k: v for k, v in shapes.items() if k not in ("ok", "none")}
        print(f"        title shapes: ok={shapes.get('ok', 0)} none={shapes.get('none', 0)} bad={bad}")


if __name__ == "__main__":
    main()
