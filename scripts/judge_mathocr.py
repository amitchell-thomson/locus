"""Judge the math-OCR benchmark outputs with Claude (multimodal) and write the report.

For every sample page, judges the raw pymupdf text layer (the BASELINE — what ingest uses
today) plus each engine's transcription, against the rendered page image as ground truth.
Produces:
  eval-artifacts/mathocr/results.json  — all judgements
  eval-artifacts/mathocr/report.md     — side-by-side summary for the owner's spot-check

Usage: uv run python scripts/judge_mathocr.py [--model claude-...]  (needs ANTHROPIC_API_KEY)
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from locus.config import Config, load
from locus.eval.math_fidelity import judge_transcription

BENCH_DIR = Path(__file__).resolve().parent.parent / "eval-artifacts" / "mathocr"
MANIFEST = BENCH_DIR / "manifest.json"
OUT_DIR = BENCH_DIR / "outputs"
RESULTS = BENCH_DIR / "results.json"
REPORT = BENCH_DIR / "report.md"


def candidates_for(item: dict) -> dict[str, str]:
    """All transcriptions to judge for one page: text-layer baseline + engine outputs."""
    out = {"text-layer": item["text_layer"]}
    for engine_dir in sorted(OUT_DIR.iterdir()) if OUT_DIR.exists() else []:
        f = engine_dir / f"{item['id']}.md"
        if f.exists():
            text = f.read_text()
            if not text.startswith("<ENGINE ERROR"):
                out[engine_dir.name] = text
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default=None, help="judge model (default: config generation model)")
    args = ap.parse_args()

    import anthropic

    client = anthropic.Anthropic(api_key=Config.anthropic_api_key())
    model = args.model or load().generation.model

    items = json.loads(MANIFEST.read_text())
    results: dict = json.loads(RESULTS.read_text()) if RESULTS.exists() else {}

    for item in items:
        pid = item["id"]
        results.setdefault(pid, {"why": item["why"]})
        for name, text in candidates_for(item).items():
            if name in results[pid]:
                print(f"skip {pid}/{name} (judged)")
                continue
            j = judge_transcription(client, model, Path(item["png"]), text)
            results[pid][name] = j.model_dump() | {"math_fidelity": round(j.math_fidelity, 3)}
            RESULTS.write_text(json.dumps(results, indent=2))  # checkpoint per judgement
            print(
                f"{pid:>11} {name:<10} fidelity={j.math_fidelity:.2f} "
                f"(eq {j.equations_correct}+{j.equations_partial}p/{j.equations_on_page}, "
                f"halluc {j.equations_hallucinated}, prose {j.prose_fidelity}/5"
                f"{', LOOP' if j.repetition_loop else ''})  {j.notes[:70]}"
            )

    _write_report(items, results, model)
    print(f"\nreport -> {REPORT}")


def _write_report(items: list[dict], results: dict, model: str) -> None:
    engines = sorted({k for v in results.values() for k in v if k != "why"})
    lines = [
        "# Math-OCR benchmark report",
        "",
        f"Judge: {model}. Ground truth: rendered page image. "
        "Score = (correct + 0.5×partial) / equations on page.",
        "",
        "## Aggregate",
        "",
        "| candidate | mean fidelity | hallucinated (total) | loops | mean prose |",
        "|---|---|---|---|---|",
    ]
    for e in engines:
        js = [results[i["id"]][e] for i in items if e in results.get(i["id"], {})]
        if not js:
            continue
        mean_f = sum(j["math_fidelity"] for j in js) / len(js)
        halluc = sum(j["equations_hallucinated"] for j in js)
        loops = sum(1 for j in js if j["repetition_loop"])
        prose = sum(j["prose_fidelity"] for j in js) / len(js)
        lines.append(f"| {e} | {mean_f:.2f} | {halluc} | {loops} | {prose:.1f}/5 |")
    lines += ["", "## Per page", ""]
    for item in items:
        pid = item["id"]
        lines += [f"### {pid} — {item['why']}", "", f"![page](pages/{pid}.png)", ""]
        lines.append("| candidate | fidelity | eq correct/partial/missing | halluc | notes |")
        lines.append("|---|---|---|---|---|")
        for e in engines:
            j = results.get(pid, {}).get(e)
            if not j:
                continue
            lines.append(
                f"| {e} | {j['math_fidelity']:.2f} | "
                f"{j['equations_correct']}/{j['equations_partial']}/{j['equations_missing']} "
                f"of {j['equations_on_page']} | {j['equations_hallucinated']} | "
                f"{j['notes'][:120]} |"
            )
        lines.append("")
    REPORT.write_text("\n".join(lines))


if __name__ == "__main__":
    main()
