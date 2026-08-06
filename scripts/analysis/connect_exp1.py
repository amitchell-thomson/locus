"""CONNECT experiment 1: priority arms x context depth x model (2026-08-06).

Live model calls (subscription `claude -p`), NO database writes. Results cached to a JSON
file so re-runs only pay for what changed.

Pairs: 8 priority (paper|note|coursework <-> code repo — the owner's stated top want) and
4 learning (paper<->coursework, note<->paper — "same concept, different vocabulary").

Variants per pair:
  V0  current system verbatim: _doc_text (1400 cap, LIKE sections) + _TEMPLATE, haiku
  V1  deep context (entity-anchored sections, README-first for code, project-object body,
      2800 cap) + task-specific framing + model picks concept from the full shared list,
      with the pick verified against the list after the call, haiku
  V2  = V1 on sonnet

argv[1] = DB path. argv[2] (optional) = 'dry' to print prompts without calling, or a
shard filter like 'v0'/'v1'/'v2' to only make that variant's calls (parallel shards share
the per-key file cache safely).
"""

import itertools
import json
import sqlite3
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, ".")
DB = sys.argv[1] if len(sys.argv) > 1 else "vault/locus.db"
ARG2 = sys.argv[2] if len(sys.argv) > 2 else ""
DRY = ARG2 == "dry"
SHARD = ARG2 if ARG2 in ("v0", "v1", "v2") else None
CACHE_DIR = Path("/home/alec/.claude/jobs/f868e84e/tmp/exp1_cache")
CACHE_DIR.mkdir(exist_ok=True)

from locus.db.connection import get_connection

conn = get_connection(DB)
conn.row_factory = sqlite3.Row

from locus.agent import state
from locus.agent.compose_daily import TEACHABLE_TYPES, _MIN_TEACHABLE_CHARS
from locus.link import connect as C
from locus.link.related import _CANON_CTE, non_topical_names

# ---------------------------------------------------------------- pool (as measured before)
own_clause, own_params = state.owner_authored_sql("d")
docs = {
    r["id"]: dict(r)
    for r in conn.execute(
        f"SELECT d.id, d.title, d.category, d.source_type, d.source_uri, d.source_date, "
        f"d.thesis, d.method, d.result, ({own_clause}) AS own FROM documents d",
        own_params,
    )
}


def klass(d):
    if d["own"]:
        return "own-note"
    if d["source_type"] == "code":
        return "code"
    return d["category"] or "?"


generic = non_topical_names(conn)
marks = ",".join("?" * len(TEACHABLE_TYPES))
teachable = {
    r["n"].lower()
    for r in conn.execute(
        f"SELECT DISTINCT canonical_name AS n FROM entity_aliases "
        f"WHERE canonical_type IN ({marks})",
        TEACHABLE_TYPES,
    )
}
canon_docs = defaultdict(set)
for r in conn.execute(f"WITH {_CANON_CTE} SELECT canonical_name, doc_id FROM canon_docs"):
    canon_docs[r["canonical_name"]].add(r["doc_id"])


def qualifies(n):
    s = (n or "").strip()
    return (
        len(s) >= _MIN_TEACHABLE_CHARS
        and " " in s
        and s.lower() not in generic
        and s.lower() in teachable
    )


qual = {n: ds for n, ds in canon_docs.items() if len(ds) >= 2 and qualifies(n)}
pairs = defaultdict(set)
for n, ds in qual.items():
    for a, b in itertools.combinations(sorted(ds), 2):
        pairs[(a, b)].add(n)

# ---------------------------------------------------------------- pair selection
def pick(class_pair, k, exclude_docs=frozenset()):
    """Top-k pairs of a class by number of shared qualifying concepts, distinct docs first."""
    cands = sorted(
        (
            (len(names), a, b)
            for (a, b), names in pairs.items()
            if tuple(sorted((klass(docs[a]), klass(docs[b])))) == tuple(sorted(class_pair))
            and a not in exclude_docs
            and b not in exclude_docs
        ),
        reverse=True,
    )
    out, used = [], set()
    for nq, a, b in cands:
        if a in used or b in used:
            continue
        out.append((a, b))
        used |= {a, b}
        if len(out) >= k:
            break
    return out


VENDOR = {i for i, d in docs.items() if "optibook" in (d["title"] or "").lower()}
chosen: list[tuple[int, int, str]] = []
for cp, k, tag in (
    (("code", "paper"), 3, "PRIORITY code<->paper"),
    (("code", "own-note"), 3, "PRIORITY code<->note"),
    (("code", "coursework"), 2, "PRIORITY code<->coursework"),
    (("coursework", "paper"), 2, "LEARNING coursework<->paper"),
    (("own-note", "paper"), 2, "LEARNING note<->paper"),
):
    for a, b in pick(cp, k, exclude_docs=VENDOR):
        chosen.append((a, b, tag))

# ---------------------------------------------------------------- context builder v2
V2_CAP = 2800


