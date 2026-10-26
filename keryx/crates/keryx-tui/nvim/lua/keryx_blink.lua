-- keryx's address source for blink.cmp, registered by keryx.lua for keryx's mail buffer only.
-- On To:, Cc: and Bcc: it offers the people in your mail (keryx addresses: the ones you write
-- to most first), live as you type; on From: your own accounts. Each entry carries its own
-- icon and highlight so the menu reads as addresses, not as words from the buffer.
local M = {}

function M.new()
  return setmetatable({}, { __index = M })
end

function M:get_trigger_characters()
  return { ",", " ", ":" }
end

local function done(callback, items, more)
  callback({ items = items, is_incomplete_forward = more, is_incomplete_backward = more })
end

local function ago(t)
  -- keryx addresses gives an RFC 3339 date; the day is enough.
  if type(t) == "string" then
    return t:sub(1, 10)
  end
  local d = os.time() - (t or 0)
  if d < 86400 then
    return "today"
  elseif d < 86400 * 30 then
    return string.format("%d days ago", math.floor(d / 86400))
  elseif d < 86400 * 365 then
    return string.format("%d months ago", math.floor(d / 86400 / 30))
  end
  return string.format("%d years ago", math.floor(d / 86400 / 365))
end

local function item(label, start, col, row, opts)
  return vim.tbl_extend("force", {
    label = label,
    -- On To/Cc/Bcc the next address follows straight on: ", " comes with it.
    insertText = opts.text or label,
    filterText = opts.filter or label,
    kind_name = opts.kind_name,
    kind_icon = opts.icon,
    kind_hl = opts.hl,
    labelDetails = { description = opts.detail or "" },
    documentation = opts.doc and { kind = "markdown", value = opts.doc } or nil,
    textEdit = {
      newText = opts.text or label,
      range = { start = { line = row, character = start }, ["end"] = { line = row, character = col } },
    },
  }, opts.extra or {})
end

function M:get_completions(ctx, callback)
  local line, col, row = ctx.line, ctx.cursor[2], ctx.cursor[1] - 1
  if line:match("^From:") then
    local start = #(line:match("^From:%s*") or "From: ")
    local items = {}
    for acc in (vim.env.KERYX_ACCOUNTS or ""):gmatch("[^\n]+") do
      table.insert(items, item(acc, start, #line, row, {
        kind_name = "Account",
        icon = "󰀄",
        hl = "KeryxOwn",
        detail = "your account",
        filter = line:sub(start + 1, col) ~= "" and acc or nil,
      }))
    end
    return done(callback, items, false)
  end
  local start = _G.KeryxAddressStart and _G.KeryxAddressStart(line, col, ctx.cursor[1])
  if not start then
    return done(callback, {}, false)
  end
  local base = line:sub(start + 1, col)
  -- Each recipient on its own line, lined up under the first ("To: " is four wide).
  local key = line:match("^(%a+:)") or (_G.KeryxKeyAt and _G.KeryxKeyAt(ctx.cursor[1])) or "To:"
  local next_line = ",\n" .. string.rep(" ", #key + 1)
  -- Your own accounts first (Personal, Oxford, OQTS), as the From: line names them; keryx
  -- addresses leaves your addresses out of the people.
  local own = {}
  local low = base:lower()
  for acc in (vim.env.KERYX_ACCOUNTS or ""):gmatch("[^\n]+") do
    if low == "" or acc:lower():find(low, 1, true) then
      table.insert(own, item(acc, start, col, row, {
        kind_name = "Account",
        icon = "󰀄",
        hl = "KeryxOwn",
        detail = "you",
        filter = base,
        text = acc .. next_line,
        extra = { sortText = "0" .. acc },
      }))
    end
  end
  if #base < 1 then
    return done(callback, own, true)
  end
  vim.system({ _G.KeryxBin or "keryx", "addresses", base, "--json", "--limit", "20" }, { text = true }, function(out)
    vim.schedule(function()
      local ok, data = pcall(vim.json.decode, out.stdout or "")
      local items = own
      if ok and type(data) == "table" and data.contacts then
        for i, c in ipairs(data.contacts) do
          local wrote = (c.sent or 0) > 0 and string.format("you wrote %d", c.sent)
            or string.format("wrote to you %d", c.received or 0)
          table.insert(items, item(c.display, start, col, row, {
            kind_name = "Address",
            icon = "󰇮",
            hl = "KeryxAddress",
            detail = wrote,
            -- Keep keryx's order (the people you write to most first) and match on what you typed.
            filter = base,
            text = c.display .. next_line,
            doc = string.format("**%s**\n\n`%s`\n\n%s · %s · last %s", c.name or c.email, c.email, wrote,
              string.format("%d from them", c.received or 0), ago(c.last)),
            extra = { sortText = string.format("1%04d", i) },
          }))
        end
      end
      done(callback, items, true)
    end)
  end)
end

return M
