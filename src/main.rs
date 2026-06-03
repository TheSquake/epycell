//! epycell — a terminal Jupyter notebook with inline figures.
//!
//! Modal:
//!   j / k or ↓ / ↑   move between cells
//!   i or Enter        edit the focused cell with a LIVE $EDITOR in the cell
//!   e                 edit the focused cell in a full-screen $EDITOR
//!   R                 run focused cell
//!   o / O             new cell below / above
//!   dd                delete focused cell
//!   w                 save · q quit
//!
//! In-cell editing embeds the real editor (Helix/vim/…) in a PTY rendered
//! inside the focused cell. Save & quit the editor to return to the notebook.

mod config;
mod kernel;
mod nb;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use kernel::{default_kernel_python, CellEvent, KernelSession, Output};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind, EnableMouseCapture, DisableMouseCapture};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{execute, cursor};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Modifier, Style};

use config::Config;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use ratatui_image::picker::Picker;
use ratatui_image::{Image, Resize};
use tui_term::vt100;
use tui_term::widget::PseudoTerminal;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Rendered output ready to draw.
enum OutputView {
    Text { text: String, style: Style },
    Image { protocol: ratatui_image::protocol::Protocol, rows: u16 },
}

impl OutputView {
    fn rows(&self) -> u16 {
        match self {
            OutputView::Text { text, .. } => text.lines().count().max(1) as u16,
            OutputView::Image { rows, .. } => *rows,
        }
    }
}

const OUTPUT_CAP: u16 = 50;

struct Cell {
    source: String,
    outputs: Vec<OutputView>,
    exec_count: Option<usize>,
    running: bool,
    markdown: bool,
    output_expanded: bool,
    output_scroll: u16, // lines scrolled within the output
}

impl Cell {
    fn new(source: &str) -> Self {
        Self {
            source: source.to_string(),
            outputs: Vec::new(),
            exec_count: None,
            running: false,
            markdown: false,
            output_expanded: false,
            output_scroll: 0,
        }
    }

    fn markdown(source: &str) -> Self {
        Self {
            markdown: true,
            ..Cell::new(source)
        }
    }

    fn source_rows(&self) -> u16 {
        self.source.split('\n').count().max(1) as u16
    }

    /// How many output blocks are visible, and their total row count.
    /// Returns (number of blocks to draw, total rows, is_truncated).
    fn visible_output_layout(&self) -> (usize, u16, bool) {
        let total: u16 = self.outputs.iter().map(|o| o.rows()).sum();
        if self.output_expanded || total <= OUTPUT_CAP {
            (self.outputs.len(), total, false)
        } else {
            let mut used: u16 = 0;
            let mut count = 0;
            for o in &self.outputs {
                let h = o.rows();
                if used + h > OUTPUT_CAP {
                    break;
                }
                used += h;
                count += 1;
            }
            (count, used, true)
        }
    }
}

/// Live in-cell editor session: which cell, its vt100 screen, and the source-box
/// height (incl. borders) to give it while editing.
struct EditState {
    idx: usize,
    parser: Arc<Mutex<vt100::Parser>>,
    rows: u16,
}

struct App {
    cells: Vec<Cell>,
    selected: usize,
    scroll_top: usize,    // first visible cell index
    scroll_offset: u16,   // lines to skip within scroll_top cell (for smooth scroll)
    status: String,
    picker: Picker,
    exec_counter: usize,
    pending_d: bool, // for the `dd` chord
    path: Option<PathBuf>,
    dirty: bool,        // unsaved source changes
    pending_quit: bool, // showing the save-before-quit prompt
    edit: Option<EditState>, // a cell is being edited live in a PTY
    running_cell: Option<RunningCell>, // async cell execution in progress
    run_queue: Vec<usize>,             // queued cell indices to run next
    free_scroll: bool,                 // mouse scroll detached from selection
    cfg: Config,
}

struct RunningCell {
    idx: usize,
    msg_id: String,
    exec_num: usize,
}

fn demo_cells() -> Vec<Cell> {
    vec![
        Cell::new("import numpy as np\nimport matplotlib.pyplot as plt\nprint('ready')"),
        Cell::new("x = np.linspace(0, 6.28, 300)\nplt.figure()\nplt.plot(x, np.sin(x), label='sin')\nplt.plot(x, np.cos(x), label='cos')\nplt.legend()\nplt.title('epycell')\nprint('figure shows even with a trailing print now')"),
    ]
}

impl App {
    fn new(picker: Picker, cells: Vec<Cell>, path: Option<PathBuf>, cfg: Config) -> Self {
        let opened = path
            .as_ref()
            .map(|p| format!("opened {}", p.display()))
            .unwrap_or_default();
        Self {
            cells: if cells.is_empty() { vec![Cell::new("")] } else { cells },
            selected: 0,
            scroll_top: 0,
            scroll_offset: 0,
            status: format!("{}   {opened}", cfg.keys.help_line()),
            picker,
            exec_counter: 0,
            pending_d: false,
            path,
            dirty: false,
            pending_quit: false,
            edit: None,
            running_cell: None,
            run_queue: Vec::new(),
            free_scroll: false,
            cfg,
        }
    }

