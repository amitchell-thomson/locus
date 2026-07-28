"""The critique / synthesis surface — the primary interaction (agent-layer plan §3.5, §8.4).

The plan's module list (§11) draws only the components it names; this package is where the two
composed surfaces live, because they belong to neither `structure/` (which proposes objects) nor
`evolve/` (which renders trajectories) — they COMPOSE those with retrieval into an answer.

Both follow one shape, and it is the shape that matters:

    ground in-process (retrieval + objects + positions + gaps — deterministic, free)
      -> hand the assembled evidence to `claude -p`
      -> verify every claim it makes against what it was given, and DROP the rest

The verification step is not decoration. A critique whose challenges are not traceable to the
owner's own corpus is just a language model's opinion, and it would be indistinguishable from a
grounded one at the point of reading — which is how trust erodes (failure mode #2).
"""
