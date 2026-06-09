"""Step 8: Markdown / plain-text / notebook extraction — sectioning, frontmatter, windows.

Synthetic files written to tmp_path so the tests are deterministic and committable.
"""

import json
from pathlib import Path

import pytest

from locus.extract.base import MAX_SECTION_CHARS, MIN_SECTION_CHARS, has_math
from locus.extract.textdoc import extract_markdown, extract_notebook, extract_text

# A body paragraph comfortably above MIN_SECTION_CHARS so sections survive the small-merge.
BODY = " ".join(f"Sentence {i} contains several descriptive words about the topic." for i in range(12))
assert len(BODY) > MIN_SECTION_CHARS


def _write(path: Path, text: str) -> Path:
    path.write_text(text, encoding="utf-8")
    return path


# --- markdown ------------------------------------------------------------------------------


def test_atx_headings_split_sections(tmp_path: Path):
    md = _write(
        tmp_path / "doc.md",
        f"# Alpha Title\n\n{BODY}\n\n## Beta Section\n\n{BODY}\n\n## Gamma Section\n\n{BODY}\n",
    )
    doc = extract_markdown(md)
    assert doc.section_strategy == "headings"
    assert doc.page_count == 0
    titles = [s.title for s in doc.sections]
    assert titles == ["Alpha Title", "Beta Section", "Gamma Section"]
    beta = doc.sections[1]
    assert beta.text.startswith("## Beta Section")  # heading line kept in verbatim text
    assert beta.page_start == 1 and beta.page_end == 1
    assert doc.title == "Alpha Title"  # first H1 wins absent frontmatter


def test_hash_inside_code_fence_is_not_a_heading(tmp_path: Path):
    md = _write(
        tmp_path / "code.md",
        f"# Real Heading\n\n{BODY}\n\n```python\n# just a comment\n#!shebang-ish\nprint(1)\n```\n\n{BODY}\n",
    )
    doc = extract_markdown(md)
    assert [s.title for s in doc.sections] == ["Real Heading"]
    assert "# just a comment" in doc.sections[0].text  # fence content kept verbatim


def test_setext_underlines_are_ignored(tmp_path: Path):
    md = _write(tmp_path / "setext.md", f"Alpha\n=====\n\n{BODY}\n\nBeta\n----\n\n{BODY}\n")
    doc = extract_markdown(md)
    assert all(s.title is None for s in doc.sections)
    assert doc.section_strategy == "single"


def test_frontmatter_title_and_date(tmp_path: Path):
    md = _write(
        tmp_path / "note.md",
        f"---\ntitle: Kalman Filter Field Notes\ndate: 2023-11-04\ntags: [control, estimation]\n---\n\n{BODY}\n",
    )
    doc = extract_markdown(md)
    assert doc.title == "Kalman Filter Field Notes"
    assert doc.source_date == "2023-11-04"
    assert "---" not in doc.sections[0].text  # frontmatter excised from the body
    assert "tags" not in doc.sections[0].text


def test_unclosed_frontmatter_is_body(tmp_path: Path):
    md = _write(tmp_path / "dashes.md", f"---\n{BODY}\n")
    doc = extract_markdown(md)
    assert doc.title == "dashes"  # filename stem fallback
    assert doc.source_date is None


def test_tiny_note_ingests_as_one_section(tmp_path: Path):
    md = _write(tmp_path / "tiny.md", "Remember: the laplace table is in the green folder.\n")
    doc = extract_markdown(md)
    assert len(doc.sections) == 1
    assert doc.section_strategy == "single"
    assert "green folder" in doc.sections[0].text


def test_empty_markdown_raises(tmp_path: Path):
    md = _write(tmp_path / "empty.md", "  \n\n  ")
    with pytest.raises(ValueError, match="no extractable text"):
        extract_markdown(md)


def test_math_markup_sets_has_math(tmp_path: Path):
    md = _write(
        tmp_path / "math.md",
        f"# Math Section\n\n{BODY}\n\n$$ H(s) = \\frac{{1}}{{s + 1}} $$\n\n"
        f"# Prose Section\n\n{BODY}\n",
    )
    doc = extract_markdown(md)
    math_sec = next(s for s in doc.sections if s.title == "Math Section")
    prose_sec = next(s for s in doc.sections if s.title == "Prose Section")
    assert math_sec.has_math
    assert not prose_sec.has_math
    assert has_math("\\begin{align} x &= 1 \\end{align}")