    /// Save to disk. Returns true on success.
    fn save(&mut self) -> bool {
        let path = self
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("untitled.ipynb"));
        let cells: Vec<nb::NbCell> = self
            .cells
            .iter()
            .map(|c| nb::NbCell {
                source: c.source.clone(),
                markdown: c.markdown,
            })
            .collect();
        match nb::save(&path, &cells) {
            Ok(()) => {
                self.status = format!("saved {}", path.display());
                self.path = Some(path);
                self.dirty = false;
                true
            }
            Err(e) => {
                self.status = format!("save failed: {e}");
                false
            }
        }
    }

    /// Effective rendered height of a cell, accounting for live-edit expansion.
    fn cell_height(&self, idx: usize) -> u16 {
        let cell = &self.cells[idx];
        let (_, out_rows, truncated) = cell.visible_output_layout();
        let visible_out = out_rows.saturating_sub(cell.output_scroll);
        let out = visible_out + if truncated { 1 } else { 0 }; // +1 for indicator
        let src = match &self.edit {
            Some(e) if e.idx == idx => e.rows,
            _ if cell.markdown => markdown_rows(&cell.source) + 2,
            _ => cell.source_rows() + 2,
        };
        1 + src + out
    }

    /// Build image protocols for any PNG outputs, fitting width to a cap.
    fn render_outputs(&self, raw: Vec<Output>) -> Vec<OutputView> {
        let fs = self.picker.font_size();
        let mut views = Vec::new();
        for o in raw {
            match o {
                Output::Stream { name, text } => {
                    let color = if name.contains("Stderr") { self.cfg.theme.error } else { self.cfg.theme.inactive };
                    views.push(OutputView::Text { text, style: Style::default().fg(color) });
                }
                Output::Text(text) => {
                    views.push(OutputView::Text { text, style: Style::default().fg(self.cfg.theme.output) });
                }
                Output::Error { ename, evalue, traceback } => {
                    let text = if traceback.is_empty() {
                        format!("{ename}: {evalue}")
                    } else {
                        strip_ansi(&traceback.join("\n"))
                    };
                    views.push(OutputView::Text { text, style: Style::default().fg(self.cfg.theme.error) });
                }
                Output::Png(bytes) => match image::load_from_memory(&bytes) {
                    Ok(img) => {
                        let max_w: u32 = 80;
                        // Compute rows from the image aspect ratio and available width.
                        // The image will be scaled to fit `w` columns wide, so height
                        // in rows = (img_h / img_w) * w * (font_w / font_h).
                        let w = (img.width().div_ceil(fs.width as u32)).min(max_w).max(1) as u16;
                        let h = ((img.height() as f64 / img.width() as f64)
                            * w as f64
                            * (fs.width as f64 / fs.height as f64))
                            .ceil() as u16;
                        let h = h.max(1);
                        match self.picker.new_protocol(img.clone(), Size::new(w, h), Resize::Fit(None)) {
                            Ok(protocol) => views.push(OutputView::Image { protocol, rows: h }),
                            Err(e) => views.push(OutputView::Text {
                                text: format!("<image encode error: {e}>"),
                                style: Style::default().fg(self.cfg.theme.error),
                            }),
                        }
                    }
                    Err(e) => views.push(OutputView::Text {
                        text: format!("<png decode error: {e}>"),
                        style: Style::default().fg(self.cfg.theme.error),
                    }),
                },
            }
        }
        views
    }

    fn ensure_visible(&mut self, area_height: u16) {
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
            return;
        }

        // If selected cell is expanded + running, pin viewport so the bottom
        // of the cell is visible (follow output as it grows).
        let cell = &self.cells[self.selected];
        if cell.output_expanded && cell.running {
            // Sum heights from scroll_top to bottom of selected cell
            loop {
                let mut y = 0u16;
                for idx in self.scroll_top..=self.selected {
                    y += self.cell_height(idx) + 1;
                }
                if y <= area_height || self.scroll_top >= self.selected {
                    break;
                }
                self.scroll_top += 1;
            }
            return;
        }

        // Walk down from scroll_top; if selected doesn't fit, advance scroll_top.
        loop {
            let mut y = 0u16;
            let mut last_visible = self.scroll_top;
            for idx in self.scroll_top..self.cells.len() {
                let h = self.cell_height(idx);
                if y + h > area_height {
                    break;
                }
                y += h + 1;
                last_visible = idx;
            }
            if self.selected <= last_visible || self.scroll_top >= self.selected {
                break;
            }
            self.scroll_top += 1;
        }
    }
}

/// A centered rect of the given cell size, clamped to `area`.
fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Render markdown source to styled ratatui text (headings, bold, lists, …).
fn markdown_text(source: &str) -> ratatui::text::Text<'_> {
    tui_markdown::from_str(source)
}

