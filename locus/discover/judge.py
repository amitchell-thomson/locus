"""Drop the candidates that are clearly not worth a slot — a FILTER, never a ranker.

Tested against real output on 2026-07-31, and the shape of the prompt decided whether this was
useful or actively harmful:

  Binary "would this help him? yes/no" REJECTED ALL 14 candidates, including the two strongest
  papers in the harvest, reasoning "focuses on market-neutral portfolios and does not address
  stop-loss cascades" — an exact-match standard, the precise opposite of method transfer. One
  reply also leaked Chinese mid-sentence. That is the §11.B failure class: the weakest model
  owning a high-value judgement.

  The same model asked for a 1-5 SCORE gave 1 to exactly the two papers judged junk by hand
  (Koopman PDE identification, language-tuned MPC) and 3 to the strongest — while the
  cross-encoder had ranked one of those junk papers SEVENTH, above three better ones.

So it is used the way it demonstrably works: as a floor. Anything scoring at or below
`drop_at_or_below` is removed; everything above keeps the cross-encoder's ordering untouched. It
never promotes, never reorders, and never invents — it only ever removes real candidates that a
model believes are irrelevant, which bounds the damage a bad verdict can do to one lost slot.

The narrow range it actually uses (1-3, never 4-5) is exactly why it must not rank: it has no
resolution at the top of its own scale.

Local by default and therefore free. Grounded: the judge only ever sees a real profile facet and a
real abstract, and its output is a number.
"""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass

log = logging.getLogger(__name__)

_PROMPT = """Rate how useful this paper would be to a quant researcher working on the project below.

Score 1-5, where 5 = the method directly transfers to his open problem, 3 = adjacent and worth
skimming, 1 = unrelated. Judge METHOD TRANSFER, not word overlap: a technique from another field
that solves the same shape of problem scores HIGH, and a paper sharing his vocabulary but
answering a different question scores low.

PROJECT: {label}
WHAT HE NEEDS: {facet}

PAPER: {title}
ABSTRACT: {abstract}

Reply with strict JSON only: {{"score": <1-5>, "why": "<one short clause>"}}"""


@dataclass
class Verdict:
    score: int
    why: str


def _local_judge(prompts: list[str], *, model: str, host: str) -> list[Verdict]:
    from ollama import Client

    client = Client(host=host)
    out: list[Verdict] = []
    for prompt in prompts:
        try:
            resp = client.generate(
                model=model, prompt=prompt, options={"temperature": 0}, format="json"
            )
            data = json.loads(resp["response"])
            out.append(Verdict(int(data.get("score", 3)), str(data.get("why", ""))[:160]))
        except Exception as exc:                    # model down, bad JSON, non-integer score
            # A judge that cannot answer must not silently reject: default to the neutral score
            # so a broken model degrades to "no filtering" rather than to "empty reading list".
            log.warning("judge failed on one candidate (%s) — passing it through", exc)
            out.append(Verdict(3, "judge unavailable"))
    return out


def score(
    items: list[tuple[str, str, str, str]],
    *,
    model: str,
    host: str,
    judge_fn=None,
) -> list[Verdict]:
    """Score `(label, facet, title, abstract)` tuples. Never raises; failures pass through as 3."""
    if not items:
        return []
    prompts = [
        _PROMPT.format(label=label, facet=(facet or "")[:600], title=title,
                       abstract=(abstract or "")[:900])
        for label, facet, title, abstract in items
    ]
    fn = judge_fn or (lambda ps: _local_judge(ps, model=model, host=host))
    verdicts = fn(prompts)
    if len(verdicts) != len(items):                 # a fake or a partial failure
        log.warning("judge returned %d verdicts for %d items — ignoring", len(verdicts), len(items))
        return [Verdict(3, "count mismatch") for _ in items]
    return verdicts
