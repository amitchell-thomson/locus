# Writing mail in Neovim

keryx writes in your own Neovim, run inside the right-hand pane of the Mail view with the list
still beside it (keryx-joins-locus X6). Your config, motions and colours all apply; keryx adds a
small Lua file for that one buffer (`keryx/crates/keryx-tui/nvim/keryx.lua`, loaded with `-c`)
and never edits your dotfiles.

| You type | What happens |
|---|---|
| `:w` (or `:wq`, `:x`, `ZZ`) | send: keryx shows who it goes to and from which account, `y` sends after the 10 s hold (`u` cancels) |
| `:q` | close; a changed mail is kept as a draft, and `r` on the thread (or `+` for a new mail) opens it again |
| `:cq` | close and throw the draft away |
| `^a` | keryx's picker over Downloads, Desktop and Documents, newest first; `⏎` adds an `Attach:` line |

The file: headers (`From:` the account it goes from, `To:`, `Cc:`, `Bcc:`, `Subject:`, any
number of `Attach: ~/path`), a blank line, your message and signature, then the marker line (drawn
as "the quote below is kept"); the quote below it is kept. Change the account on `From:` for a new
mail or a forward; a reply goes from the account it came to. Each `Attach:` line shows the file's
size, or "not found".

**Address completion** on `To:`, `Cc:` and `Bcc:` comes from `keryx addresses` (the people you
write to most first): as you type when your config has blink.cmp (a source keryx registers for
this buffer only), and on `<C-x><C-u>` otherwise. `Attach:` paths complete with blink's own path
source.

keryx embeds the editor when `$VISUAL` (else `$EDITOR`) is `nvim`, or when neither is set and
`nvim` is on the PATH. Any other editor runs full screen as before, without these keys: save and
quit to get the confirm screen.
