<p align="center">
  <img src="logo.png" width="200" alt="epycell logo">
</p>

<h1 align="center">epycell</h1>

<p align="center">
  <strong>Jupyter notebooks belong in the terminal.</strong><br>
  Your editor. Your keybindings. Your workflow. Inline figures. One binary.
</p>

<p align="center">
  <a href="https://crates.io/crates/epycell"><img src="https://img.shields.io/crates/v/epycell.svg" alt="crates.io"></a>
  <a href="https://github.com/TheSquake/epycell/releases"><img src="https://img.shields.io/github/v/release/TheSquake/epycell" alt="release"></a>
  <a href="https://github.com/TheSquake/epycell/blob/main/LICENSE"><img src="https://img.shields.io/github/license/TheSquake/epycell" alt="license"></a>
</p>

---

![demo](https://raw.githubusercontent.com/TheSquake/epycell/main/demo.gif)

---

## What is this?

epycell embeds your **real editor** (Helix, Neovim, Vim, Emacs — whatever `$EDITOR` is) live inside each notebook cell. Not a textarea. Not a reimplementation. The actual editor, with your config, your plugins, your muscle memory.

The kernel gives your editor **completions from the running namespace** via LSP — not static analysis, but the actual variables and functions you've executed. Plots render inline as sixel/kitty/iTerm2 graphics. Everything stays in the terminal.

**~2500 lines of Rust. No Electron. No browser. No Python runtime for the UI.**

## Install

```bash
# Prebuilt binary (no compilation, ~3 seconds)
cargo binstall epycell

# Or from source
cargo install epycell

# Then grab the default config + syntax themes
epycell --init
```

Or grab a binary directly from [Releases](https://github.com/TheSquake/epycell/releases).

### Requirements

```bash
pip install ipykernel matplotlib
```

epycell auto-discovers your venv (walks up looking for `.venv/`, `venv/`, `.env/`). Or set `EPYCELL_PYTHON` / `VIRTUAL_ENV`.

## Quick start

```bash
epycell notebook.ipynb     # open existing notebook
epycell                    # scratch pad with demo cells
```

Press `i` to drop into your editor. Write code. Save & quit. Press `Enter` to run. That's it.

## Keybindings

Vim-native by default. All configurable in `~/.config/epycell/config.toml`.

| Key | Action |
|-----|--------|
| `j` / `k` | Move between cells |
| `gg` / `G` | Jump to first / last cell |
| `i` | Edit cell (your $EDITOR, live in the cell) |
| `e` | Edit cell full-screen |
| `Enter` | Run cell |
| `Ctrl+r` | Run all cells |
| `Ctrl+a` | Run cells above + selected |
| `Ctrl+c` | Interrupt |
| `o` / `O` | New cell below / above |
| `yy` / `p` | Yank / paste cell |
| `Y` | Copy cell source to system clipboard |
| `?` | Ask Claude Code about the focused cell |
| `dd` | Delete cell |
| `x` | Expand/collapse output |
| `w` / `q` | Save / quit |

Mouse scroll and click work too.

## The LSP trick

When you edit a cell, epycell spawns a tiny LSP server that bridges your editor to the Jupyter kernel's live namespace over ZMQ. Your editor gets completions for variables that *actually exist at runtime* — not guesses from static analysis.

```
┌─────────┐     stdio      ┌──────────────┐     ZMQ      ┌────────┐
│  Editor  │ ◄───────────► │ epycell-lsp  │ ◄──────────► │ Kernel │
└─────────┘                └──────────────┘              └────────┘
```

Works with any LSP-capable editor. Configs for Helix, Neovim, and Emacs are auto-generated.

## Configuration

Everything is optional — epycell works out of the box.

```toml
# Terminal for spawning new windows (? key). Falls back to $TERMINAL, then "foot".
# terminal = "foot"

[theme]
selected    = "#b1b9f9"
editing     = "#7ab87a"
syntax_theme = "base16-ocean.dark"   # or path to any .tmTheme

[images]
max_width  = 80   # 0 = fit to cell width
max_height = 25   # 0 = no cap
min_width  = 0    # floor for narrow terminals
min_height = 0

[keys]
run       = "Enter"
edit      = "i"
move_down = "j, Down"
# ... see epycell --init for all options
```

Bundled syntax themes: Dracula, Gruvbox Dark, Nord, Tokyo Night, Catppuccin Mocha, One Dark, aidsDick.

## Built-in AI assistant

Press `?` on any cell to open an interactive [Claude Code](https://claude.ai/claude-code) session in a new terminal window — already loaded with your full notebook context.

Claude sees which cell you're focused on **in real time**. Navigate to a different cell, ask "what about this one?" — it reads the live state without you having to copy-paste anything.

```
┌──────────────────────────────────────────────┐
│ epycell                                      │
│ ┌──────────────────────────────────────────┐ │
│ │ [3] x = np.fft.fft(signal)              │◄──── you're focused here
│ └──────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
        │  ?
        ▼
┌──────────────────────────────────────────────┐
│ claude (interactive)                         │
│                                              │
│ ❯ explain this cell                         │
│                                              │
│ ● This applies a Fast Fourier Transform...  │
└──────────────────────────────────────────────┘
```

Requires `claude` CLI installed. Set `terminal = "kitty"` (or any terminal) in config if you're not on foot.

## Why not X?

| | epycell | euporie | jupyter-console | vim plugins |
|---|---|---|---|---|
| Your real editor | **yes** | no (widget) | no | partial |
| Kernel LSP | **yes** | custom | no | no |
| Inline figures | **any protocol** | sixel only | no | no |
| AI assistant (live context) | **yes** | no | no | no |
| Editor-agnostic | **yes** | n/a | n/a | no |
| Single binary | **yes** | Python | Python | no |

## License

GPL-3.0
