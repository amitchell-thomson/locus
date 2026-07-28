"""Agent-layer orchestration (agent-layer plan §10, Phase 1 ②).

The trust boundary between Locus and Claude for the capture/enrichment/structuring loops:
  - `claude.py`  — the one `claude -p` runner (env-scrubbed, retry-then-degrade, schema-validated).
  - `journal.py` — crash-safe `agent_runs` journaling (a run never loses track of its spend).
  - `budget.py`  — a cost/token ledger over the `claude -p` envelope + a daily-cap guard.

Grounding is done IN-PROCESS by the local retrieval engine (deterministic, free); `claude -p`
is used only for language tasks. See the plan §10 for the full contract.
"""
