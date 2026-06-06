"""Step 11: the figure-description pass — QC predicates, bounded retry, index text.

A fake Ollama client (test_llm.py pattern) drives describe_figure without a live VLM; the
QC predicates are pure functions. The live VLM was validated separately against the real
corpus during development.
"""

from locus.ingest.figures import describe_figure, index_text, rejection_reason

GOOD = (
    "A closed-loop block diagram where the reference r(t) enters a summing junction, "
    "feeds controller D, then plant G, with sensor feedback to the junction."
)


class FakeVisionClient:
    def __init__(self, responses):
        self._responses = list(responses)
        self.calls = 0
        self.prompts: list[str] = []

    def chat(self, **kwargs):
        self.calls += 1
        self.prompts.append(kwargs["messages"][0]["content"])
        item = self._responses.pop(0)
        if isinstance(item, Exception):
            raise item
        return {"message": {"content": item}}


def test_rejection_reasons():
    assert rejection_reason("") == "empty"
    assert rejection_reason("A nice diagram.") == "too-short"
    assert rejection_reason("I cannot see any image attached to this message, sorry.") == "refusal"
    assert rejection_reason("the loop goes " * 60) == "repetition-loop"
    assert rejection_reason(GOOD) is None


def test_describe_figure_first_attempt(monkeypatch):
    client = FakeVisionClient([GOOD])
    out = describe_figure(b"png", "Figure 1: loop", client=client, model="fake-vlm")
    assert out == GOOD
    assert client.calls == 1
    assert "Figure 1: loop" in client.prompts[0]  # caption given as context


def test_describe_figure_retries_then_succeeds():
    client = FakeVisionClient(["Too short.", GOOD])
    out = describe_figure(b"png", None, client=client, model="fake-vlm")
    assert out == GOOD
    assert client.calls == 2
    assert "rejected (too-short)" in client.prompts[1]  # repair names the reason


def test_describe_figure_gives_up_after_retry():
    client = FakeVisionClient(["Too short.", "Still short."])
    assert describe_figure(b"png", None, client=client, model="fake-vlm") is None
    assert client.calls == 2


def test_describe_figure_none_on_transport_error():
    client = FakeVisionClient([RuntimeError("connection refused")])
    assert describe_figure(b"png", None, client=client, model="fake-vlm") is None


def test_index_text_composition():
    assert index_text("Figure 1: loop", "A diagram.") == "Figure 1: loop\nA diagram."
    assert index_text("Figure 1: loop", None) == "Figure 1: loop"
    assert index_text(None, "A diagram.") == "A diagram."
    assert index_text(None, None) == ""
    assert index_text("  ", "\n") == ""
