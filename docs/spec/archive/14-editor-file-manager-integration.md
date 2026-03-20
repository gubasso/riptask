> **[ARCHIVED]** — This spec was the design input for implementation. Code + tests are
> now the source of truth for this behavior. This document is retained as historical
> record only.
>
> - **Status:** archived
> - **Date archived:** 2026-03-18
> - **Source of truth:** code + tests
>
> | Concern | Source of Truth |
> |---------|----------------|
> | Editor/file manager keymaps | `lib/tsk/issue.sh (path/edit), tests/integration/issue_lifecycle.bats` |
>
> ---

# Editor & File Manager Integration

### Core interaction model

```
viewer (nvim/yazi)          tsk CLI              $TSK_REPO
      │                        │                      │
      │  cursor on WHL-041     │                      │
      │  <leader>tm            │                      │
      │ ─────────────────────► │                      │
      │                        │  patch frontmatter   │
      │                        │ ────────────────────►│
      │                        │  regenerate views/   │
      │                        │ ────────────────────►│
      │  reload()              │                      │
      │ ◄───────────────────── │                      │
      │  card in new lane      │                      │
```

**Viewer** handles navigation and selection. **`tsk`** handles all mutations. **Keymaps** are the thin bridge.

The issue ID is always recoverable from the view filename: `01-WHL-041.md` → `WHL-041`.

### Opening the board view

```bash
tsk board --open
# internally: ui.opener $XDG_CACHE_HOME/tsk/views/kanban/<resolved-board>/
# or: nvim -R ~/.cache/tsk/views/kanban/<resolved-board>/
```

Opened in **readonly mode** (`-R` in nvim). Prevents accidental direct edits to view copies (changes would be lost on regeneration). All mutations go through `tsk` keymaps.

### Neovim integration (oil.nvim / mini.files)

```lua
-- Helper: extract issue ID from view filename
local function tsk_id_from_path(path)
  return path:match("(%u+%-%d+)%.md$")
    or path:match("(LOCAL%-%w+)%.md$")
end

-- Helper: get path of file under cursor in oil/mini.files
local function current_file()
  -- works for both oil.nvim and mini.files
  return require("oil").get_cursor_entry()
    and require("oil").get_current_dir() .. require("oil").get_cursor_entry().name
end

-- Move issue to a new state
vim.keymap.set("n", "<leader>tm", function()
  local id = tsk_id_from_path(current_file())
  if not id then return end
  vim.ui.select(
    {"backlog", "todo", "in-progress", "review", "done"},
    { prompt = "Move " .. id .. " to state:" },
    function(state)
      if state then
        vim.fn.system("tsk move " .. id .. " " .. state)
        require("oil").reload()  -- or MiniFiles.refresh()
      end
    end
  )
end, { desc = "tsk: move issue" })

-- Open issue for editing (opens canonical file, not the view copy)
vim.keymap.set("n", "<leader>te", function()
  local id = tsk_id_from_path(current_file())
  if not id then return end
  local path = vim.fn.system("tsk path " .. id):gsub("\n", "")
  vim.cmd("edit " .. path)
end, { desc = "tsk: edit issue" })

-- Move up in lane
vim.keymap.set("n", "<leader>t]", function()
  local id = tsk_id_from_path(current_file())
  if not id then return end
  vim.fn.system("tsk reorder-up " .. id)
  require("oil").reload()
end, { desc = "tsk: move up in lane" })

-- Move down in lane
vim.keymap.set("n", "<leader>t[", function()
  local id = tsk_id_from_path(current_file())
  if not id then return end
  vim.fn.system("tsk reorder-down " .. id)
  require("oil").reload()
end, { desc = "tsk: move down in lane" })

-- Close issue
vim.keymap.set("n", "<leader>tc", function()
  local id = tsk_id_from_path(current_file())
  if not id then return end
  vim.fn.system("tsk close " .. id)
  require("oil").reload()
end, { desc = "tsk: close issue" })
```

Apply these keymaps only when inside the views directory (`$XDG_CACHE_HOME/tsk/views/`):

```lua
vim.api.nvim_create_autocmd("BufEnter", {
  pattern = (vim.env.XDG_CACHE_HOME or vim.env.HOME .. "/.cache") .. "/tsk/views/*",
  callback = function()
    -- set buffer-local keymaps here
  end
})
```

### Yazi integration

```toml
# ~/.config/yazi/keymap.toml

[[manager.prepend_keymap]]
on   = ["<leader>", "m"]
run  = '''shell '
  ID=$(basename "$0" | grep -oP "[A-Z]+-[0-9]+|LOCAL-[a-f0-9]+")
  STATE=$(printf "backlog\ntodo\nin-progress\nreview\ndone" | fzf --prompt "Move $ID to: ")
  [ -n "$STATE" ] && tsk move "$ID" "$STATE"
' --confirm'''
desc = "tsk: move issue"

[[manager.prepend_keymap]]
on   = ["<leader>", "e"]
run  = 'shell "tsk edit $(basename \"$0\" | grep -oP \"[A-Z]+-[0-9]+|LOCAL-[a-f0-9]+\")" --block'
desc = "tsk: edit issue"

[[manager.prepend_keymap]]
on   = ["<leader>", "c"]
run  = 'shell "tsk close $(basename \"$0\" | grep -oP \"[A-Z]+-[0-9]+|LOCAL-[a-f0-9]+\")"'
desc = "tsk: close issue"

[[manager.prepend_keymap]]
on   = ["<leader>", "]"]
run  = 'shell "tsk reorder-up $(basename \"$0\" | grep -oP \"[A-Z]+-[0-9]+|LOCAL-[a-f0-9]+\")"'
desc = "tsk: move up in lane"
```

### UI opener config

```yaml
# tsk.yaml
ui:
  opener: nvim -R    # or: yazi, lf, ranger, nnn
                     # default: nvim -R
```
