"""Structured local-LLM generation with schema validation and bounded repair (CLAUDE.md §6).

Every ingest pass returns data validated against a pydantic schema. We grammar-constrain the
model with `format=<json schema>` (so an 8B model emits schema-shaped JSON in the first place)
and, if validation still fails, run a bounded repair loop that shows the model its errors and
asks for a correction. After exhausting retries we raise IngestExtractionError, which the
pipeline catches to quarantine the single document without aborting the batch.
"""

from __future__ import annotations

import time
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


def fit_to_window(text: str, *, reserve_tokens: int = 600) -> str:
    """Truncate pass input so prompt + generation always fit the configured num_ctx.

    Ollama keeps the LAST num_ctx tokens: when prompt + output exceed the window, the
    HEAD — the source text — silently slides out mid-generation and the model "summarises"
    from nothing (round-5 audit: a 12k-token section in an 8192 window produced a fluent
    summary of a non-existent 'image compression' paper, and the lexical grounding guard
    cannot reliably catch topic-coherent fabrications). Bounding the input is the root-cause
    fix: the model always sees (the head of) its source.

    `reserve_tokens` covers the prompt scaffold/instructions around the text; _NUM_PREDICT
    is the output budget. ~4 chars/token is conservative for English prose and code.
    """
    budget_tokens = load().ollama.num_ctx - _NUM_PREDICT - reserve_tokens
    budget_chars = max(4_000, budget_tokens * 4)
    if len(text) <= budget_chars:
        return text
    return text[:budget_chars] + "\n[... source truncated to fit the model context window]"


@lru_cache(maxsize=1)
def _client() -> Client:
    return Client(host=load().ollama.host)


def ingest_model() -> str:
    return load().ollama.ingest_model


def unload(model: str | None = None, settle_seconds: float = 15.0) -> bool:
    """Best-effort evict `model` (default: the ingest model) from Ollama if it is resident.

    Used to choreograph the 8 GB card between Ollama and the math-OCR engine / figures
    VLM: evict before the next consumer takes the GPU (else its allocation OOMs or — for
    an Ollama-to-Ollama handoff — the incoming model is PLANNED with a kept CPU/GPU split
    against the outgoing model's teardown). Eviction is CONFIRMED, not just issued: the
    keep_alive=0 request returns before the runner's VRAM is actually freed, and a model
    loaded inside that window splits (observed live 2026-06-06, third recurrence of the
    same race — text-model teardown vs figures-VLM load). Polls ps() until the model is
    gone or `settle_seconds`, then a short grace for the runner process to exit. Returns
    True if an unload was issued. Never raises — VRAM choreography, not correctness.
    """
    try:
        client = _client()
        model = model or ingest_model()
        if not any(m.model == model for m in client.ps().models or []):
            return False
        # keep_alive=0 with an empty prompt unloads without generating.
        client.generate(model=model, prompt="", keep_alive=0)
        deadline = time.monotonic() + settle_seconds
        while time.monotonic() < deadline:
            if not any(m.model == model for m in client.ps().models or []):
                break
            time.sleep(1.0)
        time.sleep(2.0)  # grace for the runner process to actually release VRAM
        return True
    except Exception:  # pragma: no cover - best-effort; ingest must not care
        pass
    return False


def unload_all(settle_seconds: float = 30.0) -> int:
    """Best-effort evict EVERY resident Ollama model; returns how many were unloaded.

    The OCR engine needs the whole card, and which model is resident depends on the
    previous document's last pass — after step 11 that is usually the figures VLM, not
    the ingest model, so a named unload misses it (2026-06-06 external audit: the GOT
    pass OOM'd on every doc that followed a figure-bearing one — 255 pages across 14
    docs fell back un-OCR'd). Enumerating ps() is robust to whatever model ran last.

    Eviction is then CONFIRMED, not just issued: Ollama removes the model from ps()
    before the runner's VRAM teardown finishes, and a consumer that allocates
    immediately races it (recovery batch, same day: 31 pages re-OOM'd mid-document
    against a lingering 4.4 GiB runner). Polls ps() until empty or `settle_seconds`.
    """
    count = 0
    try:
        client = _client()
        for m in client.ps().models or []:
            try:
                client.generate(model=m.model, prompt="", keep_alive=0)
                count += 1
            except Exception:  # pragma: no cover - keep evicting the rest
                pass
        if count:
            deadline = time.monotonic() + settle_seconds
            while time.monotonic() < deadline:
                if not (client.ps().models or []):
                    break
                time.sleep(1.0)
            time.sleep(2.0)  # grace for the runner process to actually exit
    except Exception:  # pragma: no cover - best-effort; ingest must not care
        pass
    return count


def unload_if_split(model: str | None = None) -> bool:
    """Unload any resident model that is split CPU/GPU (or just `model` when given).

    Ollama plans a model's CPU/GPU split at load time and KEEPS it for as long as the model
    stays resident — a model loaded while the math-OCR engine held VRAM (or while another
    model was still being evicted) stays half-on-CPU even after that VRAM is freed, and
    generation runs several-fold slower (observed live twice: the ingest model after GOT,
    and the figures VLM after the text-model handoff — 2026-06-06, 74/26 split through a
    50-figure batch). Checks ALL resident models by default so no phase's model escapes
    the check. Returns True if any unload was issued.
    """
    issued = False
    try:
        client = _client()
        for m in client.ps().models or []:
            if model is not None and m.model != model:
                continue
            if (m.size_vram or 0) < (m.size or 0):
                issued = unload(m.model) or issued
    except Exception:  # pragma: no cover - best-effort; ingest must not care
        pass
    return issued


