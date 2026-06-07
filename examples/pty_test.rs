//! PTY-embed de-risk spike.
//!
//! Spawns $EDITOR attached to a pseudo-terminal, renders its live screen inside
//! a ratatui frame via tui-term, routes keystrokes to it, and prints the file
//! back after the editor exits. Proves the loop before wiring it into cells.
//!
//!   cargo run --bin pty_test
//!
//! Quit by quitting the editor (e.g. `:wq` in vi/helix).

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{cursor, execute};
use ratatui::layout::Position;
use ratatui::widgets::{Block, Borders};
use ratatui::Terminal;
use tui_term::vt100;
use tui_term::widget::PseudoTerminal;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> Result<()> {
    let tmp = tempfile::Builder::new()
        .prefix("epycell-pty-")
        .suffix(".py")
        .tempfile()?;
    std::fs::write(
        tmp.path(),
        "import numpy as np\n# edit me in your real editor, then save & quit (:wq)\nprint('hello from the embedded pty editor')\n",
    )?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let res = run(&mut terminal, &editor, tmp.path());

    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, cursor::Show)?;

    let content = std::fs::read_to_string(tmp.path()).unwrap_or_default();
    res?;
    println!("--- file after editing in {editor} ---\n{content}");
    Ok(())
}

fn run(terminal: &mut Tui, editor: &str, path: &std::path::Path) -> Result<()> {
    let sz = terminal.size()?;
    // inner area = full minus the 1-cell border on each side
    let mut cols = sz.width.saturating_sub(2).max(1);
    let mut rows = sz.height.saturating_sub(2).max(1);

    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

    // Build the editor command: split args (e.g. EDITOR="code -w"), then the file.
    let mut words = editor.split_whitespace();
    let prog = words.next().unwrap_or("vi");
    let mut cmd = CommandBuilder::new(prog);
    for w in words {
        cmd.arg(w);
    }
    cmd.arg(path);
    cmd.env("TERM", "xterm-256color");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair.slave.spawn_command(cmd).context("spawning editor in pty")?;
    drop(pair.slave); // let the master see EOF when the child exits

    let mut reader = pair.master.try_clone_reader()?;
    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));

    // Pump PTY output into the vt100 parser on a background thread.
    {
        let parser = Arc::clone(&parser);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => parser.lock().unwrap().process(&buf[..n]),
                }
            }
        });
    }

    let mut writer = pair.master.take_writer()?;

    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }

        terminal.draw(|f| {
            let area = f.area();
            let block = Block::default()
                .title(format!(" {editor} — live in a PTY · save & quit to finish "))
                .borders(Borders::ALL);
            let inner = block.inner(area);
            f.render_widget(&block, area);

            let guard = parser.lock().unwrap();
            let screen = guard.screen();
            f.render_widget(PseudoTerminal::new(screen), inner);
            if !screen.hide_cursor() {
                let (crow, ccol) = screen.cursor_position();
                f.set_cursor_position(Position::new(inner.x + ccol, inner.y + crow));
            }
        })?;

        // Re-fit the PTY if the window changed (recreate parser; editor repaints).
        let cur = terminal.size()?;
        let ncols = cur.width.saturating_sub(2).max(1);
        let nrows = cur.height.saturating_sub(2).max(1);
        if ncols != cols || nrows != rows {
            cols = ncols;
            rows = nrows;
            pair.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
            *parser.lock().unwrap() = vt100::Parser::new(rows, cols, 0);
        }

        if event::poll(Duration::from_millis(20))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let bytes = key_to_bytes(key.code, key.modifiers);
                    if !bytes.is_empty() {
                        writer.write_all(&bytes)?;
                        writer.flush()?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Translate a crossterm key into the bytes a terminal program expects on stdin.
fn key_to_bytes(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match code {
        KeyCode::Char(c) => {
            if ctrl {
                let up = c.to_ascii_uppercase() as u8;
                if (b'@'..=b'_').contains(&up) {
                    vec![up - b'@'] // Ctrl-A..Ctrl-_
                } else if c == '?' {
                    vec![0x7f]
                } else {
                    let mut b = [0u8; 4];
                    c.encode_utf8(&mut b).as_bytes().to_vec()
                }
            } else {
                let mut b = [0u8; 4];
                c.encode_utf8(&mut b).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        _ => vec![],
    }
}
