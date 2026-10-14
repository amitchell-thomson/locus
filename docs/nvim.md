# Address completion in Neovim

keryx opens replies and new mail as `keryx-*.eml` files. This snippet completes addresses on `To:`, `Cc:` and `Bcc:` lines from `keryx addresses` (people you write to most come first). Add it to your Neovim config (chezmoi); keryx never edits your dotfiles.

```lua
-- keryx: mail filetype + address completion (<C-x><C-u>) in keryx-*.eml
vim.api.nvim_create_autocmd({ "BufRead", "BufNewFile" }, {
  pattern = "keryx-*.eml",
  callback = function(ev)
    vim.bo[ev.buf].filetype = "mail"
    vim.bo[ev.buf].completefunc = "v:lua.KeryxComplete"
  end,
})

function _G.KeryxComplete(findstart, base)
  local line = vim.api.nvim_get_current_line()
  if findstart == 1 then
    if not (line:match("^To:") or line:match("^Cc:") or line:match("^Bcc:")) then
      return -3 -- not an address line: cancel quietly
    end
    local col = vim.api.nvim_win_get_cursor(0)[2]
    local start = col
    while start > 0 and not line:sub(start, start):match("[,:]") do
      start = start - 1
    end
    while line:sub(start + 1, start + 1) == " " do
      start = start + 1
    end
    return start
  end
  local out = vim.fn.system({ "keryx", "addresses", base, "--json", "--limit", "20" })
  local ok, data = pcall(vim.json.decode, out)
  if not ok or not data.contacts then
    return {}
  end
  local items = {}
  for _, c in ipairs(data.contacts) do
    table.insert(items, { word = c.display, menu = (c.sent > 0) and ("you wrote " .. c.sent) or "" })
  end
  return items
end
```

Type part of a name or address after `To: ` and press `<C-x><C-u>`. With blink.cmp or nvim-cmp you can wrap the same `keryx addresses <prefix> --json` call as a source.

The file layout: headers (`To:`, `Cc:`, `Bcc:`, `Subject:`, any number of `Attach: ~/path`), a blank line, your message and signature, then the marker line — the quote below it is kept. Save and quit to get the confirm screen; save an empty message to send nothing.