def vision_chat(
    prompt: str,
    image_bytes: bytes,
    *,
    model: str,
    client: Client | None = None,
    temperature: float = 0.0,
    num_ctx: int = 8192,
    num_predict: int = -1,  # uncapped by default: page OCR legitimately runs long
) -> str:
    """One Ollama chat call with an attached image; returns the raw text reply.

    Shared by the math-OCR 'qwen' engine (extract/mathocr.py) and the figure-description
    pass (ingest/figures.py). Raises IngestExtractionError on network/server/model errors —
    callers decide whether that quarantines, falls back, or skips (per-unit graceful
    degradation, §6).
    """
    client = client or _client()
    try:
        resp = client.chat(
            model=model,
            messages=[{"role": "user", "content": prompt, "images": [image_bytes]}],
            options={
                "temperature": temperature,
                "num_ctx": num_ctx,
                "num_predict": num_predict,
            },
        )
    except Exception as exc:
        raise IngestExtractionError(f"Ollama vision chat failed (model={model}): {exc}") from exc
    return _message_content(resp)


def _message_content(resp) -> str:
    msg = getattr(resp, "message", None)
    if msg is not None:
        return getattr(msg, "content", None) or msg["content"]
    return resp["message"]["content"]


def _hit_length_cap(resp) -> bool:
    """True when generation stopped at num_predict (Ollama done_reason 'length').

    A length-capped response is truncated mid-JSON; the repair must demand a SHORTER
    answer, not a 'corrected' one — the model otherwise rebuilds the same overlong output
    and truncates at the same place every attempt (observed live: a math-dense section
    whose summary transcribed equations until the cap, identically across runs).
    """
    reason = getattr(resp, "done_reason", None)
    if reason is None and isinstance(resp, dict):
        reason = resp.get("done_reason")
    return reason == "length"


def _sanitize_latex_escapes(raw: str) -> str:
    """Repair unescaped LaTeX backslashes inside JSON string values before parsing.

    The local model writes LaTeX commands into JSON strings without doubling the backslash
    (the 2026-06-05 evaluation's headline corruption). The JSON parser then either silently
    corrupts the value — `\\t \\n \\f \\r \\b` are valid escapes, so `\\tau` becomes TAB+`au`,
    `\\frac` becomes FF+`rac` — or rejects it (`\\mu`, `\\sigma` are invalid escapes), which the
    repair loop cannot fix because the model keeps emitting the same bytes.

    Single left-to-right scan, only inside string literals:
      - `\\\\`           : already-escaped backslash — copy as a unit (protects correct LaTeX,
                         Windows paths, and stops the scanner re-reading the second `\\`);
      - `\\u`, `\\"`, `\\/`: valid, unambiguous JSON escapes — copy verbatim;
      - `\\t \\n \\f \\r \\b` followed by an ASCII letter: LaTeX command prefix (`\\tau`, `\\nu`,
                         `\\frac`, `\\rho`, `\\beta`) — double the backslash. A *genuine* control
                         escape mid-word ("col1\\tcol2") would be mis-rewritten, but generated
                         summaries/propositions never legitimately contain that; the corrupted
                         alternative is strictly worse. Followed by a non-letter: genuine —
                         copy verbatim;
      - any other escape letter: invalid JSON (every other LaTeX command) — double it.

    Never deletes a byte; worst case is a stray literal backslash in the parsed text.
    """
    out: list[str] = []
    in_string = False
    i, n = 0, len(raw)
    while i < n:
        ch = raw[i]
        if ch != "\\" or not in_string:
            if ch == '"':
                in_string = not in_string
            out.append(ch)
            i += 1
            continue
        nxt = raw[i + 1] if i + 1 < n else ""
        if nxt == "\\":
            out.append("\\\\")
            i += 2
        elif nxt == "u":
            out.append(raw[i : i + 6])  # \uXXXX (truncation, if any, fails validation later)
            i += 6
        elif nxt in '"/':
            out.append("\\" + nxt)
            i += 2
        elif nxt in "tnfrb":
            after = raw[i + 2] if i + 2 < n else ""
            latex = after.isascii() and after.isalpha()
            out.append(("\\\\" if latex else "\\") + nxt)
            i += 2
        else:  # invalid JSON escape — can only be a bare LaTeX/literal backslash
            out.append("\\\\")
            i += 1
    return "".join(out)


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
            # Sanitize unescaped LaTeX backslashes first (see _sanitize_latex_escapes) —
            # inside the loop, so repair attempts get the same protection.
            return schema.model_validate_json(_sanitize_latex_escapes(content))
        except ValidationError as exc:
            last_error = exc
            # Echo at most 1500 chars of the failed output back. A degenerated output
            # (runaway repetition loop) echoed in full PRIMES the model to continue the
            # same loop on the repair attempt — the repair needs the error, not 8k chars
            # of the attractor it must escape.
            echo = content[:1500] + (" …[truncated]" if len(content) > 1500 else "")
            messages.append({"role": "assistant", "content": echo})
            if _hit_length_cap(resp):
                complaint = (
                    "Your previous output hit the generation length limit and was cut off "
                    "mid-JSON. Return a COMPLETE JSON object that is much shorter: keep every "
                    "string field concise, summarize instead of transcribing, and never copy "
                    "equations or LaTeX runs into string values."
                )
            else:
                complaint = (
                    "Your previous output failed validation with these errors:\n"
                    f"{exc}\n"
                    "Return ONLY a corrected JSON object satisfying the schema. No prose."
                )
            messages.append({"role": "user", "content": complaint})

    raise IngestExtractionError(
        f"{schema.__name__}: no schema-valid output from {model} after {retries + 1} attempts: "
        f"{last_error}"
    )
