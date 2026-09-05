---
description: Summarise this session's decisions and save them to Locus as a rough note (Loop C)
---

Summarise the key decisions, conclusions, and reasoning from our conversation so far into a concise markdown note — capturing WHAT was decided or concluded and WHY, plus any open questions or next steps. Stay faithful to what was actually discussed; do not invent or embellish.

Then call the Locus MCP `capture` tool with:
- `content`: the markdown summary
- `title`: a short, descriptive title
- `project`: the relevant project tag if one clearly applies (otherwise omit)

Report back the capture path the tool returns. Do not ingest or run anything else — `capture` is write-to-inbox only; the note is picked up by the next Locus note-sync.

If the `capture` tool is not available, say so plainly rather than writing the note anywhere else — the Locus MCP server is not connected in this session.