/// Rendered height (in rows) of a markdown cell's source.
fn markdown_rows(source: &str) -> u16 {
    (markdown_text(source).height() as u16).max(1)
}

/// Syntax-highlight source code into styled ratatui Text.
fn highlight_code(source: &str, theme: &config::Theme) -> ratatui::text::Text<'static> {
    use syntect::highlighting::{Theme as SynTheme, ThemeSet};
    use syntect::parsing::SyntaxSet;
    use syntect::easy::HighlightLines;
    use std::sync::OnceLock;

    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();
    static CUSTOM_THEME: OnceLock<Option<SynTheme>> = OnceLock::new();

    let ss = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines);
    let ts = THEME_SET.get_or_init(ThemeSet::load_defaults);

    let custom = CUSTOM_THEME.get_or_init(|| {
        let name = &theme.syntax_theme;
        // If it's a path to a .tmTheme file, load it
        if name.ends_with(".tmTheme") || name.contains('/') {
            ThemeSet::get_theme(name).ok()
        } else {
            None
        }
    });

    let syntax = ss.find_syntax_by_extension("py").unwrap_or_else(|| ss.find_syntax_plain_text());
    let syn_theme = custom.as_ref()
        .unwrap_or_else(|| {
            ts.themes.get(&theme.syntax_theme)
                .unwrap_or_else(|| ts.themes.get("base16-ocean.dark").unwrap())
        });

    let mut h = HighlightLines::new(syntax, syn_theme);
    let mut lines: Vec<ratatui::text::Line<'static>> = Vec::new();

    for line in source.lines() {
        let line_with_nl = format!("{line}\n");
        let ranges = h.highlight_line(&line_with_nl, ss).unwrap_or_default();
        let spans: Vec<Span<'static>> = ranges.into_iter().map(|(style, text)| {
            let fg = ratatui::style::Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
            Span::styled(text.trim_end_matches('\n').to_string(), Style::default().fg(fg))
        }).collect();
        lines.push(ratatui::text::Line::from(spans));
    }

    if lines.is_empty() {
        lines.push(ratatui::text::Line::from(""));
    }

    ratatui::text::Text::from(lines)
}


/// When expanded, scroll output so the latest lines are visible.
fn follow_output(cell: &mut Cell) {
    if !cell.output_expanded {
        return;
    }
    let (_, out_rows, _) = cell.visible_output_layout();
    // Keep OUTPUT_CAP lines visible at the bottom
    cell.output_scroll = out_rows.saturating_sub(OUTPUT_CAP);
}

/// Append `new` to `buf`, handling \r (carriage return) by overwriting from
/// the start of the current line — this makes progress bars update in place.
fn apply_cr(buf: &mut String, new: &str) {
    for part in new.split('\r') {
        if part.is_empty() {
            // bare \r: truncate to last newline (reset current line)
            if let Some(nl) = buf.rfind('\n') {
                buf.truncate(nl + 1);
            } else {
                buf.clear();
            }
        } else {
            // If this part starts with content right after a \r, the split
            // already truncated above. Just append.
            buf.push_str(part);
        }
    }
}

