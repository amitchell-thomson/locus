---
description: Write it up in LaTeX and send it to the reMarkable to read on paper (arg: a file path, or nothing to send what we just wrote)
---

Send a document to the reMarkable via the Locus MCP `to_remarkable` tool. Three modes — pick by **who wrote the thing**, not by where it came from.

**LaTeX (`latex=`) — the default for anything YOU wrote.** Author the document in LaTeX and pass the source. This is the standing preference: it is how Locus talks to him on paper, not a per-document judgement call. Use it for a plan, a summary, an answer, a derivation, a write-up — including documents with no maths in them. Give it a short `title`; it names the file on the device and heads page 1.

Pass a **fragment**: open with body text or `\section{...}` and give the document's name as `title` rather than writing your own heading first. The device preamble is wrapped around it — page size, margins, e-ink leading, and `amsmath`, `graphicx`, `booktabs`, `enumitem`, `hyperref`, `microtype`, `tikz`, `pgfplots` already loaded. Only write your own `\documentclass` when you deliberately need to override the layout; if you do, it is compiled exactly as written and the device geometry becomes your problem.

**Two columns, 9pt.** Each column is about 2.9in. A display equation wider than that hangs into the gutter, so break wide maths with `split` or `aligned`; use `figure*` and `table*` for anything that genuinely needs the full page width. Inside a float, `\linewidth` is the column — that is the width you want for `\includegraphics` and for pgfplots.

**Draw things.** This is the point of sending it to paper: he is reading to *understand* something, and a diagram beside the prose is most of what makes that work. TikZ and pgfplots are loaded, so draw the mechanism instead of describing it — the block diagram of a pipeline, the geometry behind a derivation, a plot of the function being discussed, an annotated axis showing what a parameter does. A drawn figure beats a paragraph explaining what the figure would show. The screen is **greyscale**: never encode meaning in colour (the default plot cycle separates series by dash pattern for that reason), and keep lines at the preamble's weight or heavier, because hairlines vanish on e-ink.

**His own figures are an option, not a first resort.** He confirmed (2026-09-08) that hand-drawn TikZ/pgfplots is what he wants, so do not open with a figure hunt and do not treat a schematic as a compromise. When a real figure from a paper he read genuinely fits, `\includegraphics[width=\linewidth]{<name>}` resolves against the corpus raw store by default, so any ingested figure drops in by the filename held in `figures.raw_path`. Point `resource_dir` somewhere else to include plots you just generated instead. A name that is not there fails with an error naming the file.

**Say when a figure is illustrative.** A schematic with invented numbers is fine and often the clearest thing to draw. A plot that *looks* like a result and is not must say so in its caption.

**Style.** Write a document, not typeset chat.

- **No em dashes.** Enforced — `---` or `—` in the body or title is refused, and the fix is to recast the sentence rather than swap in punctuation of the same shape. Colon, semicolon, parentheses, or two sentences. `--` for a numeric range is fine.
- **No assistant register.** No "Let's dive in", "It's worth noting that", "In this section we will", "I hope this helps". No closing paragraph that summarises the section it just ended, and no opening sentence announcing what the section is about. State the thing.
- No hedging filler, no rhetorical questions, no bulleted restatement of a paragraph that already made the point. Prose carries the argument; lists carry lists.
- Define a term at first use, then use it. He is reading to learn: undefined jargon is a dead end, and a defined term restated three times is padding.

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
