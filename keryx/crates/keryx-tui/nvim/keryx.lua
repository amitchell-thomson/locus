-- keryx: writing a mail in your Neovim, inside keryx's right pane (keryx-joins-locus X6).
-- keryx loads this with -c for the one buffer it opens; your config is never edited.
--
--   :w  (or :wq, :x, ZZ)  send: keryx shows who it goes to, then the 10 s hold
--   :q                    close; a changed mail is kept as a draft (r or + opens it again)
--   :cq                   close and throw the draft away
--   gx on an Attach: line open that file to check it
--
-- keryx passes KERYX_SENT (touched on a write, which means send), KERYX_DRAFT (where a kept
-- draft goes) and KERYX_BIN (the keryx binary, for address completion).

local buf = vim.api.nvim_get_current_buf()
local sent, draft, bin = vim.env.KERYX_SENT, vim.env.KERYX_DRAFT, vim.env.KERYX_BIN or "keryx"
local marker = "-- keryx: text above this line is sent; the quote below is kept --"

vim.bo[buf].filetype = "mail"
vim.bo[buf].completefunc = "v:lua.KeryxComplete"
-- The template's empty headers end in a space; your listchars would show it on every one.
vim.wo.list = false

vim.api.nvim_create_autocmd("BufWritePost", {
  buffer = buf,
  callback = function()
    vim.fn.writefile({}, sent)
    vim.schedule(function()
      vim.cmd("qa!")
    end)
  end,
})

vim.api.nvim_create_autocmd("QuitPre", {
  buffer = buf,
  callback = function()
    if vim.bo[buf].modified and draft then
      vim.fn.writefile(vim.api.nvim_buf_get_lines(buf, 0, -1, false), draft)
      vim.bo[buf].modified = false
    end
  end,
})

-- The header block ends at the first blank line.
local function header_end()
  for i, l in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    if l == "" then
      return i - 1
    end
  end
  return 0
end

-- The header a line belongs to (`To:`), following indented lines back to the one they continue.
local function key_at(row)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, row, false)
  for i = row, 1, -1 do
    local key = (lines[i] or ""):match("^(%a+:)")
    if key then
      return key
    end
    if not (lines[i] or ""):match("^%s") then
      return nil
    end
  end
  return nil
end
_G.KeryxKeyAt = key_at

-- The mail filetype hard-wraps at 72 columns as you type, which broke a long To: in two (and
-- an unindented second half is no header at all). Lines wrap on screen instead.
vim.bo[buf].textwidth = 0
vim.bo[buf].formatoptions = vim.bo[buf].formatoptions:gsub("[tc]", "")
-- Long headers wrap under themselves instead of running off the pane.
vim.wo.wrap = true
vim.wo.linebreak = true
vim.wo.breakindent = true
vim.wo.breakindentopt = "shift:4,sbr"
vim.wo.showbreak = "↳ "

-- keryx's colours, from your colourscheme's own groups.
vim.api.nvim_set_hl(0, "KeryxHeader", { link = "Statement", default = true })
vim.api.nvim_set_hl(0, "KeryxAddress", { link = "Special", default = true })
vim.api.nvim_set_hl(0, "KeryxOwn", { link = "Function", default = true })
vim.api.nvim_set_hl(0, "KeryxName", { link = "Identifier", default = true })

-- Sizes beside Attach: lines, a rule under the headers, the marker drawn quietly.
local ns = vim.api.nvim_create_namespace("keryx")
local function human(n)
  if n >= 1024 * 1024 then
    return string.format("%.1f MB", n / 1024 / 1024)
  end
  return string.format("%d kB", math.max(1, math.floor(n / 1024 + 0.5)))