fn strip_ansi(s: &str) -> String {
    // crude CSI stripper for tracebacks
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            while let Some(&n) = chars.peek() {
                chars.next();
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    // bottom status bar
    let body = Rect { height: area.height.saturating_sub(1), ..area };
    let status_area = Rect { y: area.bottom() - 1, height: 1, ..area };

    let mut y: i32 = -(app.scroll_offset as i32);
    for idx in app.scroll_top..app.cells.len() {
        let h = app.cell_height(idx) as i32;
        if y >= body.bottom() as i32 {
            break;
        }
        let cell_bottom = y + h;
        if cell_bottom > 0 {
            let draw_y = y.max(0) as u16;
            let draw_h = (cell_bottom.min(body.bottom() as i32) - draw_y as i32) as u16;
            let clip_top = (0 - y).max(0) as u16;
            if draw_h > 0 {
                let cell_rect = Rect { x: body.x, y: draw_y, width: body.width, height: draw_h };
                draw_cell(f, app, idx, &app.cells[idx], cell_rect, clip_top);
            }
        }
        y += h + 1;
    }

    let editing = app.edit.is_some();
    let status = Paragraph::new(Line::from(vec![
        Span::styled(
            if editing { " EDIT " } else { " NAV " },
            Style::default()
                .bg(if editing { app.cfg.theme.status_edit } else { app.cfg.theme.status_nav })
                .fg(app.cfg.theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(&app.status, Style::default().fg(app.cfg.theme.inactive)),
    ]));
    f.render_widget(status, status_area);

    if app.pending_quit {
        let rect = centered_rect(area, 56, 6);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .title(" Unsaved changes ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.cfg.theme.error).add_modifier(Modifier::BOLD));
        let body = Paragraph::new(vec![
            Line::from(""),
            Line::from("  Save before quitting?"),
            Line::from(""),
            Line::from(vec![
                Span::raw("    "),
                Span::styled("y", Style::default().fg(app.cfg.theme.editing).add_modifier(Modifier::BOLD)),
                Span::raw(" save & quit    "),
                Span::styled("n", Style::default().fg(app.cfg.theme.selected).add_modifier(Modifier::BOLD)),
                Span::raw(" quit    "),
                Span::styled("Esc", Style::default().fg(app.cfg.theme.inactive)),
                Span::raw(" cancel"),
            ]),
        ])
        .block(block);
        f.render_widget(body, rect);
    }
}

fn draw_cell(f: &mut Frame, app: &App, idx: usize, cell: &Cell, area: Rect, clip_top: u16) {
    let selected = idx == app.selected;
    let editing = app.edit.as_ref().map(|e| e.idx) == Some(idx);

    // header (skip if clipped)
    let count = cell.exec_count.map(|c| c.to_string()).unwrap_or_else(|| " ".into());
    let marker = if cell.running { "*" } else { &count };
    let header_style = if selected {
        Style::default().fg(app.cfg.theme.selected).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.cfg.theme.inactive)
    };
    let label = if cell.markdown {
        "Markdown:".to_string()
    } else {
        format!("In [{marker}]:")
    };
    if clip_top == 0 {
        let header = Rect { height: 1, ..area };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(label, header_style))),
            header,
        );
    }

    // source block
    let header_visible = if clip_top == 0 { 1u16 } else { 0u16 };
    let src_h = match &app.edit {
        Some(e) if e.idx == idx => e.rows,
        _ if cell.markdown => markdown_rows(&cell.source) + 2,
        _ => cell.source_rows() + 2,
    };
    let src_clip = clip_top.saturating_sub(1); // lines clipped from source area
    let src_visible = src_h.saturating_sub(src_clip);
    let src_rect = Rect {
        y: area.y + header_visible,
        height: src_visible.min(area.height.saturating_sub(header_visible)),
        ..area
    };
    let border_color = if editing {
        app.cfg.theme.editing
    } else if selected {
        app.cfg.theme.selected
    } else {
        app.cfg.theme.inactive
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    if editing {
        let inner = block.inner(src_rect);
        f.render_widget(&block, src_rect);
        let guard = app.edit.as_ref().unwrap().parser.lock().unwrap();
        let screen = guard.screen();
        f.render_widget(PseudoTerminal::new(screen), inner);
        if !screen.hide_cursor() {
            let (crow, ccol) = screen.cursor_position();
            f.set_cursor_position(Position::new(inner.x + ccol, inner.y + crow));
        }
    } else if cell.markdown {
        f.render_widget(
            Paragraph::new(markdown_text(&cell.source))
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((src_clip, 0)),
            src_rect,
        );
    } else {
        f.render_widget(
            Paragraph::new(highlight_code(&cell.source, &app.cfg.theme))
                .block(block)
                .scroll((src_clip, 0)),
            src_rect,
        );
    }

    // outputs — use the shared layout to determine what's visible
    let mut oy = src_rect.bottom();
    let (block_count, out_rows, truncated) = cell.visible_output_layout();
    let scroll = cell.output_scroll.min(out_rows.saturating_sub(1));
    let mut skipped: u16 = 0;
    let avail_h = area.bottom().saturating_sub(oy);

    for out in cell.outputs.iter().take(block_count) {
        if oy >= area.bottom() {
            break;
        }
        let out_h = out.rows();
        if skipped + out_h <= scroll {
            skipped += out_h;
            continue;
        }
        let skip_in_block = scroll.saturating_sub(skipped);
        skipped = scroll;

        let draw_h = (out_h - skip_in_block).min(area.bottom() - oy);
        let orect = Rect { x: area.x + 1, y: oy, width: area.width.saturating_sub(1), height: draw_h };
        match out {
            OutputView::Text { text, style } => {
                f.render_widget(
                    Paragraph::new(text.as_str())
                        .style(*style)
                        .wrap(Wrap { trim: false })
                        .scroll((skip_in_block, 0)),
                    orect,
                );
            }
            OutputView::Image { protocol, .. } => {
                if skip_in_block == 0 {
                    f.render_widget(Image::new(protocol), orect);
                }
            }
        }
        oy += draw_h;
    }

    // Show truncation indicator
    if truncated && oy < area.bottom() {
        let total: u16 = cell.outputs.iter().map(|o| o.rows()).sum();
        let hidden = total - out_rows;
        let indicator = format!("▼ {} more lines (x to expand)", hidden);
        let irect = Rect { x: area.x + 1, y: oy, width: area.width.saturating_sub(1), height: 1 };
        f.render_widget(
            Paragraph::new(indicator).style(Style::default().fg(app.cfg.theme.inactive)),
            irect,
        );
    }

    // Scroll position indicator when scrolled
    if scroll > 0 && oy <= area.bottom() && oy > src_rect.bottom() {
        let pos = format!("[{}/{}]", scroll + avail_h.min(out_rows), out_rows);
        let px = area.right().saturating_sub(pos.len() as u16 + 1);
        let prect = Rect { x: px, y: src_rect.bottom(), width: pos.len() as u16, height: 1 };
        f.render_widget(
            Paragraph::new(pos).style(Style::default().fg(app.cfg.theme.inactive)),
            prect,
        );
    }
}

/// Resolve the user's terminal editor: $VISUAL, then $EDITOR, then `vi`.
fn editor_command() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

/// Suspend the TUI, open the cell in the user's $EDITOR, read it back.
fn external_edit(terminal: &mut Tui, source: &str, conn_file: &Path) -> Result<String> {
    let tmp_dir = tempfile::Builder::new()
        .prefix("epycell-edit-")
        .tempdir()?;
    let cell_file = tmp_dir.path().join("cell.py");
    std::fs::write(&cell_file, source)?;

    let lsp_bin = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("epycell-lsp"))
        .with_file_name("epycell-lsp");

    // Helix config
    let helix_dir = tmp_dir.path().join(".helix");
    std::fs::create_dir_all(&helix_dir)?;
    std::fs::write(
        helix_dir.join("languages.toml"),
        format!(
            r#"[language-server.epycell-lsp]
command = "{}"
args = ["{}"]

[[language]]
name = "python"
language-servers = ["epycell-lsp"]
"#,
            lsp_bin.display(),
            conn_file.display()
        ),
    )?;

    // hand the terminal over to the editor
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, cursor::Show)?;

    let editor = editor_command();
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(prog)
        .args(parts)
        .arg(&cell_file)
        .current_dir(tmp_dir.path())
        .status()
        .with_context(|| format!("launching editor '{editor}'"))?;

    // take it back
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    if !status.success() {
        return Ok(source.to_string());
    }
    Ok(std::fs::read_to_string(&cell_file)?)
}

