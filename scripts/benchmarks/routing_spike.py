"""Phase-0 routing spike (THROWAWAY): per-pass quality + cost, local qwen vs Haiku vs Sonnet.

Answers the §7 go/no-go: for the DURABLE passes (summaries, propositions, entities), is a cheap
Claude model (Haiku) a clear step up on the local 8B, and is Sonnet worth it anywhere? Plus: what
does bulk re-ingest actually cost?

Design (matches the owner's Phase-0 decision "via claude -p, no key"):
- QUALITY is model-dependent: we run the REAL pass functions (same prompts, schemas, and
  post-filters) against 3 representative live sections, once per engine, and grade every output
  with a FIXED strong grader (Sonnet) reusing locus.eval.judge's rubric. Haiku/Sonnet run by
  monkeypatching each pass module's `generate_structured` with a `claude -p` shim, so there is
  ZERO prompt drift from the shipped pipeline.
- COST is channel-dependent: the `claude -p` envelope reports `total_cost_usd` (inflated by the
  ~17k-token Claude Code harness prompt, cached) AND `usage.input_tokens/output_tokens` (the
  genuinely-uncached pass tokens ≈ the real per-pass I/O). We report the claude -p wall cost
  (the "ongoing daily" channel) separately from a Batch extrapolation priced on the raw token
  counts (the bulk channel §7 actually picks).

Run: uv run python scripts/benchmarks/routing_spike.py
Writes a JSON record of every call + a summary table to scripts/benchmarks/routing_spike_out.json
"""

from __future__ import annotations

import json
import subprocess
import sqlite3
import sys
import tempfile
import time
from dataclasses import dataclass, field, asdict
from pathlib import Path

from pydantic import BaseModel

from locus.config import load
from locus.ingest import propositions as prop_mod
from locus.ingest import entities as ent_mod
from locus.ingest import summarize as sum_mod
from locus.ingest.llm import DEFAULT_SYSTEM
from locus.eval import judge as judge_mod

# The three claude model ids we compare (Haiku default, Sonnet the escalation candidate).
HAIKU = "claude-haiku-4-5-20251001"
SONNET = "claude-sonnet-5"
GRADER = SONNET  # one fixed strong grader for all engines, so scores are comparable.

# Representative live sections (chosen in the DB survey): math-dense / prose / owner's own note.
SECTION_IDS = [4257, 3916, 7712]

CLAUDE = str(Path.home() / ".local/bin/claude")
NEUTRAL_CWD = tempfile.mkdtemp(prefix="locus-routing-spike-")  # §10: no repo CLAUDE.md / project MCP