end
local function decorate()
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local stop = header_end()
  local current = nil
  for i = 1, stop do
    -- The header's field names, and each recipient on it (its name and its address), read as
    -- such; indented lines continue the header above.
    local key = lines[i]:match("^(%a+:)")
    local from = 1
    if key then
      current = key
      vim.api.nvim_buf_set_extmark(buf, ns, i - 1, 0, { end_col = #key, hl_group = "KeryxHeader" })
      from = #key + 1
    elseif not lines[i]:match("^%s") then
      current = nil
    end
    if current == "To:" or current == "Cc:" or current == "Bcc:" or current == "From:" then
      local line = lines[i]
      while from <= #line do
        local stop_at = line:find(",", from, true) or (#line + 1)
        local entry = line:sub(from, stop_at - 1)
        local name_s, addr_s, addr_e = entry:match("^%s*()[^<]-%s*()<[^>]*>()")
        if addr_s then
          if addr_s > name_s then
            vim.api.nvim_buf_set_extmark(buf, ns, i - 1, from - 1 + name_s - 1, {
              end_col = from - 1 + addr_s - 1,
              hl_group = current == "From:" and "KeryxOwn" or "KeryxName",
            })
          end
          vim.api.nvim_buf_set_extmark(buf, ns, i - 1, from - 1 + addr_s - 1, {
            end_col = from - 1 + addr_e - 1,
            hl_group = current == "From:" and "KeryxOwn" or "KeryxAddress",
          })
        else
          local s, e = entry:find("[%w%._%%+%-]+@[%w%.%-]+%.%a+")
          if s then
            vim.api.nvim_buf_set_extmark(buf, ns, i - 1, from - 1 + s - 1, {
              end_col = from - 1 + e,
              hl_group = current == "From:" and "KeryxOwn" or "KeryxAddress",
            })
          end
        end
        from = stop_at + 1
      end
    end
    local p = lines[i]:match("^Attach:%s*(.-)%s*$")
    if p and p ~= "" then
      local st = vim.uv.fs_stat(vim.fn.expand(p))
      local text, hl = "not found", "DiagnosticError"
      if st and st.type == "file" then
        text, hl = human(st.size), "Comment"
      elseif st then
        text = "not a file"
      end
      vim.api.nvim_buf_set_extmark(buf, ns, i - 1, 0, { virt_text = { { "  " .. text, hl } } })
    end
  end
  if stop > 0 then
    local width = math.max(10, vim.api.nvim_win_get_width(0) - 8)
    vim.api.nvim_buf_set_extmark(buf, ns, stop, 0, {
      virt_text = { { string.rep("─", width), "FloatBorder" } },
      virt_text_pos = "overlay",
    })
  end
  for i, l in ipairs(lines) do
    if l == marker then
      local label = "┄ the quote below is kept ┄"
      local pad = string.rep(" ", math.max(0, vim.fn.strdisplaywidth(l) - vim.fn.strdisplaywidth(label)))
      vim.api.nvim_buf_set_extmark(buf, ns, i - 1, 0, {
        virt_text = { { label .. pad, "Comment" } },
        virt_text_pos = "overlay",
      })
    end
  end
end
vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI", "BufEnter", "VimResized" }, {
  buffer = buf,
  callback = decorate,
})
decorate()

-- gx on an Attach: line opens that file to check it (Preview for a PDF); elsewhere gx is yours.
vim.keymap.set("n", "gx", function()
  local p = vim.api.nvim_get_current_line():match("^Attach:%s*(.-)%s*$")
  if p and p ~= "" then
    vim.ui.open(vim.fn.expand(p))
  else
    vim.ui.open(vim.fn.expand("<cfile>"))
  end
end, { buffer = buf, desc = "keryx: open the attachment on this line" })

-- After the last recipient: the empty indented line and its comma go, when you leave insert
-- mode and before a write, so the header reads as you would type it.
local function tidy_header()
  local stop = header_end()
  local lines = vim.api.nvim_buf_get_lines(buf, 0, stop, false)
  local out, changed = {}, false
  for _, l in ipairs(lines) do
    if l:match("^%s+$") then
      changed = true
    else
      table.insert(out, l)
    end
  end
  for i, l in ipairs(out) do
    local nxt = out[i + 1]
    if l:match(",%s*$") and not (nxt and nxt:match("^%s")) then
      out[i] = l:gsub(",%s*$", "")
      changed = true
    end
  end
  if changed then
    vim.api.nvim_buf_set_lines(buf, 0, stop, false, out)
  end
end
vim.api.nvim_create_autocmd({ "InsertLeave", "BufWritePre" }, { buffer = buf, callback = tidy_header })

-- keryx's ^a picker sends this.
vim.api.nvim_create_user_command("KeryxAttach", function(o)
  local at = header_end()
  vim.api.nvim_buf_set_lines(buf, at, at, false, { "Attach: " .. o.args })
  decorate()
end, { nargs = "+" })

-- Addresses from your mail on To:, Cc: and Bcc: (<C-x><C-u>, or blink as you type).
local function address_start(line, col, row)
  local key = line:match("^(%a+:)") or (row and key_at(row))
  if not (key == "To:" or key == "Cc:" or key == "Bcc:") then
    return nil
  end
  local start = col
  while start > 0 and not line:sub(start, start):match("[,:]") do
    start = start - 1
  end
  while line:sub(start + 1, start + 1) == " " do
    start = start + 1
  end
  return start
end

local function contacts(base)
  local out = vim.fn.system({ bin, "addresses", base, "--json", "--limit", "20" })
  local ok, data = pcall(vim.json.decode, out)
  if not ok or type(data) ~= "table" or not data.contacts then
    return {}
  end
  return data.contacts
end

function _G.KeryxComplete(findstart, base)
  local line = vim.api.nvim_get_current_line()
  if findstart == 1 then
    local cur = vim.api.nvim_win_get_cursor(0)
    return address_start(line, cur[2], cur[1]) or -3
  end
  local items = {}
  for _, c in ipairs(contacts(base)) do
    table.insert(items, { word = c.display, menu = (c.sent or 0) > 0 and ("you wrote " .. c.sent) or "" })
  end
  return items
end

-- blink.cmp, when your config has it: on the header lines only keryx's source (addresses on
-- To/Cc/Bcc, your accounts on From, file paths on Attach), so the menu is a list of people and
-- not words from the buffer; in the body your usual sources. Set in this Neovim only.
local here = debug.getinfo(1, "S").source:sub(2):match("(.*/)") or "./"
vim.opt.runtimepath:append(here)
vim.api.nvim_create_autocmd("InsertEnter", {
  buffer = buf,
  once = true,
  callback = function()
    local ok, blink = pcall(require, "blink.cmp")
    if not (ok and blink.add_source_provider) then
      return
    end
    pcall(blink.add_source_provider, "keryx", {
      name = "Mail",
      module = "keryx_blink",
      min_keyword_length = 0,
      score_offset = 100,
    })
    local okc, config = pcall(require, "blink.cmp.config")
    if okc then
      config.sources.per_filetype.mail = function()
        local row = vim.api.nvim_win_get_cursor(0)[1]
        if row <= header_end() then
          local line = vim.api.nvim_get_current_line()
          if line:match("^Attach:") then
            return { "path" }
          end
          return { "keryx" }
        end
        return { inherit_defaults = true }
      end
    end
  end,
})
-- Within the header, show the menu as you type, without waiting for a keyword.
vim.api.nvim_create_autocmd("TextChangedI", {
  buffer = buf,
  callback = function()
    if vim.api.nvim_win_get_cursor(0)[1] <= header_end() then
      local ok, blink = pcall(require, "blink.cmp")
      if ok and blink.show and not (blink.is_visible and blink.is_visible()) then
        pcall(blink.show, { providers = { "keryx" } })
      end
    end
  end,
})
_G.KeryxAddressStart = address_start
_G.KeryxBin = bin

-- Start where the writing is: an empty To: for a new mail, else the body, in insert mode.
local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
local to_line
for i = 1, header_end() do
  if lines[i]:match("^To:%s*$") then
    to_line = i
  end
end
if to_line then
  vim.api.nvim_win_set_cursor(0, { to_line, #lines[to_line] })
  vim.cmd("startinsert!")
else
  local body = math.min(header_end() + 2, #lines)
  vim.api.nvim_win_set_cursor(0, { body, 0 })
  vim.cmd("startinsert")
end