def side_text_v2(doc_id: int, concepts: list[str]) -> str:
    d = docs[doc_id]
    parts = [p for p in (d["thesis"], d["method"], d["result"]) if p]
    # sections that ANCHOR any shared concept (the join that made the pair exist)
    marks_c = ",".join("?" * len(concepts))
    anchored = conn.execute(
        f"SELECT DISTINCT s.id, s.summary, s.file_path FROM sections s "
        f"JOIN entities e ON e.section_id=s.id "
        f"JOIN entity_aliases a ON a.variant_name=e.name AND a.variant_type=e.type "
        f"WHERE s.doc_id=? AND a.canonical_name IN ({marks_c}) "
        f"AND COALESCE(s.summary,'')!='' "
        f"ORDER BY (s.file_path IS NULL OR lower(s.file_path) LIKE '%.md') DESC LIMIT 6",
        (doc_id, *concepts),
    ).fetchall()
    parts += [r["summary"] for r in anchored]
    if d["source_type"] == "code":
        # README/narrative sections carry the repo's intent (CLAUDE.md §6)
        seen = {r["id"] for r in anchored}
        for r in conn.execute(
            "SELECT id, summary FROM sections WHERE doc_id=? "
            "AND lower(COALESCE(file_path,'')) LIKE '%.md' AND COALESCE(summary,'')!='' "
            "ORDER BY position LIMIT 3",
            (doc_id,),
        ):
            if r["id"] not in seen:
                parts.append(r["summary"])
        obj = conn.execute(
            "SELECT o.body FROM objects o JOIN object_links ol ON ol.object_id=o.id "
            "WHERE o.type='project' AND ol.target_kind='doc' AND ol.relation='implements' "
            "AND ol.target_key=? AND COALESCE(o.body,'')!=''",
            (d["source_uri"],),
        ).fetchone()
        if obj:
            parts.insert(0, obj["body"])
    return " ".join(parts)[:V2_CAP]


# ---------------------------------------------------------------- prompts
_PROJECT_SYSTEM = (
    "You find concrete, actionable ideas for a person's own software/quant projects from "
    "material in their personal knowledge corpus. You are given stored text from both sides. "
    "You never invent facts about either side; if the material offers nothing genuinely "
    "useful to the project, reply with exactly NO_CONNECTION."
)

_PROJECT_TEMPLATE = """HIS PROJECT (code he wrote and maintains):
{proj_title}
{proj_text}

MATERIAL ({material_kind}):
{mat_title}
{mat_text}

Concepts both sides develop: {concept_list}

Write ONE prompt (2-4 sentences, under 420 characters) proposing a specific idea from the
material for the project. Name the technique precisely, say which part of the project it
would change, and end with a concrete question he could act on. Do not restate titles, do
not say "this paper", do not flatter. Then on a separate final line write exactly:
CONCEPT: <the one concept from the list above that the idea is built on>"""

_LEARN_SYSTEM = (
    "You show a person that something they are reading and something they already studied "
    "are the same underlying idea in different vocabulary. You are given stored text from "
    "both sides. You never invent facts about either side; if the two treatments are not "
    "genuinely the same idea, reply with exactly NO_CONNECTION."
)

_LEARN_TEMPLATE = """SIDE A ({kind_a}):
{title_a}
{text_a}

SIDE B ({kind_b}):
{title_b}
{text_b}

Concepts both sides develop: {concept_list}

Write ONE prompt (2-3 sentences, under 420 characters) that makes the identification
precise: state how side A and side B treat the same underlying idea in different
vocabulary — name the specific technique or result on EACH side — and ask one question
that tests whether he can carry a result from one over to the other. Do not restate
titles. Then on a separate final line write exactly:
CONCEPT: <the one concept from the list above that the identification is built on>"""


def concept_list_for(a, b):
    names = sorted(pairs[(min(a, b), max(a, b))], key=lambda n: (len(qual[n]), n))
    return names[:6]


def build_v1_prompt(a, b):
    ka, kb = klass(docs[a]), klass(docs[b])
    names = concept_list_for(a, b)
    ta, tb = side_text_v2(a, names), side_text_v2(b, names)
    clist = "; ".join(names)
    if "code" in (ka, kb):
        proj, mat = (a, b) if ka == "code" else (b, a)
        km = klass(docs[mat])
        kind = {"paper": "a paper he read", "own-note": "his own notes",
                "coursework": "his university coursework"}.get(km, km)
        body = _PROJECT_TEMPLATE.format(
            proj_title=docs[proj]["title"], proj_text=side_text_v2(proj, names),
            material_kind=kind, mat_title=docs[mat]["title"],
            mat_text=side_text_v2(mat, names), concept_list=clist,
        )
        return f"{_PROJECT_SYSTEM}\n\n{body}", names, min(len(ta), len(tb))
    kind = {"paper": "a paper he read", "own-note": "his own notes",
            "coursework": "his university coursework"}
    body = _LEARN_TEMPLATE.format(
        kind_a=kind.get(ka, ka), title_a=docs[a]["title"], text_a=ta,
        kind_b=kind.get(kb, kb), title_b=docs[b]["title"], text_b=tb,
        concept_list=clist,
    )
    return f"{_LEARN_SYSTEM}\n\n{body}", names, min(len(ta), len(tb))


