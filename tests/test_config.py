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


def test_sections_and_keys_of_removed_features_are_ignored(tmp_path):
    """The server's config.toml still carries [alias], [obsidian], [discovery], [daily],
    [generation] and dead [mcp]/[retrieve]/[agent] keys after Slice C deleted what read them.
    Loading must ignore them, not refuse: a deploy must not need a hand edit on the server."""
    import tomllib
    from pathlib import Path

    example = Path(__file__).resolve().parents[1] / "config.example.toml"
    data = tomllib.loads(example.read_text())
    data["alias"] = {"use_llm": True, "block_threshold": 0.8}
    data["obsidian"] = {"out_dir": "data/obsidian"}
    data["discovery"] = {"paper_cap": 3, "arxiv_categories": ["q-fin.ST"]}
    data["daily"] = {"rule_gap_em": 1.8}
    data["generation"] = {"model": "claude-opus-4-8"}
    data["mcp"].update(include_figure_images=True, annotation_image_cap=4)
    data["retrieve"].update(rerank_top_k=8, multi_query_expansion=True, rough_penalty=0.0)
    data["agent"].update(connect_model="sonnet", connect_idea_projects=["oqts"])
    data["structure"].update(max_objects_per_doc=3, model="haiku")
    cfg = Config.model_validate(data)
    assert not hasattr(cfg, "alias") and not hasattr(cfg, "discovery")
    assert cfg.structure.belief_source_categories == ["note", "project", "career"]