async fn start_cell_run(app: &mut App, session: &mut KernelSession) -> Result<()> {
    start_cell_run_at(app, session, app.selected).await
}

async fn start_cell_run_at(app: &mut App, session: &mut KernelSession, idx: usize) -> Result<()> {
    if app.running_cell.is_some() {
        app.status = "a cell is already running".into();
        return Ok(());
    }
    let code = app.cells[idx].source.clone();
    app.exec_counter += 1;
    let n = app.exec_counter;
    app.cells[idx].running = true;
    app.cells[idx].outputs.clear();
    app.cells[idx].exec_count = Some(n);
    app.status = format!("running cell [{n}]…");

    let msg_id = session.start_cell(&code).await?;
    app.running_cell = Some(RunningCell { idx, msg_id, exec_num: n });
    Ok(())
}

async fn poll_running_cell(app: &mut App, session: &mut KernelSession) -> Result<()> {
    // If nothing running but queue has items, start the next one.
    if app.running_cell.is_none() {
        if let Some(idx) = app.run_queue.first().copied() {
            app.run_queue.remove(0);
            start_cell_run_at(app, session, idx).await?;
        }
        return Ok(());
    }

    let rc = app.running_cell.as_ref().unwrap();
    let idx = rc.idx;
    let msg_id = rc.msg_id.clone();
    let n = rc.exec_num;

    match session.poll_output(&msg_id).await? {
        Some(CellEvent::Output(Output::Stream { name, text })) => {
            let color = if name.contains("Stderr") { app.cfg.theme.error } else { app.cfg.theme.inactive };
            let style = Style::default().fg(color);
            // Merge into the last stream output, handling \r for in-place updates.
            let merged = match app.cells[idx].outputs.last_mut() {
                Some(OutputView::Text { text: existing, style: s }) if *s == style => {
                    apply_cr(existing, &text);
                    true
                }
                _ => false,
            };
            if !merged {
                let mut buf = String::new();
                apply_cr(&mut buf, &text);
                app.cells[idx].outputs.push(OutputView::Text { text: buf, style });
            }
            follow_output(&mut app.cells[idx]);
        }
        Some(CellEvent::Output(raw)) => {
            let views = app.render_outputs(vec![raw]);
            app.cells[idx].outputs.extend(views);
            follow_output(&mut app.cells[idx]);
        }
        Some(CellEvent::Idle) => {
            app.cells[idx].running = false;
            app.status = format!("cell [{n}] done");
            app.running_cell = None;
        }
        None => {}
    }
    Ok(())
}

/// Compute the source-box height (incl. borders) to give the live editor.
fn edit_box_rows(body_height: u16, source_lines: u16) -> u16 {
    let content = source_lines + 2 + 3; // +2 borders, +3 for editor status/ruler
    let max = body_height.saturating_sub(4);
    content.clamp(6, max)
}

