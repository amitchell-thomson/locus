"""Extract structured content from Markdown, plain-text, and Jupyter-notebook files.

All three produce an `ExtractedDoc` (see extract/base.py) so the ingest pipeline treats
them exactly like PDFs downstream (§2.6). Section boundaries:

  - markdown : ATX headings (`# ...` .. `###### ...`), fence-aware — a `#` line inside a
               ``` / ~~~ code fence is a comment, never a heading. Setext underlines are
               deliberately not recognised (they collide with horizontal rules and YAML
               frontmatter delimiters). Strategy "headings", or "single"/"windowed" when
               the file has no headings.
  - text     : no headings — one section, char-windowed when oversized ("single"/"windowed").
  - notebook : .ipynb JSON (nbformat v4) rendered to markdown — markdown cells verbatim
               (preserves $...$ math), code cells as fenced blocks, outputs skipped (Colab
               noise: base64 images / HTML) — then sectioned exactly like markdown. This is
               the "prefer source formats over PDF exports" path (build step 6): the
               notebook's own LaTeX never suffers the export-to-PDF formula loss.

Section text is verbatim and *includes its own heading line*, mirroring pdf.py, so merged
small sections keep their absorbed headings visible.

YAML frontmatter (markdown only): a leading `---` block is parsed for `title` and
`date`/`created` with a deliberately minimal flat `key: value` parser — no PyYAML (§3
build-vs-buy; this is the only YAML the pipeline reads). `tags` is recognised but unused:
there is no extractor→tags path in the pipeline yet — wire it when one exists, don't build
half of one here.
"""

from __future__ import annotations

import json
import re
from datetime import date
from pathlib import Path

from locus.extract.base import ExtractedDoc, build_sections

# ATX heading: 1-6 #'s, a space, then the title. Anchored per line.
_ATX = re.compile(r"^(#{1,6})\s+(.+?)\s*#*\s*$")
# Code-fence delimiter (``` or ~~~, optionally followed by an info string).
_FENCE = re.compile(r"^(`{3,}|~{3,})")
# Flat frontmatter entry: `key: value`. Lists / nested mappings are ignored.
_FRONTMATTER_KV = re.compile(r"^([A-Za-z_][\w-]*)\s*:\s*(.*)$")
# A date at the start of a frontmatter value: 2024-05-17, 2024/05/17, or bare 2024.
_FM_DATE = re.compile(r"^(\d{4})(?:[-/](\d{1,2})(?:[-/](\d{1,2}))?)?\b")


def extract_markdown(path: str | Path) -> ExtractedDoc:
    """Extract a structured ExtractedDoc from the Markdown file at `path`."""
    path = Path(path)
    meta, body = _parse_frontmatter(_read_text(path))
    return _from_markdown(body, path, meta)


def extract_text(path: str | Path) -> ExtractedDoc:
    """Extract a structured ExtractedDoc from the plain-text file at `path`.

    No structure to detect: one untitled section, windowed if oversized. Title falls back
    to the filename stem (almost always a suspect slug, so the synthesis pass's arbitrated
    title takes over downstream).
    """
    path = Path(path)
    text = _read_text(path)
    if not text.strip():
        raise ValueError("no extractable text")
    sections = build_sections([(None, text)])
    return ExtractedDoc(
        title=path.stem,
        page_count=0,
        section_strategy="windowed" if len(sections) > 1 else "single",
        sections=sections,
        source_path=str(path),
        source_date=None,  # plain text has no embedded date; pipeline falls back to mtime
    )


def extract_notebook(path: str | Path) -> ExtractedDoc:
    """Extract a structured ExtractedDoc from the Jupyter notebook at `path`.

    Reads the .ipynb JSON directly (stdlib; nbformat v4 is documented and stable) rather
    than pulling in nbconvert's export machinery for what is a ~30-line transform.
    """
    path = Path(path)
    try:
        nb = json.loads(_read_text(path))
    except json.JSONDecodeError as exc:
        raise ValueError(f"not a valid notebook (bad JSON): {exc}") from exc
    body = _notebook_to_markdown(nb)
    return _from_markdown(body, path, meta={})


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8-sig", errors="replace")  # -sig strips a BOM


