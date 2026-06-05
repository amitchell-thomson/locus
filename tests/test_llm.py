"""Stage 4: the structured-generation repair loop (CLAUDE.md §6), tested without a live LLM.

A fake client returns canned responses so we can assert exactly how the repair loop behaves on
valid, recoverable-invalid, and unrecoverable output, plus transport errors.
"""

import pytest
from pydantic import BaseModel

from locus.ingest.llm import IngestExtractionError, _sanitize_latex_escapes, generate_structured


class Foo(BaseModel):
    n: int


class Text(BaseModel):
    s: str


class FakeClient:
    def __init__(self, responses):
        self._responses = list(responses)
        self.calls = 0
        self.temperatures: list[float] = []
        self.messages_seen: list[list[dict]] = []

    def chat(self, **kwargs):
        self.calls += 1
        self.temperatures.append(kwargs.get("options", {}).get("temperature"))
        self.messages_seen.append([dict(m) for m in kwargs.get("messages", [])])
        item = self._responses.pop(0)
        if isinstance(item, Exception):
            raise item
        if isinstance(item, tuple):  # (content, done_reason)
            content, reason = item
            return {"message": {"content": content}, "done_reason": reason}
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


def test_length_truncated_output_gets_a_shorter_repair_demand():
    # An unterminated string (output cut at num_predict) must produce a "be shorter" repair
    # instruction, not a generic validation echo — the 2A2A quarantine: the model rebuilt the
    # same overlong summary on every attempt because nothing told it length was the problem.
    truncated = ('{"n": "the summary goes on and on \\\\rightarrow', "length")
    client = FakeClient([truncated, '{"n": 7}'])
    result = generate_structured(Foo, "u", client=client, retries=2)
    assert result.n == 7
    repair_turn = client.messages_seen[1][-1]["content"]
    assert "length limit" in repair_turn and "shorter" in repair_turn
    assert "failed validation" not in repair_turn


def test_transport_error_becomes_extraction_error():
    client = FakeClient([RuntimeError("connection refused")])
    with pytest.raises(IngestExtractionError):
        generate_structured(Foo, "u", client=client)


# --- LaTeX/JSON escape sanitizer (the 2026-06-05 evaluation's headline corruption) ----------


def _roundtrip(value_bytes: str) -> str:
    """Parse a raw JSON document whose string value contains `value_bytes` as written by the
    model (i.e. single backslashes), through the sanitizer."""
    return Text.model_validate_json(_sanitize_latex_escapes('{"s": "' + value_bytes + '"}')).s


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        # Valid-escape-letter LaTeX: silently corrupted before the fix (TAB+au, LF+u, FF+rac).
        (r"\tau decay", r"\tau decay"),
        (r"\theta_c is the threshold", r"\theta_c is the threshold"),
        (r"the \nu parameter", r"the \nu parameter"),
        (r"\frac{K}{s-2}", r"\frac{K}{s-2}"),
        (r"\beta-convergence", r"\beta-convergence"),
        (r"\text{percent}", r"\text{percent}"),
        # Invalid-escape LaTeX: a hard ValidationError before the fix.
        (r"\mu and \sigma bounds", r"\mu and \sigma bounds"),
        (r"\lambda \partial \alpha", r"\lambda \partial \alpha"),
        # Already-correct double escapes parse to one literal backslash, untouched.
        (r"\\tau decay", r"\tau decay"),
        (r"C:\\path\\to", r"C:\path\to"),
        # Genuine JSON escapes survive: LF before a non-letter, tab at end, quote, unicode.
        (r"line1\n line2", "line1\n line2"),
        (r"ends with tab\t", "ends with tab\t"),
        (r"a \"quote\" here", 'a "quote" here'),
        (r"café", "café"),
        # Mixed: genuine \n (before a digit) and LaTeX \nu in one string.
        (r"x = 3\n4 and \nu = 2", "x = 3\n4 and \\nu = 2"),
    ],
)
def test_sanitizer_roundtrips(raw, expected):
    parsed = _roundtrip(raw)
    assert parsed == expected
    # The corruption signature never survives: no control chars beyond real \n \t \r.
    assert not any(ord(c) < 32 and c not in "\n\t\r" for c in parsed)


def test_sanitizer_leaves_structure_alone():
    # Structural JSON (keys, braces, separators) and non-string content are untouched.
    raw = '{"a": 1, "b": [true, null], "c": "plain"}'
    assert _sanitize_latex_escapes(raw) == raw


def test_generate_structured_parses_latex_bearing_output():
    # End-to-end: the model emits unescaped LaTeX; the first attempt now succeeds.
    client = FakeClient(['{"s": "the time constant \\tau is given by \\frac{1}{\\omega_0}"}'])
    result = generate_structured(Text, "u", client=client)
    assert result.s == r"the time constant \tau is given by \frac{1}{\omega_0}"
    assert client.calls == 1  # no repair round was needed
