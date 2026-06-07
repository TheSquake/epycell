use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Theme {
    pub bg: Color,
    pub selected: Color,
    pub editing: Color,
    pub inactive: Color,
    pub error: Color,
    pub output: Color,
    pub status_nav: Color,
    pub status_edit: Color,
    pub syntax_theme: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Rgb(0x0d, 0x19, 0x26),
            selected: Color::Rgb(0xb8, 0xb8, 0x7a),
            editing: Color::Rgb(0x7a, 0xb8, 0x7a),
            inactive: Color::Rgb(0x4a, 0x5a, 0x6a),
            error: Color::Rgb(0xb8, 0x7a, 0x7a),
            output: Color::Rgb(0x7a, 0xb8, 0xb8),
            status_nav: Color::Rgb(0x7a, 0x7a, 0xb8),
            status_edit: Color::Rgb(0x7a, 0xb8, 0x7a),
            syntax_theme: "base16-ocean.dark".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyBind {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyBind {
    fn new(code: KeyCode) -> Self {
        Self { code, mods: KeyModifiers::NONE }
    }

    fn with_mods(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }

    pub fn matches(&self, code: KeyCode, mods: KeyModifiers) -> bool {
        self.code == code && self.mods == mods
    }

    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) { s.push_str("C-"); }
        if self.mods.contains(KeyModifiers::ALT) { s.push_str("A-"); }
        if self.mods.contains(KeyModifiers::SHIFT) { s.push_str("S-"); }
        match self.code {
            KeyCode::Char(c) => s.push(c),
            KeyCode::Enter => s.push_str("Enter"),
            KeyCode::Esc => s.push_str("Esc"),
            KeyCode::Tab => s.push_str("Tab"),
            KeyCode::Up => s.push_str("↑"),
            KeyCode::Down => s.push_str("↓"),
            KeyCode::Left => s.push_str("←"),
            KeyCode::Right => s.push_str("→"),
            KeyCode::Backspace => s.push_str("BS"),
            KeyCode::Delete => s.push_str("Del"),
            KeyCode::Home => s.push_str("Home"),
            KeyCode::End => s.push_str("End"),
            KeyCode::PageUp => s.push_str("PgUp"),
            KeyCode::PageDown => s.push_str("PgDn"),
            KeyCode::F(n) => { s.push_str(&format!("F{n}")); }
            _ => s.push_str("?"),
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct Keys {
    pub run: Vec<KeyBind>,
    pub run_all: Vec<KeyBind>,
    pub run_above: Vec<KeyBind>,
    pub interrupt: Vec<KeyBind>,
    pub move_down: Vec<KeyBind>,
    pub move_up: Vec<KeyBind>,
    pub edit: Vec<KeyBind>,
    pub edit_full: Vec<KeyBind>,
    pub new_below: Vec<KeyBind>,
    pub new_above: Vec<KeyBind>,
    pub delete: Vec<KeyBind>,
    pub save: Vec<KeyBind>,
    pub quit: Vec<KeyBind>,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            run: vec![KeyBind::new(KeyCode::Char('R'))],
            run_all: vec![KeyBind::with_mods(KeyCode::Char('r'), KeyModifiers::CONTROL)],
            run_above: vec![KeyBind::with_mods(KeyCode::Char('a'), KeyModifiers::CONTROL)],
            interrupt: vec![KeyBind::with_mods(KeyCode::Char('c'), KeyModifiers::CONTROL)],
            move_down: vec![KeyBind::new(KeyCode::Char('j')), KeyBind::new(KeyCode::Down)],
            move_up: vec![KeyBind::new(KeyCode::Char('k')), KeyBind::new(KeyCode::Up)],
            edit: vec![KeyBind::new(KeyCode::Char('i')), KeyBind::new(KeyCode::Enter)],
            edit_full: vec![KeyBind::new(KeyCode::Char('e'))],
            new_below: vec![KeyBind::new(KeyCode::Char('o'))],
            new_above: vec![KeyBind::new(KeyCode::Char('O'))],
            delete: vec![KeyBind::new(KeyCode::Char('d'))],
            save: vec![KeyBind::new(KeyCode::Char('w'))],
            quit: vec![KeyBind::new(KeyCode::Char('q'))],
        }
    }
}

impl Keys {
    fn first_label(binds: &[KeyBind]) -> String {
        binds.first().map(|b| b.label()).unwrap_or_default()
    }

    pub fn help_line(&self) -> String {
        format!(
            "{}/{} move · {} edit · {} full-edit · {} run · {} run-all · {} new · {}{}del · {} save · {} quit",
            Self::first_label(&self.move_down),
            Self::first_label(&self.move_up),
            Self::first_label(&self.edit),
            Self::first_label(&self.edit_full),
            Self::first_label(&self.run),
            Self::first_label(&self.run_all),
            Self::first_label(&self.new_below),
            Self::first_label(&self.delete),
            Self::first_label(&self.delete),
            Self::first_label(&self.save),
            Self::first_label(&self.quit),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Images {
    pub max_width: u16,  // 0 = fit to cell width
    pub max_height: u16, // 0 = default cap of 30
    pub min_width: u16,  // 0 = no minimum
    pub min_height: u16, // 0 = no minimum
}

impl Default for Images {
    fn default() -> Self {
        Self { max_width: 0, max_height: 0, min_width: 0, min_height: 0 }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub theme: Theme,
    pub keys: Keys,
    pub images: Images,
}

impl Default for Config {
    fn default() -> Self {
        Self { theme: Theme::default(), keys: Keys::default(), images: Images::default() }
    }
}

pub fn load() -> Config {
    let path = config_path();
    if !path.exists() {
        return Config::default();
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Config::default(),
    };
    let raw: RawConfig = match toml::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("epycell: config parse error: {e}");
            return Config::default();
        }
    };
    let mut cfg = Config::default();
    if let Some(t) = raw.theme {
        if let Some(c) = t.bg.and_then(|s| parse_hex(&s)) { cfg.theme.bg = c; }
        if let Some(c) = t.selected.and_then(|s| parse_hex(&s)) { cfg.theme.selected = c; }
        if let Some(c) = t.editing.and_then(|s| parse_hex(&s)) { cfg.theme.editing = c; }
        if let Some(c) = t.inactive.and_then(|s| parse_hex(&s)) { cfg.theme.inactive = c; }
        if let Some(c) = t.error.and_then(|s| parse_hex(&s)) { cfg.theme.error = c; }
        if let Some(c) = t.output.and_then(|s| parse_hex(&s)) { cfg.theme.output = c; }
        if let Some(c) = t.status_nav.and_then(|s| parse_hex(&s)) { cfg.theme.status_nav = c; }
        if let Some(c) = t.status_edit.and_then(|s| parse_hex(&s)) { cfg.theme.status_edit = c; }
        if let Some(s) = t.syntax_theme { cfg.theme.syntax_theme = s; }
    }
    if let Some(k) = raw.keys {
        if let Some(v) = k.run.and_then(|s| parse_keys(&s)) { cfg.keys.run = v; }
        if let Some(v) = k.run_all.and_then(|s| parse_keys(&s)) { cfg.keys.run_all = v; }
        if let Some(v) = k.run_above.and_then(|s| parse_keys(&s)) { cfg.keys.run_above = v; }
        if let Some(v) = k.interrupt.and_then(|s| parse_keys(&s)) { cfg.keys.interrupt = v; }
        if let Some(v) = k.move_down.and_then(|s| parse_keys(&s)) { cfg.keys.move_down = v; }
        if let Some(v) = k.move_up.and_then(|s| parse_keys(&s)) { cfg.keys.move_up = v; }
        if let Some(v) = k.edit.and_then(|s| parse_keys(&s)) { cfg.keys.edit = v; }
        if let Some(v) = k.edit_full.and_then(|s| parse_keys(&s)) { cfg.keys.edit_full = v; }
        if let Some(v) = k.new_below.and_then(|s| parse_keys(&s)) { cfg.keys.new_below = v; }
        if let Some(v) = k.new_above.and_then(|s| parse_keys(&s)) { cfg.keys.new_above = v; }
        if let Some(v) = k.delete.and_then(|s| parse_keys(&s)) { cfg.keys.delete = v; }
        if let Some(v) = k.save.and_then(|s| parse_keys(&s)) { cfg.keys.save = v; }
        if let Some(v) = k.quit.and_then(|s| parse_keys(&s)) { cfg.keys.quit = v; }
    }
    if let Some(i) = raw.images {
        if let Some(w) = i.max_width { cfg.images.max_width = w; }
        if let Some(h) = i.max_height { cfg.images.max_height = h; }
        if let Some(w) = i.min_width { cfg.images.min_width = w; }
        if let Some(h) = i.min_height { cfg.images.min_height = h; }
    }
    cfg
}

fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("epycell/config.toml")
    } else {
        PathBuf::from(std::env::var("HOME").expect("HOME unset"))
            .join(".config/epycell/config.toml")
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 { return None; }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn parse_keys(s: &str) -> Option<Vec<KeyBind>> {
    let binds: Vec<KeyBind> = s.split(',')
        .filter_map(|part| parse_single_key(part.trim()))
        .collect();
    if binds.is_empty() { None } else { Some(binds) }
}

fn parse_single_key(s: &str) -> Option<KeyBind> {
    let parts: Vec<&str> = s.split('+').collect();
    let mut mods = KeyModifiers::NONE;
    let key_part = parts.last()?;

    for &p in &parts[..parts.len() - 1] {
        match p.to_lowercase().as_str() {
            "ctrl" | "c" => mods |= KeyModifiers::CONTROL,
            "alt" | "meta" | "m" => mods |= KeyModifiers::ALT,
            "shift" | "s" => mods |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }

    let code = match key_part.to_lowercase().as_str() {
        "enter" | "return" | "cr" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        s if s.len() == 1 => {
            let c = s.chars().next()?;
            if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                KeyCode::Char(c.to_ascii_uppercase())
            } else {
                KeyCode::Char(c)
            }
        }
        s if s.starts_with('f') => {
            let n: u8 = s[1..].parse().ok()?;
            KeyCode::F(n)
        }
        _ => return None,
    };

    Some(KeyBind::with_mods(code, mods))
}

// Raw TOML structures for deserialization

#[derive(Deserialize, Default)]
struct RawConfig {
    theme: Option<RawTheme>,
    keys: Option<RawKeys>,
    images: Option<RawImages>,
}

#[derive(Deserialize, Default)]
struct RawImages {
    max_width: Option<u16>,
    max_height: Option<u16>,
    min_width: Option<u16>,
    min_height: Option<u16>,
}

#[derive(Deserialize, Default)]
struct RawTheme {
    bg: Option<String>,
    selected: Option<String>,
    editing: Option<String>,
    inactive: Option<String>,
    error: Option<String>,
    output: Option<String>,
    status_nav: Option<String>,
    status_edit: Option<String>,
    syntax_theme: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawKeys {
    run: Option<String>,
    run_all: Option<String>,
    run_above: Option<String>,
    interrupt: Option<String>,
    move_down: Option<String>,
    move_up: Option<String>,
    edit: Option<String>,
    edit_full: Option<String>,
    new_below: Option<String>,
    new_above: Option<String>,
    delete: Option<String>,
    save: Option<String>,
    quit: Option<String>,
}