def _from_markdown(body: str, path: Path, meta: dict[str, str]) -> ExtractedDoc:
    """Shared markdown→ExtractedDoc assembly (markdown files and rendered notebooks)."""
    if not body.strip():
        raise ValueError("no extractable text")
    spans = _split_markdown(body)
    sections = build_sections(spans)
    if any(title is not None for title, _ in spans):
        strategy = "headings"
    elif len(sections) > 1:
        strategy = "windowed"
    else:
        strategy = "single"
    return ExtractedDoc(
        title=_resolve_title(meta, body, path),
        page_count=0,
        section_strategy=strategy,
        sections=sections,
        source_path=str(path),
        source_date=_parse_fm_date(meta.get("date") or meta.get("created")),
    )


def _parse_frontmatter(text: str) -> tuple[dict[str, str], str]:
    """Split a leading `---` YAML frontmatter block off `text`.

    Returns (meta, body). Only flat `key: value` lines are read (lowercased keys, quotes
    stripped); anything else inside the block is ignored. No closing `---` within the
    first 100 lines ⇒ not frontmatter, the whole text is body.
    """
    lines = text.splitlines(keepends=True)
    if not lines or lines[0].strip() != "---":
        return {}, text
    meta: dict[str, str] = {}
    for i, line in enumerate(lines[1:101], start=1):
        if line.strip() in ("---", "..."):
            return meta, "".join(lines[i + 1 :])
        m = _FRONTMATTER_KV.match(line.strip())
        if m:
            meta[m.group(1).lower()] = m.group(2).strip().strip("'\"")
    return {}, text


def _parse_fm_date(raw: str | None) -> str | None:
    """Frontmatter date value → ISO 'YYYY-MM-DD'. None if absent or not a real date."""
    if not raw:
        return None
    m = _FM_DATE.match(raw.strip())
    if not m:
        return None
    year, month, day = int(m.group(1)), int(m.group(2) or 1), int(m.group(3) or 1)
    try:
        return date(year, month, day).isoformat()
    except ValueError:
        return None


def _split_markdown(body: str) -> list[tuple[str | None, str]]:
    """Split markdown into (title, text) spans at ATX headings, fence-aware.

    Every heading level starts a span (base.build_sections merges tiny fragments back).
    Span text includes the heading line itself. Text before the first heading becomes an
    untitled leading span.
    """
    spans: list[tuple[str | None, str]] = []
    cur_title: str | None = None
    cur_lines: list[str] = []
    fence: str | None = None  # the delimiter that opened the current fence, if any

    for line in body.splitlines(keepends=True):
        f = _FENCE.match(line.strip())
        if f:
            if fence is None:
                fence = f.group(1)[0] * 3  # remember the char; close on the same kind
            elif f.group(1).startswith(fence[0]):
                fence = None
        heading = _ATX.match(line) if fence is None else None
        if heading:
            if cur_lines:
                spans.append((cur_title, "".join(cur_lines)))
            cur_title = heading.group(2)
            cur_lines = [line]
        else:
            cur_lines.append(line)
    if cur_lines:
        spans.append((cur_title, "".join(cur_lines)))
    return spans


def _resolve_title(meta: dict[str, str], body: str, path: Path) -> str | None:
    """Frontmatter title, else the first H1, else the filename stem."""
    if meta.get("title"):
        return meta["title"]
    fence = None
    for line in body.splitlines():
        f = _FENCE.match(line.strip())
        if f:
            fence = None if fence else "open"
            continue
        if fence is None:
            m = _ATX.match(line)
            if m and len(m.group(1)) == 1:
                return m.group(2)
    return path.stem


def _notebook_to_markdown(nb: dict) -> str:
    """Render nbformat-v4 cells to markdown: markdown cells verbatim, code cells fenced.

    Outputs are deliberately skipped — in Colab/Jupyter exports they are base64 images,
    HTML tables, and tracebacks that pollute retrieval; the *source* cells carry the
    knowledge (prose + $...$ math + code).
    """
    lang = (
        nb.get("metadata", {}).get("kernelspec", {}).get("language")
        or nb.get("metadata", {}).get("language_info", {}).get("name")
        or "python"
    )
    parts: list[str] = []
    for cell in nb.get("cells", []):
        source = cell.get("source", "")
        text = "".join(source) if isinstance(source, list) else source
        if not text.strip():
            continue
        if cell.get("cell_type") == "markdown":
            parts.append(text)
        elif cell.get("cell_type") == "code":
            parts.append(f"```{lang}\n{text}\n```")
    return "\n\n".join(parts)
