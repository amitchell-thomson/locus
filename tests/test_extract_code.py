"""Step 10: repo extraction — eligible files, AST chunks/spans, call graph, entities.

Synthetic repos built in tmp_path; git-dependent tests are skipped when git is missing.
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest

from locus.extract.code import RepoSnapshot, collect_repo, extract_repo, repo_head

HAS_GIT = shutil.which("git") is not None
needs_git = pytest.mark.skipif(not HAS_GIT, reason="git not installed")

PY_MAIN = '''\
"""Module docstring."""

import math

CONSTANT = 17.3


def alpha(x):
    """Doubles x."""
    return beta(x) * 2


def beta(x):
    return math.sqrt(x)


class Engine:
    """A class with methods."""

    def run(self):
        return alpha(1)

    def stop(self):
        return self.run()
'''

PY_BROKEN = "def broken(:\n    pass\n"


def _make_tree(root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "main.py").write_text(PY_MAIN)
    (root / "broken.py").write_text(PY_BROKEN)
    (root / "README.md").write_text("# My Project\n\nIt computes things end to end.\n")
    (root / "uv.lock").write_text("locked")
    (root / "data.csv").write_text("a,b\n1,2\n")
    sub = root / "__pycache__"
    sub.mkdir()
    (sub / "main.cpython-311.pyc").write_text("x")
    return root


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", *args],
        check=True, capture_output=True,
    )


def _make_git_repo(root: Path) -> Path:
    _make_tree(root)
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "initial")
    return root


def test_walk_path_eligibility_without_git(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    snap = collect_repo(repo)
    assert snap.head is None
    assert snap.files == ["README.md", "broken.py", "main.py"]  # lock/csv/pycache excluded
    assert len(snap.content_hash) == 64  # manifest hash, not a commit sha
    # Deterministic: same tree -> same hash.
    assert collect_repo(repo).content_hash == snap.content_hash


def test_exclude_globs_drop_answer_key_files(tmp_path: Path, monkeypatch):
    # Round-5 audit: files carrying the labelled eval queries verbatim must be excludable
    # from repo ingest, or they outrank real content on their own questions.
    from locus.extract import code as code_mod

    repo = _make_tree(tmp_path / "plain")
    (repo / "eval").mkdir()
    (repo / "eval" / "fixtures.py").write_text("QUERIES = ['the exact eval question']\n")
    monkeypatch.setattr(code_mod, "_exclude_globs", lambda: ["eval/fixtures.py"])
    snap = collect_repo(repo)
    assert "eval/fixtures.py" not in snap.files
    assert "main.py" in snap.files  # everything else untouched


@needs_git
def test_git_path_uses_head_and_tracked_files(tmp_path: Path):
    repo = _make_git_repo(tmp_path / "gitrepo")
    snap = collect_repo(repo)
    assert snap.head == repo_head(repo)
    assert snap.content_hash == snap.head
    assert "main.py" in snap.files and "uv.lock" not in snap.files
    assert snap.source_date is not None  # last-commit date


@needs_git
def test_non_git_tree_nested_inside_a_git_repo_uses_walk(tmp_path: Path):
    """Regression (live 2026-06-05): `git rev-parse` walks UP, so a plain drop folder
    nested inside another repo resolved to the ENCLOSING repo's HEAD and ls-files (in the
    gitignored drop) returned nothing -> 'no eligible source files'. repo_head must require
    the directory to be its own work-tree root."""
    outer = _make_git_repo(tmp_path / "outer")
    drop = _make_tree(outer / "vault" / "incoming" / "projects" / "drop")
    assert repo_head(drop) is None  # NOT the outer repo's HEAD
    snap = collect_repo(drop)
    assert snap.head is None
    assert "main.py" in snap.files  # walk path found the files


def test_sections_and_def_granular_chunks(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    assert doc.page_count == 0 and doc.section_strategy == "repo"
    assert [s.title for s in doc.sections] == ["README.md", "broken.py", "main.py"]

    main = next(s for s in doc.sections if s.title == "main.py")
    assert main.file_path == "main.py"
    assert main.chunks is not None
    # Defs map to chunks with real line spans (alpha starts at its def line).
    src_lines = PY_MAIN.splitlines()
    alpha_def_line = src_lines.index("def alpha(x):") + 1
    alpha_chunk = next(c for c in main.chunks if c.text.startswith("def alpha"))
    assert alpha_chunk.line_start == alpha_def_line
    assert alpha_chunk.line_end > alpha_def_line
    assert alpha_chunk.file_path == "main.py"
    # Coverage: every line of the file falls inside some chunk span.
    covered = set()
    for c in main.chunks:
        covered.update(range(c.line_start, c.line_end + 1))
    nonblank = {i + 1 for i, line in enumerate(src_lines) if line.strip()}
    assert nonblank <= covered


def test_call_graph_and_entities(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    main = next(s for s in doc.sections if s.title == "main.py")
    assert "beta" in main.call_graph["alpha"]
    assert "alpha" in main.call_graph["Engine.run"]
    assert "self.run" in main.call_graph["Engine.stop"]
    assert "Engine" not in main.call_graph  # classes are not callables in the graph
    ents = {(e.name, e.type) for e in main.entities}
    assert ("alpha", "method") in ents
    assert ("Engine", "concept") in ents
    assert ("Engine.run", "method") in ents


def test_syntax_error_degrades_to_plain_text(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    broken = next(s for s in doc.sections if s.title == "broken.py")
    assert broken.chunks is None  # falls back to generic chunk_text downstream
    assert broken.call_graph is None
    assert "def broken" in broken.text


def test_readme_is_plain_text_section(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    readme = next(s for s in doc.sections if s.title == "README.md")
    assert readme.chunks is None and readme.call_graph is None
    assert "computes things" in readme.text


NB = json.dumps(
    {
        "cells": [
            {"cell_type": "markdown", "source": ["# Research\n", "\n", "We estimate $$\\beta = (X^TX)^{-1}X^Ty$$ via OLS.\n"]},
            {"cell_type": "code", "source": ["import numpy as np\n", "beta = np.linalg.lstsq(X, y)[0]\n"]},
            {"cell_type": "code", "source": [""], "outputs": [{"text": "noise"}]},  # empty source
        ],
        "metadata": {"language_info": {"name": "python"}},
        "nbformat": 4,
        "nbformat_minor": 5,
    }
)
# A notebook whose cells carry no source (outputs-only export) must render empty.
NB_EMPTY = json.dumps({"cells": [{"cell_type": "code", "source": [""], "outputs": [1, 2, 3]}], "nbformat": 4})


def test_notebook_in_repo_is_rendered_text_section(tmp_path: Path):
    repo = tmp_path / "research"
    repo.mkdir()
    (repo / "analysis.ipynb").write_text(NB)
    doc = extract_repo(repo)
    sec = next(s for s in doc.sections if s.title == "analysis.ipynb")
    assert sec.file_path == "analysis.ipynb"
    assert sec.chunks is None and sec.call_graph is None  # rides generic chunker, no AST
    assert "We estimate" in sec.text  # markdown cell verbatim
    assert "import numpy as np" in sec.text and "```python" in sec.text  # code cell fenced
    assert sec.has_math  # display math preserved from the markdown cell


def test_notebook_is_eligible_above_byte_cap(tmp_path: Path):
    """A >1 MB notebook (output-heavy) is eligible — the cap doesn't apply to .ipynb."""
    repo = tmp_path / "bignb"
    repo.mkdir()
    bloat = json.dumps({"cells": [{"cell_type": "code", "source": ["x = 1\n"], "outputs": ["z" * (1 << 21)]}], "nbformat": 4})
    (repo / "heavy.ipynb").write_text(bloat)
    snap = collect_repo(repo)
    assert "heavy.ipynb" in snap.files


