<p align="center">
  <img src="logo.png" width="200" alt="epycell logo">
</p>

# epycell

Fast Terminal Jupyter notebook written in Rust — your editor in every cell, kernel completions via LSP.

<!-- TODO: replace with your recorded GIF -->
![demo](https://raw.githubusercontent.com/TheSquake/epycell/main/demo.gif)

## Why

Jupyter belongs in the terminal. epycell gives you:

- **Your real editor** (helix/vim/emacs) embedded live in each cell — not a textarea widget
- **Kernel-backed LSP** — completions and hover docs from the running namespace, any language
- **Inline figures** — sixel, kitty graphics, iTerm2 (auto-detected, works everywhere)
- **Async execution** — run cells without blocking the UI, stream output in real-time
- **Single binary** — ~2500 lines of Rust, no electron, no browser, no Python runtime for the UI

## Install

**Prebuilt binary** (fastest — no compilation):
```
cargo binstall epycell
```

Or download from [GitHub Releases](https://github.com/TheSquake/epycell/releases).

**From source:**
```
cargo install epycell
```

Then run:
```
epycell --init    # creates ~/.config/epycell/ with default config + themes
```

### Requirements

A Python environment with `ipykernel` and `matplotlib`:

```
pip install ipykernel matplotlib
```

epycell auto-discovers your venv — it walks up from the notebook's directory looking for `.venv/`, `venv/`, or `.env/`. Or set `EPYCELL_PYTHON` / `VIRTUAL_ENV` explicitly.

The in-cell editor is your `$VISUAL` or `$EDITOR` (falls back to `vi`). Helix, Neovim, Vim, Emacs — whatever you have configured. The editor runs live inside the cell with full access to your config, plugins, and LSP.

## Usage

```
epycell notebook.ipynb     # open a notebook
epycell                    # scratch notebook (demo cells)
epycell --init             # install config + syntax themes
```

### Keybindings (default, all configurable)

| Key | Action |
|-----|--------|
| `j` / `k` | Move between cells |
| `gg` / `G` | Jump to first / last cell |
| `Enter` | Run cell |
| `i` | Edit cell in-place (your $EDITOR, live in the cell) |
| `e` | Edit cell full-screen |
| `Ctrl+r` | Run all cells |
| `Ctrl+a` | Run all cells above + selected |
| `Ctrl+c` | Interrupt running cell |
| `o` / `O` | New cell below / above |
| `yy` / `p` | Yank cell / paste below |
| `x` | Expand/collapse long output |
| `dd` | Delete cell |
| `w` | Save |
| `q` | Quit |

Mouse scroll and click-to-select work everywhere.

## The LSP trick

When you press `i` to edit a cell, epycell spawns `epycell-lsp` — a minimal LSP server that bridges your editor's completion/hover requests to the Jupyter kernel's live namespace via ZMQ. Your editor gets completions from variables, functions, and imports that you've *actually executed*, not static analysis.

Works with any editor that speaks LSP. Editor configs (`.helix/languages.toml`, `.nvim.lua`, `.dir-locals.el`) are auto-generated in the temp edit directory.

```
┌─────────┐     LSP (stdio)     ┌──────────────┐    ZMQ     ┌────────┐
│  Editor  │ ◄────────────────► │ epycell-lsp  │ ◄────────► │ Kernel │
└─────────┘                     └──────────────┘            └────────┘
```

## Configuration

`~/.config/epycell/config.toml` — fully optional, everything has sensible defaults.

```toml
[theme]
bg         = "#0d1926"
selected   = "#b1b9f9"
editing    = "#7ab87a"
inactive   = "#4a5a6a"
error      = "#b87a7a"
output     = "#7ab8b8"
status_nav = "#7a7ab8"
status_edit = "#7ab87a"

# Built-in: "base16-ocean.dark", "base16-eighties.dark", "Solarized (dark)", etc.
# Or path to any .tmTheme file:
syntax_theme = "base16-ocean.dark"

[images]
# All values are 0 (unconstrained) by default
max_width = 80    # max columns (0 = fit to cell width)
max_height = 25   # max rows (0 = no cap)
min_width = 20    # min columns (0 = no minimum)
min_height = 5    # min rows (0 = no minimum)

[keys]
run       = "Enter"
edit      = "i"
quit      = "q"
# ... see `epycell --init` for all options
```

### Bundled syntax themes

`epycell --init` installs: Dracula, Gruvbox Dark, Nord, Tokyo Night, Catppuccin Mocha, One Dark, and aidsDick.

## vs. alternatives

| | epycell | euporie | jupyter-console | vim plugins |
|---|---|---|---|---|
| Your editor config | ✓ | ✗ (built-in widget) | ✗ | partial |
| Kernel LSP | ✓ | custom completions | ✗ | ✗ |
| Inline figures | ✓ (any protocol) | ✓ (sixel) | ✗ | ✗ |
| Editor-agnostic | ✓ | n/a | n/a | ✗ |
| Single binary | ✓ | ✗ (Python) | ✗ (Python) | ✗ |

## License

GPL-3.0
