---
description: Send a document to the reMarkable to read on paper (arg: a file path, or nothing to send what we just wrote)
---

Send a document to the reMarkable via the Locus MCP `to_remarkable` tool. It has two modes — pick by what the thing IS, not by where it came from.

**Markdown (`markdown=`) — the default.** Pass the document's TEXT. Use this for anything you wrote or read: a plan, a summary, an answer, the contents of a `.md` file. Read the file yourself and pass its contents; never pass a path in this mode. It works from any machine, because the text travels with the call. Give it a short `title` — it names the file on the device and heads page 1.

**Existing PDF (`pdf_path=`).** Pass a path. Use this only when a PDF already exists — one you just generated, or one in the repo or vault. It is pushed unchanged, not re-rendered, so nothing reflows. The path is resolved **on the Locus server**: absolute, or relative to the server's working directory, or relative to the Locus checkout root (`docs/plan.pdf`). `title` is optional here and defaults to the filename.

Pass exactly one of the two.

What to send, in order of preference:
1. If `$ARGUMENTS` names a file — a PDF goes via `pdf_path`, anything else you read and send as `markdown`.
2. If `$ARGUMENTS` describes a document ("the plan", "your last answer"), send that as markdown.
3. If `$ARGUMENTS` is empty, send the most substantial thing we produced in this conversation.

Do not summarise or rewrite the document unless asked — send it as it is. Report the device path the tool returns.

If a `pdf_path` send comes back saying the file could not be found on the server, **do not retry with a different path**: it means this session is not running on the Locus server, and no path will work. Say so, and offer to send the content as markdown instead.

If the `to_remarkable` tool is not available, say so plainly — the Locus MCP server is not connected in this session.
