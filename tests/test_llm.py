"""Stage 4: the structured-generation repair loop (CLAUDE.md §6), tested without a live LLM.

A fake client returns canned responses so we can assert exactly how the repair loop behaves on
valid, recoverable-invalid, and unrecoverable output, plus transport errors.
"""

import pytest
from pydantic import BaseModel

from locus.ingest.llm import IngestExtractionError, generate_structured


class Foo(BaseModel):
    n: int


class FakeClient:
    def __init__(self, responses):
        self._responses = list(responses)
        self.calls = 0
        self.temperatures: list[float] = []

    def chat(self, **kwargs):
        self.calls += 1
        self.temperatures.append(kwargs.get("options", {}).get("temperature"))
        item = self._responses.pop(0)
        if isinstance(item, Exception):
            raise item
        return {"message": {"content": item}}


def test_valid_on_first_attempt():
    client = FakeClient(['{"n": 5}'])
    result = generate_structured(Foo, "u", client=client)
    assert result.n == 5
    assert client.calls == 1


def test_repairs_after_invalid_then_valid():
    client = FakeClient(["not json at all", '{"n": 7}'])
    result = generate_structured(Foo, "u", client=client, retries=2)
    assert result.n == 7
    assert client.calls == 2  # one bad, one repaired


def test_raises_after_exhausting_retries():
    client = FakeClient(["bad", "still bad", "nope"])
    with pytest.raises(IngestExtractionError):
        generate_structured(Foo, "u", client=client, retries=2)
    assert client.calls == 3  # initial + 2 repairs


def test_retries_escalate_temperature():
    """Repair attempts add entropy: a temperature-0 degeneration loop reproduces identically
    on every retry, so retrying at the same temperature cannot escape it (observed live)."""
    client = FakeClient(["bad", "still bad", '{"n": 3}'])
    result = generate_structured(Foo, "u", client=client, retries=2)
    assert result.n == 3
    assert client.temperatures == [0.0, 0.3, 0.6]


def test_transport_error_becomes_extraction_error():
    client = FakeClient([RuntimeError("connection refused")])
    with pytest.raises(IngestExtractionError):
        generate_structured(Foo, "u", client=client)
