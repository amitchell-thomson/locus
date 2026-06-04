"""Math-OCR engine benchmark (plan step 6, eval phase D).

Races three OCR-to-markup candidates on flagged corpus pages so the routing engine is chosen
empirically (§11.C discipline), not by reputation:

  - qwen   : qwen2.5vl:7b via Ollama (VLM generalist; no new Python deps)
  - got    : GOT-OCR-2.0 via transformers (~580M modern OCR specialist)
  - nougat : facebook/nougat-small via transformers (~250M academic-PDF baseline)

Usage:
  uv run python scripts/benchmark_mathocr.py select            # pick pages, render PNGs
  uv run python scripts/benchmark_mathocr.py run --engine qwen # transcribe (one engine)
  (judging is scripts/judge_mathocr.py — Claude multimodal, builds the math-fidelity metric)

Outputs under eval-artifacts/mathocr/: pages/*.png, outputs/<engine>/*.md, manifest.json.
Engines run sequentially (8 GB VRAM is shared with Ollama); torch engines try CUDA and fall
back to CPU — ingest-time work is unbounded (§2.4), so speed is irrelevant here.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import pymupdf

from locus.config import load
from locus.db.connection import get_connection

BENCH_DIR = Path(__file__).resolve().parent.parent / "eval-artifacts" / "mathocr"
PAGES_DIR = BENCH_DIR / "pages"
OUT_DIR = BENCH_DIR / "outputs"
MANIFEST = BENCH_DIR / "manifest.json"
RENDER_ZOOM = 2.0  # ~144 DPI; readable math without huge images

# (doc_id, 1-based page, why it is in the sample) — spans every failure mode the detector
# flags: ligature/symbol corruption, dense CM math, vector-drawing formulas, math-unicode
# mangling, slide-image formulas.
SAMPLE: list[tuple[int, int, str]] = [
    (23, 3, "symbol corruption: omega->'!' (H(!), ei!t) + dropped ligatures"),
    (23, 16, "ligature + symbol corruption mid-document (5 broken words)"),
    (19, 18, "dense CM math: ODE kernel/integrating-factor derivations"),
    (19, 44, "dense CM math: separation-of-variables procedure"),
    (17, 30, "dense CM math: control theory (transfer functions)"),
    (22, 2, "vector-drawing formulas: linear algebra (Colab export)"),
    (22, 6, "vector-drawing formulas: MLE/Bayesian (Colab export)"),
    (24, 1, "math-unicode mangling: optimization notes"),
    (20, 40, "slides: signals content with image formulas"),
    (20, 90, "slides: comms content with image formulas"),
]

PROMPT = (
    "Transcribe this page to GitHub-flavored Markdown.\n"
    "- Transcribe ALL text faithfully and completely; do not summarize, skip, or describe.\n"
    "- Typeset every mathematical expression in LaTeX: $...$ inline, $$...$$ for display "
    "equations.\n"
    "- Preserve the heading/list structure you see.\n"
    "- Output ONLY the transcription, no commentary."
)


def _docs(conn) -> dict[int, Path]:
    raw = load().paths.raw_store
    return {
        r["id"]: raw / r["raw_path"]
        for r in conn.execute("SELECT id, raw_path FROM documents")
    }


def select() -> None:
    """Render the sample pages to PNGs and write the manifest."""
    conn = get_connection(load().paths.db)
    docs = _docs(conn)
    PAGES_DIR.mkdir(parents=True, exist_ok=True)
    items = []
    for doc_id, page_no, why in SAMPLE:
        pdf = pymupdf.open(docs[doc_id])
        page = pdf[page_no - 1]
        png = PAGES_DIR / f"doc{doc_id}_p{page_no}.png"
        page.get_pixmap(matrix=pymupdf.Matrix(RENDER_ZOOM, RENDER_ZOOM)).save(png)
        items.append(
            {
                "id": f"doc{doc_id}_p{page_no}",
                "doc_id": doc_id,
                "page": page_no,
                "why": why,
                "png": str(png),
                "text_layer": page.get_text("text"),  # for the judge's reference
            }
        )
        pdf.close()
        print(f"rendered {png.name}  ({why})")
    MANIFEST.write_text(json.dumps(items, indent=2))
    print(f"\n{len(items)} pages -> {MANIFEST}")


# --- engines ------------------------------------------------------------------------------


def _run_qwen(png: Path) -> str:
    from ollama import Client

    client = Client(host=load().ollama.host)
    resp = client.chat(
        model="qwen2.5vl:7b",
        messages=[{"role": "user", "content": PROMPT, "images": [str(png)]}],
        options={"temperature": 0.0, "num_ctx": 8192},
    )
    return resp["message"]["content"]


def _device():
    import torch

    return "cuda" if torch.cuda.is_available() else "cpu"


_GOT = {}


def _run_got(png: Path) -> str:
    import torch
    # Explicit classes: with transformers 5.x the Auto* registry hides GOT's processor (and
    # emits a misleading 'Unrecognized image processor' if torchvision/pillow are missing).
    from transformers import (
        AutoTokenizer,
        GotOcr2ForConditionalGeneration,
        GotOcr2ImageProcessor,
        GotOcr2Processor,
    )

    if "model" not in _GOT:
        name = "stepfun-ai/GOT-OCR-2.0-hf"
        _GOT["processor"] = GotOcr2Processor(
            image_processor=GotOcr2ImageProcessor.from_pretrained(name),
            tokenizer=AutoTokenizer.from_pretrained(name),
        )
        # .to(device) rather than device_map= : the latter pulls in accelerate for nothing.
        _GOT["model"] = (
            GotOcr2ForConditionalGeneration.from_pretrained(name, dtype=torch.float16)
            .to(_device())
            .eval()
        )
    proc, model = _GOT["processor"], _GOT["model"]
    inputs = proc(str(png), return_tensors="pt", format=True).to(model.device)
    with __import__("torch").no_grad():
        out = model.generate(
            **inputs, max_new_tokens=4096, do_sample=False,
            stop_strings="<|im_end|>", tokenizer=proc.tokenizer,
        )
    return proc.decode(out[0, inputs["input_ids"].shape[1]:], skip_special_tokens=True)


_NOUGAT = {}


def _run_nougat(png: Path) -> str:
    import torch
    from PIL import Image
    from transformers import NougatProcessor, VisionEncoderDecoderModel

    if "model" not in _NOUGAT:
        name = "facebook/nougat-small"
        _NOUGAT["processor"] = NougatProcessor.from_pretrained(name)
        # fp32: in fp16 generation intermittently trips a CUDA device-side assert (observed
        # on this corpus); the model is ~250M, so full precision costs nothing that matters.
        _NOUGAT["model"] = VisionEncoderDecoderModel.from_pretrained(name).to(_device()).eval()
    proc, model = _NOUGAT["processor"], _NOUGAT["model"]
    # transformers 5.x strict kwarg validation rejects the None defaults the legacy Nougat
    # processor ships with — pass every processing kwarg explicitly from its own config.
    ip = proc.image_processor
    keys = (
        "do_crop_margin", "do_resize", "do_thumbnail", "do_align_long_axis", "do_pad",
        "do_rescale", "do_normalize", "rescale_factor", "image_mean", "image_std", "resample",
    )
    kwargs = {k: getattr(ip, k) for k in keys if getattr(ip, k, None) is not None}
    kwargs["size"] = {"height": ip.size["height"], "width": ip.size["width"]}
    pixel_values = proc(Image.open(png).convert("RGB"), return_tensors="pt", **kwargs).pixel_values
    with torch.no_grad():
        out = model.generate(
            pixel_values.to(model.device, model.dtype),
            max_new_tokens=4096,
            do_sample=False,
            bad_words_ids=[[proc.tokenizer.unk_token_id]],
        )
    text = proc.batch_decode(out, skip_special_tokens=True)[0]
    return proc.post_process_generation(text, fix_markdown=True)


ENGINES = {"qwen": _run_qwen, "got": _run_got, "nougat": _run_nougat}


def run(engine: str) -> None:
    items = json.loads(MANIFEST.read_text())
    out_dir = OUT_DIR / engine
    out_dir.mkdir(parents=True, exist_ok=True)
    fn = ENGINES[engine]
    for item in items:
        dest = out_dir / f"{item['id']}.md"
        if dest.exists():
            print(f"skip {item['id']} (exists)")
            continue
        t0 = time.time()
        try:
            text = fn(Path(item["png"]))
        except Exception as exc:  # record the failure; the race must finish
            text = f"<ENGINE ERROR: {exc}>"
        dest.write_text(text)
        print(f"{engine} {item['id']}: {len(text)} chars in {time.time() - t0:.0f}s")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("select")
    pr = sub.add_parser("run")
    pr.add_argument("--engine", choices=sorted(ENGINES), required=True)
    args = ap.parse_args()
    if args.cmd == "select":
        select()
    else:
        run(args.engine)


if __name__ == "__main__":
    sys.exit(main())
