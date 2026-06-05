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


# Repair attempts run at increasing temperature. At temperature 0 a model that degenerates
# (observed live: a runaway LaTeX echo loop inside a JSON string, on a math-heavy section)
# reproduces the SAME degeneration on every retry — the repair loop needs entropy to escape
# the attractor, not just the error message.
_RETRY_TEMPERATURES = (0.3, 0.6)
# Cap generated tokens: no ingest pass legitimately needs more, and a runaway generation
# otherwise burns the whole context window before failing validation.
_NUM_PREDICT = 2048


@lru_cache(maxsize=1)
def _client() -> Client:
    return Client(host=load().ollama.host)


def ingest_model() -> str:
    return load().ollama.ingest_model


def unload(model: str | None = None) -> bool:
    """Best-effort evict `model` (default: the ingest model) from Ollama if it is resident.

    Used to choreograph the 8 GB card between Ollama and the math-OCR engine: evict before
    the OCR engine takes the GPU (else its allocation OOMs against a resident 6 GB LLM and
    every page silently falls back un-OCR'd), and evict a CPU/GPU-split residue afterwards
    (see unload_if_split). Returns True if an unload was issued. Never raises — this is
    VRAM choreography, not a correctness requirement.
    """
    try:
        client = _client()
        model = model or ingest_model()
        if any(m.model == model for m in client.ps().models or []):
            # keep_alive=0 with an empty prompt unloads without generating.
            client.generate(model=model, prompt="", keep_alive=0)
            return True
    except Exception:  # pragma: no cover - best-effort; ingest must not care
        pass
    return False


def unload_if_split(model: str | None = None) -> bool:
    """Unload `model` only if it is resident but split CPU/GPU.

    Ollama plans a model's CPU/GPU split at load time and KEEPS it for as long as the model
    stays resident — a model loaded while the math-OCR engine held VRAM stays half-on-CPU
    even after that VRAM is freed, and generation runs several-fold slower (observed live).
    Called between a document's OCR phase and its LLM passes (no request in flight), so the
    next pass reloads it fully on the GPU. Returns True if an unload was issued.
    """
    try:
        client = _client()
        model = model or ingest_model()
        for m in client.ps().models or []:
            if m.model == model and (m.size_vram or 0) < (m.size or 0):
                return unload(model)
    except Exception:  # pragma: no cover - best-effort; ingest must not care
        pass
    return False


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
    for attempt in range(retries + 1):
        temp = (
            temperature
            if attempt == 0
            else _RETRY_TEMPERATURES[min(attempt - 1, len(_RETRY_TEMPERATURES) - 1)]
        )
        try:
            resp = client.chat(
                model=model, messages=messages, format=json_schema,
                options={
                    "temperature": temp,
                    "num_ctx": ollama_cfg.num_ctx,
                    "num_predict": _NUM_PREDICT,
                },
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