def test_empty_notebook_renders_no_section(tmp_path: Path):
    repo = tmp_path / "emptynb"
    repo.mkdir()
    (repo / "blank.ipynb").write_text(NB_EMPTY)
    (repo / "main.py").write_text(PY_MAIN)  # keep the repo non-empty
    doc = extract_repo(repo)
    assert not any(s.title == "blank.ipynb" for s in doc.sections)


def test_oversized_def_line_splits_with_exact_subspans(tmp_path: Path):
    body = "\n".join(f"    x{i} = compute_value({i}) + compute_value({i + 1})" for i in range(400))
    (tmp_path / "big").mkdir()
    (tmp_path / "big" / "big.py").write_text(f"def huge():\n{body}\n    return x0\n")
    doc = extract_repo(tmp_path / "big")
    sec = doc.sections[0]
    assert len(sec.chunks) > 1  # split
    spans = [(c.line_start, c.line_end) for c in sec.chunks]
    # Sub-spans are contiguous and ordered.
    for (s1, e1), (s2, _) in zip(spans, spans[1:]):
        assert s2 == e1 + 1
    assert spans[0][0] == 1


def test_empty_repo_raises(tmp_path: Path):
    empty = tmp_path / "empty"
    empty.mkdir()
    (empty / "data.csv").write_text("a,b\n")
    with pytest.raises(ValueError, match="no eligible source files"):
        collect_repo(empty)


