"""Stage 0: config loads, validates, and resolves paths to absolute."""

from locus.config import Config, load


def test_config_loads_and_validates():
    cfg = load()
    assert cfg.embed.dim == 768  # locked to nomic-embed-text
    assert cfg.ollama.embed_model == "nomic-embed-text"
    assert "qwen2.5" in cfg.ollama.ingest_model  # qwen2.5 family (quant tag may vary)
    assert cfg.retrieve.proposition_top_k == 10  # propositions are first-class (decision A)


def test_paths_are_absolute():
    cfg = load()
    assert cfg.paths.db.is_absolute()
    assert cfg.paths.raw_store.is_absolute()


def test_api_key_reads_from_env(monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test-123")
    assert Config.anthropic_api_key() == "sk-test-123"


def test_api_key_missing_raises(monkeypatch):
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    try:
        Config.anthropic_api_key()
        raise AssertionError("expected RuntimeError")
    except RuntimeError as exc:
        assert "ANTHROPIC_API_KEY" in str(exc)