def build_v0_prompt(a, b):
    """The current system's prompt, byte-faithful to write_note."""
    names = concept_list_for(a, b)
    shared = names[0]
    his, other = (a, b) if docs[a]["own"] or docs[a]["source_type"] == "code" else (b, a)
    ht, htext = C._doc_text(conn, docs[his]["source_uri"], shared)
    ot, otext = C._doc_text(conn, docs[other]["source_uri"], shared)
    bridge = docs[other]["category"] == "coursework"
    template, system = (
        (C._BRIDGE_TEMPLATE, C._BRIDGE_SYSTEM) if bridge else (C._TEMPLATE, C._SYSTEM)
    )
    prompt = template.format(
        his_title=ht, his_text=htext, other_title=ot, other_text=otext, shared=shared
    )
    return f"{system}\n\n{prompt}", shared, min(len(htext), len(otext))


# ---------------------------------------------------------------- verification
def verify_v1(text, names):
    t = " ".join((text or "").split())
    if not t:
        return "EMPTY", None
    if "no_connection" in t.lower():
        return "NO_CONNECTION", None
    low = t.lower()
    if any(m in low for m in C._REFUSAL_MARKERS):
        return "REFUSAL", None
    if "concept:" not in low:
        return "NO_CONCEPT_LINE", None
    picked = t[low.rindex("concept:") + len("concept:"):].strip().strip(".").strip()
    match = next((n for n in names if n.lower() == picked.lower()), None)
    if match is None:
        return "CONCEPT_NOT_IN_LIST", picked
    body = t[: low.rindex("concept:")].strip()
    if len(body) > 500:
        return "TOO_LONG", match
    return "OK", (match, body)


# ---------------------------------------------------------------- run
# Per-key file cache: shards run in parallel without clobbering each other. Errors are NOT
# cached, so a rerun retries them.
_old = Path("/home/alec/.claude/jobs/f868e84e/tmp/exp1_cache.json")
if _old.exists():
    for k, v in json.loads(_old.read_text()).items():
        f = CACHE_DIR / f"{k.replace(':', '_')}.json"
        if not f.exists():
            f.write_text(json.dumps(v))
    _old.unlink()


def cached_call(key, prompt, model):
    f = CACHE_DIR / f"{key.replace(':', '_')}.json"
    if f.exists():
        return json.loads(f.read_text())
    if SHARD and not key.startswith(SHARD):
        return {"text": "<<PENDING (other shard)>>", "cost": 0.0}
    from locus.agent.claude import ClaudeError, call

    try:
        res = call(prompt, model=model)
        out = {"text": res.text, "cost": res.cost_usd, "usage_out":
               (res.usage or {}).get("output_tokens")}
        f.write_text(json.dumps(out))
    except ClaudeError as e:
        return {"text": f"<<ERROR: {e}>>", "cost": 0.0}
    return out


total_cost = 0.0
for a, b, tag in chosen:
    ka, kb = klass(docs[a]), klass(docs[b])
    names = concept_list_for(a, b)
    print("\n" + "=" * 90)
    print(f"[{tag}]  ({a},{b})")
    print(f"  A [{ka:9s}] {(docs[a]['title'] or '')[:70]}")
    print(f"  B [{kb:9s}] {(docs[b]['title'] or '')[:70]}")
    print(f"  shared qualifying: {names}")

    p0, shared0, ml0 = build_v0_prompt(a, b)
    p1, names1, ml1 = build_v1_prompt(a, b)
    print(f"  minlen v0={ml0} v2={ml1}")
    if DRY:
        if chosen.index((a, b, tag)) == 0:
            print("\n--- V1 PROMPT (first pair only) ---\n" + p1)
        continue

    r0 = cached_call(f"v0:{a}:{b}", p0, "haiku")
    r1 = cached_call(f"v1:{a}:{b}", p1, "haiku")
    r2 = cached_call(f"v2:{a}:{b}", p1, "sonnet")
    total_cost += r0["cost"] + r1["cost"] + r2["cost"]

    t0 = " ".join(r0["text"].split())
    ok0 = "OK" if shared0.lower() in t0.lower() and not any(
        m in t0.lower() for m in C._REFUSAL_MARKERS
    ) else "FAIL-GATE"
    print(f"\n  V0 haiku/current [{ok0}] shared={shared0!r}:\n    {t0[:600]}")
    for label, r in (("V1 haiku/deep", r1), ("V2 sonnet/deep", r2)):
        verdict, payload = verify_v1(r["text"], names1)
        if verdict == "OK":
            concept, body = payload
            print(f"\n  {label} [OK, concept={concept!r}]:\n    {body}")
        else:
            print(f"\n  {label} [{verdict}] payload={payload!r}:\n    "
                  f"{' '.join(r['text'].split())[:400]}")

print(f"\n\nTOTAL REPORTED COST: ${total_cost:.4f}")