def test_repo_title_is_dir_name(tmp_path: Path):
    repo = _make_tree(tmp_path / "myproject")
    doc = extract_repo(repo)
    assert doc.title == "myproject"  # suspect (no spaces) -> synthesis arbitrates downstream


# --- round-3 evaluation: code-ingest noise filters -----------------------------------------


TRIVIAL_INIT = '''\
"""Package docstring."""

from .engine import Engine
from . import helpers

__all__ = ["Engine"]
__version__ = "1.0"
'''

REAL_INIT = '''\
"""Package with real logic in its __init__."""


def configure(level="INFO"):
    return {"level": level}
'''

TEST_FILE = '''\
import pytest


def helper_fixture():
    return 3


class TestMargin:
    def test_margin_warning_is_bool(self):
        assert True

    def test_k_equals_2(self):
        assert 2 == 2


def test_top_level_case():
    assert True
'''


def test_trivial_init_is_skipped(tmp_path: Path):
    root = tmp_path / "repo"
    _make_tree(root)
    pkg = root / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text(TRIVIAL_INIT)
    doc = extract_repo(root)
    paths = [s.file_path for s in doc.sections]
    assert "pkg/__init__.py" not in paths
    assert "main.py" in paths  # the rest of the repo is untouched
    # positions stay contiguous despite the skip
    assert [s.position for s in doc.sections] == list(range(len(doc.sections)))


def test_real_init_is_kept(tmp_path: Path):
    root = tmp_path / "repo"
    _make_tree(root)
    pkg = root / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text(REAL_INIT)
    doc = extract_repo(root)
    assert "pkg/__init__.py" in [s.file_path for s in doc.sections]


def test_test_methods_emit_no_entities(tmp_path: Path):
    root = tmp_path / "repo"
    _make_tree(root)
    (root / "test_margin.py").write_text(TEST_FILE)
    doc = extract_repo(root)
    sec = next(s for s in doc.sections if s.file_path == "test_margin.py")
    names = [e.name for e in (sec.entities or [])]
    # test_* defs (top-level and methods) are excluded; everything else stays.
    assert "helper_fixture" in names
    assert "TestMargin" in names  # the class itself is kept (one row, not 36)
    assert not any(n.split(".")[-1].startswith("test_") for n in names)
    # The test bodies remain retrievable as chunks.
    assert any("test_k_equals_2" in c.text for c in (sec.chunks or []))