# PHASE-0 FINDING: locus.config.load() injects the project .env's ANTHROPIC_API_KEY into
# os.environ; `claude -p` would inherit it and prefer the metered key over the subscription
# OAuth login (defeating the "via claude -p, no key" decision + §7's channel split — and
# silently rerouting to metered billing). The agent-layer runner MUST run claude -p with these
# scrubbed. Build the clean env once.
import os as _os
_CLEAN_ENV = {k: v for k, v in _os.environ.items()
              if k not in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")}

_CALLS: list[dict] = []  # every claude -p call's cost/token record, appended by the shim


def _claude_p(prompt: str, model: str, *, role: str, retries: int = 2) -> str:
    """Shell `claude -p`, record cost/tokens from the envelope, return the .result string.

    `claude -p` errors are frequently transient (observed live: one call in a 27-call run
    exited nonzero with an incidental connector warning, then succeeded on retry). Retry with
    a short backoff, mirroring the §10 bounded-repair-then-degrade contract.
    """
    last = ""
    for attempt in range(retries + 1):
        proc = subprocess.run(
            [CLAUDE, "-p", prompt, "--output-format", "json", "--model", model],
            cwd=NEUTRAL_CWD, capture_output=True, text=True, timeout=300, env=_CLEAN_ENV,
        )
        if proc.returncode == 0 and proc.stdout.strip():
            break
        last = proc.stderr[:500] or f"empty stdout (rc={proc.returncode})"
        if attempt < retries:
            time.sleep(3 * (attempt + 1))
    else:
        raise RuntimeError(f"claude -p failed ({role}, {model}) after {retries + 1} tries: {last}")
    env = json.loads(proc.stdout)
    u = env.get("usage", {})
    _CALLS.append({
        "role": role, "model": model,
        "cost_usd": env.get("total_cost_usd"),
        "input_tokens": u.get("input_tokens"), "output_tokens": u.get("output_tokens"),
        "cache_create": u.get("cache_creation_input_tokens"),
        "cache_read": u.get("cache_read_input_tokens"),
        "api_error": env.get("api_error_status"),
    })
    if env.get("is_error"):
        raise RuntimeError(f"claude -p is_error ({role}, {model}): {env.get('result')!r}")
    return env.get("result", "")


def _slice_json(text: str) -> str:
    """Tolerant: slice the first {...} span out of prose around it (mirrors §10 contract)."""
    i, j = text.find("{"), text.rfind("}")
    if i == -1 or j == -1 or j < i:
        raise ValueError(f"no JSON object in claude output: {text[:200]!r}")
    return text[i : j + 1]


def make_shim(model: str, pass_name: str):
    """A generate_structured-compatible shim backed by claude -p for one engine."""

    def shim(schema: type[BaseModel], user: str, *, system: str = DEFAULT_SYSTEM,
             client=None, model=model, retries: int = 2, temperature: float = 0.0):
        prompt = (
            f"{system}\n\n{user}\n\n"
            "Return ONLY a single JSON object conforming to this JSON Schema "
            "(no prose, no code fences):\n"
            f"{json.dumps(schema.model_json_schema())}"
        )
        raw = _claude_p(prompt, model, role=f"gen:{pass_name}")
        return schema.model_validate_json(_slice_json(raw))

    return shim


def section_text(conn: sqlite3.Connection, section_id: int) -> tuple[str, str, str]:
    """(doc_title, section_title, reconstructed text) from ordered chunks."""
    row = conn.execute(
        "select d.title, s.title from sections s join documents d on d.id=s.doc_id where s.id=?",
        (section_id,),
    ).fetchone()
    chunks = conn.execute(
        "select raw_text from chunks where section_id=? order by position", (section_id,)
    ).fetchall()
    return row[0], row[1], "\n".join(c[0] for c in chunks)


@dataclass
class PassOutput:
    summary: str = ""
    propositions: list[str] = field(default_factory=list)
    entities: list[tuple[str, str]] = field(default_factory=list)


def run_engine(engine: str, model: str | None, title: str, text: str) -> PassOutput:
    """Run summary+propositions+entities for one engine. engine 'local' uses ollama; else claude."""
    if engine == "local":
        s = sum_mod.summarize_section(title, text)
        p = prop_mod.extract_propositions(title, text)
        e = ent_mod.extract_entities(title, text)
        return PassOutput(s.summary, p, [(x.name, x.type) for x in e])

    # Patch each module's generate_structured to the claude shim for this engine.
    orig = (sum_mod.generate_structured, prop_mod.generate_structured, ent_mod.generate_structured)
    try:
        sum_mod.generate_structured = make_shim(model, "summary")
        prop_mod.generate_structured = make_shim(model, "propositions")
        ent_mod.generate_structured = make_shim(model, "entities")
        s = sum_mod.summarize_section(title, text)
        p = prop_mod.extract_propositions(title, text)
        e = ent_mod.extract_entities(title, text)
    finally:
        sum_mod.generate_structured, prop_mod.generate_structured, ent_mod.generate_structured = orig
    return PassOutput(s.summary, p, [(x.name, x.type) for x in e])


def grade(source: str, out: PassOutput) -> dict:
    """Grade one engine's output on the source, via claude -p using the judge rubric."""
    user = judge_mod._build_user(source, out.summary, out.propositions, out.entities)
    prompt = (
        f"{judge_mod._SYSTEM}\n\n{user}\n\n"
        "Return ONLY a JSON object with integer fields (1-5): summary_faithfulness, "
        "proposition_faithfulness, proposition_atomicity, proposition_self_containment, "
        "entity_recall, entity_precision, and a string 'notes'."
    )
    raw = _claude_p(prompt, GRADER, role="judge")
    scores = judge_mod.SectionScores.model_validate_json(_slice_json(raw))
    return scores.model_dump()


def _write(results: list[dict]) -> None:
    """Persist results + call ledger after every step, so a crash never discards progress."""
    outpath = Path(__file__).parent / "routing_spike_out.json"
    outpath.write_text(json.dumps({"results": results, "calls": _CALLS}, indent=2))


def main() -> None:
    conn = sqlite3.connect(str(load().paths.db))
    results = []
    for sid in SECTION_IDS:
        dtitle, stitle, text = section_text(conn, sid)
        source = f"{stitle or ''}\n{text}"
        print(f"\n=== section {sid}: {dtitle!r} / {stitle!r} ({len(text)} chars) ===", file=sys.stderr)
        for engine, model in (("local", None), ("haiku", HAIKU), ("sonnet", SONNET)):
            print(f"  running {engine} ...", file=sys.stderr)
            try:
                out = run_engine(engine, model, stitle, text)
                scores = grade(source, out)
            except Exception as exc:  # one flaky engine must not sink the whole run
                print(f"    !! {engine} FAILED: {exc}", file=sys.stderr)
                results.append({"section_id": sid, "engine": engine, "model": model,
                                "error": str(exc)[:300]})
                _write(results)
                continue
            results.append({
                "section_id": sid, "doc_title": dtitle, "section_title": stitle,
                "engine": engine, "model": model,
                "n_props": len(out.propositions), "n_entities": len(out.entities),
                "summary_len": len(out.summary), "scores": scores,
                "mean": round(sum(v for k, v in scores.items() if isinstance(v, int)) / 6, 3),
            })
            print(f"    -> mean {results[-1]['mean']}  props={len(out.propositions)} ents={len(out.entities)}",
                  file=sys.stderr)
            _write(results)  # incremental: a later crash never discards earlier results

    print(f"\nwrote {Path(__file__).parent / 'routing_spike_out.json'}", file=sys.stderr)
    results = [r for r in results if "error" not in r]  # drop failures from the summary tables

    # --- summary table: quality per engine per pass-dimension ---
    print("\n### QUALITY (mean judge score, fixed Sonnet grader) ###")
    dims = ["summary_faithfulness", "proposition_faithfulness", "proposition_atomicity",
            "proposition_self_containment", "entity_recall", "entity_precision"]
    for engine in ("local", "haiku", "sonnet"):
        rows = [r for r in results if r["engine"] == engine]
        agg = {d: round(sum(r["scores"][d] for r in rows) / len(rows), 2) for d in dims}
        overall = round(sum(r["mean"] for r in rows) / len(rows), 3)
        print(f"{engine:7s} overall={overall}  " + "  ".join(f"{d.split('_')[0][:4]}.{d.split('_')[-1][:3]}={v}" for d, v in agg.items()))

    # --- cost: claude -p wall cost vs raw pass tokens ---
    print("\n### COST ###")
    for model in (HAIKU, SONNET):
        gen = [c for c in _CALLS if c["role"].startswith("gen:") and c["model"] == model]
        if not gen:
            continue
        wall = sum(c["cost_usd"] for c in gen)
        tin = sum(c["input_tokens"] or 0 for c in gen)
        tout = sum(c["output_tokens"] or 0 for c in gen)
        ncalls = len(gen)  # 3 passes x 3 sections = 9 durable-pass calls
        print(f"{model}: {ncalls} durable-pass calls | claude -p wall=${wall:.3f} "
              f"(harness-inflated) | raw pass tokens: {tin} in / {tout} out "
              f"(~{tin/ncalls:.0f} in, {tout/ncalls:.0f} out per pass-call)")


if __name__ == "__main__":
    main()
