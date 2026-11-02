//! Herdr config, plugin config, theme colors, and agent status glyphs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{env, fs};
use toml::{Table, Value};

pub const RESET: &str = "\x1b[0m";
const DEFAULT_DIM: &str = "\x1b[2m";

pub const DEFAULT_GROUP_ORDER: [&str; 5] = ["agents", "workspaces", "actions", "custom", "plugins"];

#[derive(Clone, Debug)]
pub struct Colors {
    pub key: String,
    pub dim: String,
    pub cat: String,
}

impl Default for Colors {
    fn default() -> Self {
        Colors { key: "\x1b[35m".into(), dim: DEFAULT_DIM.into(), cat: "\x1b[36m".into() }
    }
}

/// Convert #rrggbb to a 24-bit ANSI foreground sequence; empty when invalid.
pub fn hex_to_ansi(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if !hex.is_ascii() || hex.len() != 6 {
        return String::new();
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16);
    match (channel(0..2), channel(2..4), channel(4..6)) {
        (Ok(r), Ok(g), Ok(b)) => format!("\x1b[38;2;{r};{g};{b}m"),
        _ => String::new(),
    }
}

fn theme_custom(config: &Table) -> Option<&Table> {
    config.get("theme")?.get("custom")?.as_table()
}

pub fn theme_color<'a>(config: &'a Table, name: &str) -> Option<&'a str> {
    theme_custom(config)?.get(name)?.as_str().filter(|s| !s.is_empty())
}

/// Override key/cat/dim from [theme.custom] so row text matches the theme.
pub fn apply_theme_colors(config: &Table, colors: &mut Colors) {
    for (token, slot) in [
        ("accent", &mut colors.key),
        ("overlay0", &mut colors.cat),
        ("subtext0", &mut colors.dim),
    ] {
        if let Some(ansi) = theme_color(config, token).map(hex_to_ansi).filter(|a| !a.is_empty()) {
            *slot = ansi;
        }
    }
}

/// Same as `hex_to_ansi`, for the background.
pub fn hex_to_ansi_bg(hex: &str) -> String {
    hex_to_ansi(hex).replacen("[38;", "[48;", 1)
}

pub fn load_toml(path: &Path) -> Table {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<Table>(&text).ok())
        .unwrap_or_default()
}

fn home() -> String {
    env::var("HOME").unwrap_or_default()
}

pub fn herdr_config_path() -> PathBuf {
    PathBuf::from(home()).join(".config/herdr/config.toml")
}

pub fn tilde(path: &str) -> String {
    let home = home();
    if home.is_empty() {
        return path.to_string();
    }
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => path.to_string(),
    }
}

/// The plugin's own config, from HERDR_PLUGIN_CONFIG_DIR/config.toml.
pub struct PluginConfig {
    pub leader: String,
    pub modifier_style: String,
    pub show_key_only: bool,
    pub group_order: Vec<String>,
    glyphs: Table,
}

impl PluginConfig {
    pub fn load() -> Self {
        let table = match env::var("HERDR_PLUGIN_CONFIG_DIR") {
            Ok(dir) if !dir.is_empty() => load_toml(&Path::new(&dir).join("config.toml")),
            _ => Table::new(),
        };
        Self::from_table(&table)
    }

    pub fn from_table(table: &Table) -> Self {
        let string = |name: &str| table.get(name).and_then(Value::as_str).unwrap_or("").to_string();
        let group_order = match table.get("group_order") {
            Some(Value::Array(items)) => {
                items.iter().filter_map(Value::as_str).map(String::from).collect()
            }
            _ => DEFAULT_GROUP_ORDER.iter().map(|g| g.to_string()).collect(),
        };
        PluginConfig {
            leader: string("leader"),
            modifier_style: string("modifier_style"),
            show_key_only: table.get("show_key_only").and_then(Value::as_bool).unwrap_or(true),
            group_order,
            glyphs: table.get("glyphs").and_then(Value::as_table).cloned().unwrap_or_default(),
        }
    }

    pub fn glyphs(&self) -> Glyphs {
        let mut glyphs = Glyphs::default();
        for status in ["working", "done", "idle", "blocked", "unknown"] {
            if let Some(glyph) =
                self.glyphs.get(status).and_then(Value::as_str).filter(|g| !g.is_empty())
            {
                glyphs.set(status, glyph);
            }
        }
        glyphs
    }
}

