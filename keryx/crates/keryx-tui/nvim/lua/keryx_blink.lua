-- keryx's address source for blink.cmp, registered by keryx.lua for keryx's mail buffer only.
local M = {}

function M.new()
  return setmetatable({}, { __index = M })
end

local function empty(callback)
  callback({ items = {}, is_incomplete_forward = false, is_incomplete_backward = false })
end

function M:get_completions(ctx, callback)
  local col = ctx.cursor[2]
  local start = _G.KeryxAddressStart and _G.KeryxAddressStart(ctx.line, col)
  if not start then
    return empty(callback)
  end
  local base = ctx.line:sub(start + 1, col)
  if #base < 2 then
    return empty(callback)
  end
  local row = ctx.cursor[1] - 1
  vim.system({ _G.KeryxBin or "keryx", "addresses", base, "--json", "--limit", "20" }, { text = true }, function(out)
    vim.schedule(function()
      local ok, data = pcall(vim.json.decode, out.stdout or "")
      local items = {}
      if ok and type(data) == "table" and data.contacts then
        for _, c in ipairs(data.contacts) do
          table.insert(items, {
            label = c.display,
            filterText = base,
            kind = require("blink.cmp.types").CompletionItemKind.Text,
            labelDetails = { description = (c.sent or 0) > 0 and ("you wrote " .. c.sent) or "" },
            textEdit = {
              newText = c.display,
              range = { start = { line = row, character = start }, ["end"] = { line = row, character = col } },
            },
          })
        end
      end
      callback({ items = items, is_incomplete_forward = true, is_incomplete_backward = true })
    end)
  end)
end

return M
