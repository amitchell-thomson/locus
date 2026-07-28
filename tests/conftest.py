"""Suite-wide test isolation from the owner's live `config.toml`.

`locus.config.load()` reads the REAL config.toml (process-cached), so any switch the owner flips
there silently changes what the test suite does. One switch does more than change behaviour — it
changes the BILLING channel:

    [ingest].pass_routing = { summarize = "haiku", ... }   # enabled live on 2026-07-28

With that set, every ingest test whose pass is not monkeypatched (the `concepts` pass on the
code-repo path is the live example) routes to the metered Anthropic SDK. On a machine with a key
in `.env` the suite quietly SPENDS MONEY on every run; on one without, 15 tests fail with
"ANTHROPIC_API_KEY is not set". Neither is a test result — CLAUDE.md §14 is explicit that tests
are model-free by default, and a suite whose cost depends on a config file the owner edits for
production reasons is not isolated.

This autouse fixture pins routing to `local` for the whole suite, restoring the pre-routing
behaviour (guarded local-Ollama paths where a model is unavoidable, fakes everywhere else). A
test that specifically exercises routing sets its own value — `test_ingest_routing.py` patches
`route_for`/`load` directly and is unaffected.
"""

from __future__ import annotations

import pytest

from locus import config


@pytest.fixture(autouse=True)
def _local_pass_routing(monkeypatch):
    """Force `[ingest].pass_routing` empty (= every pass local) for every test."""
    cfg = config.load()
    monkeypatch.setattr(cfg.ingest, "pass_routing", {}, raising=False)
    yield