# --- plain text ----------------------------------------------------------------------------


def test_long_text_windows_under_max(tmp_path: Path):
    paragraphs = "\n\n".join(BODY for _ in range(40))  # well over MAX_SECTION_CHARS
    txt = _write(tmp_path / "long.txt", paragraphs)
    doc = extract_text(txt)
    assert doc.section_strategy == "windowed"
    assert len(doc.sections) > 1
    assert all(len(s.text) <= MAX_SECTION_CHARS for s in doc.sections)
    assert doc.sections[0].title == "Section (part 1)"
    # Nothing lost: total body content survives the windowing.
    rejoined = "".join(s.text for s in doc.sections)
    assert rejoined.count("Sentence 0") == 40


def test_short_text_single_section(tmp_path: Path):
    txt = _write(tmp_path / "short.txt", BODY)
    doc = extract_text(txt)
    assert doc.section_strategy == "single"
    assert len(doc.sections) == 1
    assert doc.sections[0].title is None
    assert doc.title == "short"  # stem; suspect, so synthesis arbitration applies downstream


def test_empty_text_raises(tmp_path: Path):
    txt = _write(tmp_path / "empty.txt", "")
    with pytest.raises(ValueError, match="no extractable text"):
        extract_text(txt)


# --- notebook ------------------------------------------------------------------------------


def _make_nb(cells: list[dict]) -> dict:
    return {
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {"kernelspec": {"language": "python"}},
        "cells": cells,
    }


def test_notebook_markdown_and_code_cells(tmp_path: Path):
    nb = _make_nb(
        [
            {"cell_type": "markdown", "source": [f"# Fourier Notebook\n", "\n", f"{BODY}\n"]},
            {
                "cell_type": "markdown",
                "source": ["The transform is $X(\\omega) = \\int x(t) e^{-i\\omega t} dt$ "
                           "and $$ \\hat{f} $$\n"],
            },
            {
                "cell_type": "code",
                "source": ["# fft demo\n", "import numpy as np\n"],
                "outputs": [{"output_type": "display_data", "data": {"image/png": "AAAA"}}],
            },
            {"cell_type": "markdown", "source": [f"## Results\n\n{BODY}\n"]},
        ]
    )
    path = _write(tmp_path / "fourier.ipynb", json.dumps(nb))
    doc = extract_notebook(path)
    assert doc.title == "Fourier Notebook"
    assert doc.section_strategy == "headings"
    joined = "\n".join(s.text for s in doc.sections)
    assert "$X(\\omega)" in joined  # $...$ math preserved verbatim
    assert "import numpy as np" in joined  # code cell present, fenced
    assert "```python" in joined
    assert "AAAA" not in joined  # outputs dropped
    assert any(s.has_math for s in doc.sections)
    # the '# fft demo' comment inside the fenced code cell did not become a heading
    assert all((s.title or "").strip() != "fft demo" for s in doc.sections)


def test_notebook_bad_json_raises(tmp_path: Path):
    path = _write(tmp_path / "broken.ipynb", "{not json")
    with pytest.raises(ValueError, match="bad JSON"):
        extract_notebook(path)


def test_data_dump_word_list_is_rejected(tmp_path: Path):
    """A single-column word/id list quarantines rather than being confabulated into a
    document (the words_alpha.txt -> 'network security' hallucination, 2026-06-09 audit)."""
    word_list = "\n".join(f"word{i:05d}" for i in range(300))  # 300 single-token lines
    path = _write(tmp_path / "words_alpha.txt", word_list)
    with pytest.raises(ValueError, match="non-prose data file"):
        extract_text(path)


def test_short_list_and_prose_are_not_rejected(tmp_path: Path):
    # Below the line floor: a short list is harmless and ingests.
    extract_text(_write(tmp_path / "short.txt", "\n".join(f"item{i}" for i in range(40)) + "\n" + BODY))
    # Real prose (multi-token lines) ingests even when long.
    prose = "\n".join("This sentence has several words on its own line." for _ in range(300))
    doc = extract_text(_write(tmp_path / "prose.txt", prose))
    assert doc.sections