/// Glyph and colour per agent status, so a blocked agent is findable at a glance.
pub struct Glyphs {
    known: HashMap<&'static str, (String, String)>,
    unknown: (String, String),
}

impl Default for Glyphs {
    fn default() -> Self {
        let entry = |glyph: &str, color: &str| (glyph.to_string(), color.to_string());
        Glyphs {
            known: HashMap::from([
                ("working", entry("●", "\x1b[33m")),
                ("done", entry("✓", "\x1b[32m")),
                ("idle", entry("◌", DEFAULT_DIM)),
                ("blocked", entry("▲", "\x1b[31m")),
            ]),
            unknown: entry("·", DEFAULT_DIM),
        }
    }
}

impl Glyphs {
    fn set(&mut self, status: &str, glyph: &str) {
        if status == "unknown" {
            self.unknown.0 = glyph.to_string();
        } else if let Some(entry) = self.known.get_mut(status) {
            entry.0 = glyph.to_string();
        }
    }

    pub fn get(&self, status: &str) -> &(String, String) {
        self.known.get(status).unwrap_or(&self.unknown)
    }
}

/// Sorted by how much the agent wants from you: blocked is waiting on an
/// answer, working may block soon, done is finished, idle wants nothing.
pub fn status_rank(status: &str) -> usize {
    ["blocked", "working", "done", "idle"].iter().position(|s| *s == status).unwrap_or(4)
}

/// Resolved once in main, read by pickers and prompts deep in the call tree.
pub struct Settings {
    pub colors: Colors,
    pub glyphs: Glyphs,
    /// ANSI background for the selected row.
    pub highlight: String,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

pub fn init_settings(settings: Settings) {
    let _ = SETTINGS.set(settings);
}

pub fn settings() -> &'static Settings {
    SETTINGS.get().expect("settings initialised in main")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_converts_to_truecolor() {
        assert_eq!(hex_to_ansi("#ff8000"), "\x1b[38;2;255;128;0m");
        assert_eq!(hex_to_ansi("ff8000"), "\x1b[38;2;255;128;0m");
    }

    #[test]
    fn hex_background_uses_the_48_code() {
        assert_eq!(hex_to_ansi_bg("#010203"), "\x1b[48;2;1;2;3m");
        assert_eq!(hex_to_ansi_bg("nope"), "");
    }

    #[test]
    fn hex_rejects_bad_input() {
        assert_eq!(hex_to_ansi("#fff"), "");
        assert_eq!(hex_to_ansi("#gggggg"), "");
        assert_eq!(hex_to_ansi("#ééé"), "");
    }

    #[test]
    fn theme_overrides_only_valid_tokens() {
        let config: Table = toml::from_str(
            "[theme.custom]\naccent = \"#010203\"\noverlay0 = \"nope\"\nsubtext0 = \"#040506\"",
        )
        .unwrap();
        let mut colors = Colors::default();
        apply_theme_colors(&config, &mut colors);
        assert_eq!(colors.key, "\x1b[38;2;1;2;3m");
        assert_eq!(colors.cat, Colors::default().cat);
        assert_eq!(colors.dim, "\x1b[38;2;4;5;6m");
    }

    #[test]
    fn plugin_config_defaults() {
        let config = PluginConfig::from_table(&Table::new());
        assert!(config.show_key_only);
        assert_eq!(config.group_order, DEFAULT_GROUP_ORDER);
        assert_eq!(config.leader, "");
    }

    #[test]
    fn plugin_config_reads_overrides() {
        let table: Table = toml::from_str(
            "leader = \"X\"\nshow_key_only = false\ngroup_order = [\"actions\"]\n\
             [glyphs]\nblocked = \"!\"\nunknown = \"?\"",
        )
        .unwrap();
        let config = PluginConfig::from_table(&table);
        assert_eq!(config.leader, "X");
        assert!(!config.show_key_only);
        assert_eq!(config.group_order, ["actions"]);
        let glyphs = config.glyphs();
        assert_eq!(glyphs.get("blocked").0, "!");
        assert_eq!(glyphs.get("blocked").1, "\x1b[31m");
        assert_eq!(glyphs.get("anything-else").0, "?");
        assert_eq!(glyphs.get("working").0, "●");
    }

    #[test]
    fn status_rank_orders_by_urgency() {
        assert!(status_rank("blocked") < status_rank("working"));
        assert!(status_rank("working") < status_rank("done"));
        assert!(status_rank("done") < status_rank("idle"));
        assert!(status_rank("idle") < status_rank("unknown"));
    }
}
