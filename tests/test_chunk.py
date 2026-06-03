"""Stage 3: chunker — token-budget guarantee and boundary-respecting splits."""

from locus.ingest.chunk import chunk_text, count_tokens


def test_empty_text_yields_no_chunks():
    assert chunk_text("") == []
    assert chunk_text("   \n\n  ") == []


def test_short_text_is_one_chunk():
    chunks = chunk_text("A single short paragraph of text.", max_tokens=512)
    assert chunks == ["A single short paragraph of text."]


def test_respects_token_budget():
    text = "\n\n".join(f"Paragraph number {i} with several words in it." for i in range(40))
    chunks = chunk_text(text, max_tokens=50)
    assert len(chunks) > 1
    assert all(count_tokens(c) <= 50 for c in chunks)


def test_unbroken_run_is_hard_split_within_budget():
    # No whitespace/sentence boundaries: must fall through to the token-window hard split.
    text = "x" * 4000
    chunks = chunk_text(text, max_tokens=50)
    assert len(chunks) > 1
    assert all(count_tokens(c) <= 50 for c in chunks)


def test_content_preserved_on_boundary_split():
    paras = [f"Unique marker {i} sits in paragraph {i} here." for i in range(10)]
    chunks = chunk_text("\n\n".join(paras), max_tokens=30)
    joined = "\n".join(chunks)
    for i in range(10):
        assert f"Unique marker {i}" in joined


def test_max_tokens_override_is_honoured():
    text = "\n\n".join(f"Sentence {i} content." for i in range(60))
    few = chunk_text(text, max_tokens=200)
    many = chunk_text(text, max_tokens=20)
    assert len(many) > len(few)
