"""Structured local-LLM generation with schema validation and bounded repair (CLAUDE.md §6).

Every ingest pass returns data validated against a pydantic schema. We grammar-constrain the
model with `format=<json schema>` (so an 8B model emits schema-shaped JSON in the first place)
and, if validation still fails, run a bounded repair loop that shows the model its errors and
asks for a correction. After exhausting retries we raise IngestExtractionError, which the
pipeline catches to quarantine the single document without aborting the batch.
"""

from __future__ import annotations

from functools import lru_cache
from typing import TypeVar

from ollama import Client
from pydantic import BaseModel, ValidationError

from locus.config import load

T = TypeVar("T", bound=BaseModel)

# Shared system prompt: extraction must be faithful and JSON-only.
DEFAULT_SYSTEM = (
    "You extract structured metadata from technical documents for a knowledge base. "
    "Be faithful to the source text and never invent information not present in it. "
    "Output only JSON conforming to the provided schema — no prose, no code fences."
)


class IngestExtractionError(RuntimeError):
    """Raised when a pass cannot produce schema-valid output within the retry budget."""


@lru_cache(maxsize=1)
def _client() -> Client:
    return Client(host=load().ollama.host)


def ingest_model() -> str:
    return load().ollama.ingest_model


def _message_content(resp) -> str:
    msg = getattr(resp, "message", None)
    if msg is not None:
        return getattr(msg, "content", None) or msg["content"]
    return resp["message"]["content"]


def generate_structured(
    schema: type[T],
    user: str,
    *,
    system: str = DEFAULT_SYSTEM,
    client: Client | None = None,
    model: str | None = None,
    retries: int = 2,
    temperature: float = 0.0,
) -> T:
    """Generate JSON matching `schema`, repairing on validation failure up to `retries` times."""
    client = client or _client()
    ollama_cfg = load().ollama
    model = model or ollama_cfg.ingest_model
    json_schema = schema.model_json_schema()
    messages = [{"role": "system", "content": system}, {"role": "user", "content": user}]

    last_error: Exception | None = None
    for _attempt in range(retries + 1):
        try:
            resp = client.chat(
                model=model, messages=messages, format=json_schema,
                options={"temperature": temperature, "num_ctx": ollama_cfg.num_ctx},
            )
        except Exception as exc:  # network / server / model errors
            raise IngestExtractionError(f"Ollama chat failed (model={model}): {exc}") from exc

        content = _message_content(resp)
        try:
            return schema.model_validate_json(content)
        except ValidationError as exc:
            last_error = exc
            messages.append({"role": "assistant", "content": content})
            messages.append(
                {
                    "role": "user",
                    "content": (
                        "Your previous output failed validation with these errors:\n"
                        f"{exc}\n"
                        "Return ONLY a corrected JSON object satisfying the schema. No prose."
                    ),
                }
            )

    raise IngestExtractionError(
        f"{schema.__name__}: no schema-valid output from {model} after {retries + 1} attempts: "
        f"{last_error}"
    )