/// Edit a cell with a real editor running live, embedded in the cell's box.
/// Blocks (modal) until the editor process exits, then reads the file back.
/// Sets up epycell-lsp editor configs in the temp dir for completion/hover.
fn edit_cell_pty(terminal: &mut Tui, app: &mut App, idx: usize, conn_file: &Path) -> Result<()> {
    let tmp_dir = tempfile::Builder::new()
        .prefix("epycell-edit-")
        .tempdir()?;
    let cell_file = tmp_dir.path().join("cell.py");
    std::fs::write(&cell_file, &app.cells[idx].source)?;

    let lsp_bin = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("epycell-lsp"))
        .with_file_name("epycell-lsp");
    let lsp_cmd = format!("{} {}", lsp_bin.display(), conn_file.display());

    // Helix config
    let helix_dir = tmp_dir.path().join(".helix");
    std::fs::create_dir_all(&helix_dir)?;
    std::fs::write(
        helix_dir.join("languages.toml"),
        format!(
            r#"[language-server.epycell-lsp]
command = "{}"
args = ["{}"]

[[language]]
name = "python"
language-servers = ["epycell-lsp"]
"#,
            lsp_bin.display(),
            conn_file.display()
        ),
    )?;

    // Neovim config
    std::fs::write(
        tmp_dir.path().join(".nvim.lua"),
        format!(
            r#"vim.api.nvim_create_autocmd("FileType", {{
  pattern = "python",
  callback = function()
    vim.lsp.start({{
      name = "epycell-lsp",
      cmd = {{ "{}", "{}" }},
    }})
  end,
}})
"#,
            lsp_bin.display(),
            conn_file.display()
        ),
    )?;

    // Emacs config
    std::fs::write(
        tmp_dir.path().join(".dir-locals.el"),
        format!(
            r#"((python-mode . ((eglot-server-programs . ((python-mode . ("{}")))))))
"#,
            lsp_cmd
        ),
    )?;


    let source_lines = app.cells[idx].source_rows();
    let sz = terminal.size()?;
    let mut edit_rows = edit_box_rows(sz.height.saturating_sub(1), source_lines);
    let mut cols = sz.width.saturating_sub(2).max(1);
    let mut rows = edit_rows.saturating_sub(2).max(1);

    let editor = editor_command();
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
    let mut words = editor.split_whitespace();
    let prog = words.next().unwrap_or("vi");
    let mut cmd = CommandBuilder::new(prog);
    for w in words {
        cmd.arg(w);
    }
    cmd.arg(&cell_file);
    cmd.env("TERM", "xterm-256color");
    cmd.cwd(tmp_dir.path());
    let mut child = pair.slave.spawn_command(cmd).context("spawning editor in pty")?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
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

    // Pin the editing cell to the top so its box rect is deterministic.
    app.scroll_top = idx;
    app.edit = Some(EditState { idx, parser: Arc::clone(&parser), rows: edit_rows });
    app.status = format!("editing in {editor} — save & quit to return");

    let res = (|| -> Result<()> {
        loop {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            terminal.draw(|f| draw(f, app))?;

            let cur = terminal.size()?;
            let nedit_rows = edit_box_rows(cur.height.saturating_sub(1), source_lines);
            let ncols = cur.width.saturating_sub(2).max(1);
            let nrows = nedit_rows.saturating_sub(2).max(1);
            if ncols != cols || nrows != rows {
                cols = ncols;
                rows = nrows;
                edit_rows = nedit_rows;
                pair.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
                *parser.lock().unwrap() = vt100::Parser::new(rows, cols, 0);
                if let Some(e) = app.edit.as_mut() {
                    e.rows = edit_rows;
                }
            }

            if event::poll(Duration::from_millis(20))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let bytes = key_to_bytes(key.code, key.modifiers);
                        if !bytes.is_empty() {
                            writer.write_all(&bytes)?;
                            writer.flush()?;
                        }
                    }
                    Event::Mouse(mouse) => {
                        match mouse.kind {
                            MouseEventKind::ScrollDown => {
                                if app.scroll_top + 1 < app.cells.len() {
                                    app.scroll_top += 1;
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                if app.scroll_top > 0 {
                                    app.scroll_top -= 1;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    app.edit = None;
    terminal.clear()?; // editor may have left artifacts; force a full repaint
    res?;

    // Read the edited file back; editors typically add a trailing newline.
    let mut new_src = std::fs::read_to_string(&cell_file)?;
    if new_src.ends_with('\n') {
        new_src.pop();
    }
    if new_src != app.cells[idx].source {
        app.cells[idx].source = new_src;
        app.dirty = true;
    }
    app.status = format!("edited cell {} in {editor}", idx + 1);
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
                    vec![up - b'@']
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

/// Headless check (no TUI): confirm a cell with a trailing print STILL emits a
/// figure once inline mode is on. Prints the output kinds and exits non-zero on
/// failure.
async fn selftest() -> Result<()> {
    let python = default_kernel_python();
    let mut session = KernelSession::launch(&python).await?;
    let outputs = session
        .run_cell(
            "import numpy as np\nimport matplotlib.pyplot as plt\n\
             plt.figure(); plt.plot(np.arange(10))\n\
             print('trailing print, no plt.gcf() as last expr')",
        )
        .await?;
    session.shutdown().await;

    let mut has_stream = false;
    let mut has_png = false;
    for o in &outputs {
        match o {
            Output::Stream { .. } => has_stream = true,
            Output::Png(b) => {
                has_png = true;
                println!("  png output: {} bytes", b.len());
            }
            Output::Text(t) => println!("  text: {t}"),
            Output::Error { ename, evalue, .. } => println!("  error: {ename}: {evalue}"),
        }
    }
    println!("stream(print)={has_stream}  figure(png)={has_png}");
    if has_stream && has_png {
        println!("SELFTEST PASS — figure flushes even with a trailing print");
        Ok(())
    } else {
        anyhow::bail!("SELFTEST FAIL — expected both a print stream and a png figure");
    }
}

fn init_config() -> Result<()> {
    let cfg_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("epycell")
    } else {
        PathBuf::from(std::env::var("HOME").expect("HOME unset")).join(".config/epycell")
    };
    let themes_dir = cfg_dir.join("themes");
    std::fs::create_dir_all(&themes_dir)?;

    let cfg_path = cfg_dir.join("config.toml");
    if !cfg_path.exists() {
        std::fs::write(&cfg_path, include_str!("../default_config.toml"))?;
        eprintln!("created {}", cfg_path.display());
    } else {
        eprintln!("exists  {}", cfg_path.display());
    }

    let themes: &[(&str, &str)] = &[
        ("aidsDick.tmTheme", include_str!("../themes/aidsDick.tmTheme")),
        ("catppuccin-mocha.tmTheme", include_str!("../themes/catppuccin-mocha.tmTheme")),
        ("dracula.tmTheme", include_str!("../themes/dracula.tmTheme")),
        ("gruvbox-dark.tmTheme", include_str!("../themes/gruvbox-dark.tmTheme")),
        ("nord.tmTheme", include_str!("../themes/nord.tmTheme")),
        ("onedark.tmTheme", include_str!("../themes/onedark.tmTheme")),
        ("tokyonight.tmTheme", include_str!("../themes/tokyonight.tmTheme")),
    ];

    for (name, content) in themes {
        let path = themes_dir.join(name);
        if !path.exists() {
            std::fs::write(&path, content)?;
            eprintln!("created {}", path.display());
        } else {
            eprintln!("exists  {}", path.display());
        }
    }

    eprintln!("\nepycell config ready at {}", cfg_dir.display());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Headless self-test: verify inline figures flush even with a trailing print.
    if std::env::args().nth(1).as_deref() == Some("--selftest") {
        return selftest().await;
    }

    // Install default config + themes.
    if std::env::args().nth(1).as_deref() == Some("--init") {
        return init_config();
    }

    // Detect graphics protocol BEFORE we grab the terminal.
    let picker = Picker::from_query_stdio().context("querying terminal graphics support")?;

    // Optional notebook path: `epycell notebook.ipynb`
    let path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);
    let cells = match &path {
        Some(p) if p.exists() => nb::load(p)?
            .into_iter()
            .map(|c| if c.markdown { Cell::markdown(&c.source) } else { Cell::new(&c.source) })
            .collect(),
        Some(_) => Vec::new(), // new file at this path
        None => demo_cells(),
    };

    let python = default_kernel_python();
    if !Path::new(&python).exists() {
        anyhow::bail!("kernel python not found at {} — set up the uv venv first", python.display());
    }
    eprintln!("launching kernel…");
    let mut session = KernelSession::launch(&python).await?;

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let cfg = config::load();
    let app = App::new(picker, cells, path, cfg);
    let result = run_app(&mut terminal, &mut session, app).await;

    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture, cursor::Show)?;
    session.shutdown().await;
    result
}

async fn run_app(terminal: &mut Tui, session: &mut KernelSession, mut app: App) -> Result<()> {
    loop {
        // Poll for async cell output before drawing so updates appear immediately.
        poll_running_cell(&mut app, session).await?;

        let h = terminal.size()?.height.saturating_sub(1);
        if !app.free_scroll {
            app.ensure_visible(h);
        }
        terminal.draw(|f| draw(f, &app))?;

        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let ev = event::read()?;
        match &ev {
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        app.scroll_offset += 3;
                        let ch = app.cell_height(app.scroll_top) + 1;
                        if app.scroll_offset >= ch {
                            if app.scroll_top + 1 < app.cells.len() {
                                app.scroll_top += 1;
                            }
                            app.scroll_offset = 0;
                        }
                        app.free_scroll = true;
                    }
                    MouseEventKind::ScrollUp => {
                        if app.scroll_offset >= 3 {
                            app.scroll_offset -= 3;
                        } else if app.scroll_offset > 0 {
                            app.scroll_offset = 0;
                        } else if app.scroll_top > 0 {
                            app.scroll_top -= 1;
                            // Jump to bottom of previous cell so next scroll-up
                            // continues smoothly
                            let ch = app.cell_height(app.scroll_top) + 1;
                            app.scroll_offset = ch.saturating_sub(1);
                        }
                        app.free_scroll = true;
                    }
                    _ => {}
                }
                continue;
            }
            Event::Key(key) if key.kind != KeyEventKind::Press => continue,
            Event::Key(_) => {}
            _ => continue,
        }
        let key = match ev {
            Event::Key(k) => k,
            _ => continue,
        };

        // Save-before-quit confirmation takes priority over everything.
        if app.pending_quit {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if app.save() {
                        break;
                    }
                    // save failed: stay open so the error is visible
                    app.pending_quit = false;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => break,
                KeyCode::Esc | KeyCode::Char('c') => {
                    app.pending_quit = false;
                    app.status = "cancelled — still editing".into();
                }
                _ => {}
            }
            continue;
        }

        // Navigate mode — config-driven keybindings
        let code = key.code;
        let mods = key.modifiers;
        let keys = &app.cfg.keys;
        if keys.quit.iter().any(|b| b.matches(code, mods)) {
            if app.dirty {
                app.pending_quit = true;
                app.status = "Unsaved changes — save before quitting?  y = save & quit · n = quit · Esc = cancel".into();
            } else {
                break;
            }
        } else if keys.save.iter().any(|b| b.matches(code, mods)) {
            app.save();
        } else if keys.run_all.iter().any(|b| b.matches(code, mods)) {
            let queue: Vec<usize> = (0..app.cells.len())
                .filter(|&i| !app.cells[i].markdown)
                .collect();
            if !queue.is_empty() {
                app.run_queue = queue;
                app.status = format!("running all {} code cells…", app.run_queue.len());
            }
        } else if keys.run_above.iter().any(|b| b.matches(code, mods)) {
            let queue: Vec<usize> = (0..=app.selected)
                .filter(|&i| !app.cells[i].markdown)
                .collect();
            if !queue.is_empty() {
                app.run_queue = queue;
                app.status = format!("running {} cells up to selected…", app.run_queue.len());
            }
        } else if keys.interrupt.iter().any(|b| b.matches(code, mods)) {
            if let Some(rc) = app.running_cell.take() {
                session.interrupt();
                app.cells[rc.idx].running = false;
                app.run_queue.clear();
                app.status = format!("interrupted cell [{}]", rc.exec_num);
            } else {
                app.status = "nothing running".into();
            }
        } else if keys.run.iter().any(|b| b.matches(code, mods)) {
            if app.cells[app.selected].markdown {
                app.status = "markdown cell — nothing to run".into();
            } else {
                start_cell_run(&mut app, session).await?;
            }
        } else if keys.move_down.iter().any(|b| b.matches(code, mods)) {
            if app.selected + 1 < app.cells.len() {
                app.selected += 1;
            }
            app.free_scroll = false;
            app.scroll_offset = 0;
            app.pending_d = false;
        } else if keys.move_up.iter().any(|b| b.matches(code, mods)) {
            if app.selected > 0 {
                app.selected -= 1;
            }
            app.free_scroll = false;
            app.scroll_offset = 0;
            app.pending_d = false;
        } else if keys.edit.iter().any(|b| b.matches(code, mods)) {
            let idx = app.selected;
            let conn = session.connection_file().to_path_buf();
            edit_cell_pty(terminal, &mut app, idx, &conn)?;
        } else if keys.edit_full.iter().any(|b| b.matches(code, mods)) {
            let conn = session.connection_file().to_path_buf();
            let new_src = external_edit(terminal, &app.cells[app.selected].source, &conn)?;
            if new_src != app.cells[app.selected].source {
                app.cells[app.selected].source = new_src;
                app.dirty = true;
            }
            app.status = format!("edited in {}", editor_command());
        } else if keys.new_below.iter().any(|b| b.matches(code, mods)) {
            app.cells.insert(app.selected + 1, Cell::new(""));
            app.selected += 1;
            app.dirty = true;
        } else if keys.new_above.iter().any(|b| b.matches(code, mods)) {
            app.cells.insert(app.selected, Cell::new(""));
            app.dirty = true;
        } else if keys.delete.iter().any(|b| b.matches(code, mods)) {
            if app.pending_d {
                if app.cells.len() > 1 {
                    app.cells.remove(app.selected);
                    if app.selected >= app.cells.len() {
                        app.selected = app.cells.len() - 1;
                    }
                    app.dirty = true;
                }
                app.pending_d = false;
            } else {
                app.pending_d = true;
            }
        } else if code == KeyCode::Char('x') && mods == KeyModifiers::NONE {
            let cell = &mut app.cells[app.selected];
            cell.output_expanded = !cell.output_expanded;
            if !cell.output_expanded {
                cell.output_scroll = 0;
            }
        } else {
            app.pending_d = false;
        }
    }
    Ok(())
}
