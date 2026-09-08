---
description: Write it up in LaTeX and send it to the reMarkable to read on paper (arg: a file path, or nothing to send what we just wrote)
---

Send a document to the reMarkable via the Locus MCP `to_remarkable` tool. Three modes — pick by **who wrote the thing**, not by where it came from.

**LaTeX (`latex=`) — the default for anything YOU wrote.** Author the document in LaTeX and pass the source. This is the standing preference: it is how Locus talks to him on paper, not a per-document judgement call. Use it for a plan, a summary, an answer, a derivation, a write-up — including documents with no maths in them. Give it a short `title`; it names the file on the device and heads page 1.

Pass a **fragment**: start at `\section{...}` and write body text. The device preamble is wrapped around it — page size, margins, e-ink leading, and `amsmath`, `graphicx`, `booktabs`, `enumitem`, `hyperref`, `microtype` already loaded. Only write your own `\documentclass` when you deliberately need to override the layout; if you do, it is compiled exactly as written and the device geometry becomes your problem.

Write it as a document, not as transcribed chat: real sectioning, `align` for anything multi-line, `tabular` with `booktabs` rules for anything tabular, `\includegraphics[width=...]` for figures. That control is the entire reason the format was chosen.

**Markdown (`markdown=`) — only for relaying text that is ALREADY markdown.** His own notes, a `.md` file you read, a stored pass output. It sends those unchanged, which is the point: do not re-author his words as LaTeX. Never use this for prose you are composing.

**Existing PDF (`pdf_path=`).** Only when a PDF already exists — one you just generated, or one in the repo or vault. Pushed unchanged, nothing reflows. The path resolves **on the Locus server**: absolute, relative to its working directory, or relative to the checkout root (`docs/plan.pdf`). `title` is optional and defaults to the filename.

Pass exactly one of the three.

What to send, in order of preference:
1. If `$ARGUMENTS` names a file — a PDF goes via `pdf_path`; a `.md` file you read and relay via `markdown`; anything else you read and typeset as `latex`.
2. If `$ARGUMENTS` describes a document ("the plan", "your last answer"), send that — as `latex` if you wrote it, as `markdown` if it is his.
3. If `$ARGUMENTS` is empty, send the most substantial thing we produced in this conversation.

Do not summarise or rewrite the document unless asked. Typesetting it in LaTeX is not rewriting it — keep the words, give them structure. Report the device path the tool returns.

If a LaTeX send comes back with a compile error, the message names the line that broke. Fix the source and call again; do not fall back to `markdown=` to get something through, because that silently downgrades the thing he asked for.

If a `pdf_path` send comes back saying the file could not be found on the server, **do not retry with a different path**: it means this session is not running on the Locus server, and no path will work. Say so, and offer to send the content as `latex` instead.

If the `to_remarkable` tool is not available, say so plainly — the Locus MCP server is not connected in this session.
