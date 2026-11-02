//! Renders Herdr key chords ("prefix+shift+n") the way they read on a keyboard.

use std::collections::HashMap;
use toml::{Table, Value};

const DEFAULT_KEYS: [(&str, &str); 37] = [
    ("prefix", "ctrl+b"),
    ("help", "prefix+?"),
    ("settings", "prefix+s"),
    ("goto", "prefix+g"),
    ("detach", "prefix+q"),
    ("reload_config", "prefix+shift+r"),
    ("open_notification_target", "prefix+o"),
    ("new_workspace", "prefix+shift+n"),
    ("rename_workspace", "prefix+shift+w"),
    ("close_workspace", "prefix+shift+d"),
    ("workspace_picker", "prefix+w"),
    ("new_worktree", "prefix+shift+g"),
    ("new_tab", "prefix+c"),
    ("rename_tab", "prefix+shift+t"),
    ("previous_tab", "prefix+p"),
    ("next_tab", "prefix+n"),
    ("switch_tab", "prefix+1..9"),
    ("close_tab", "prefix+shift+x"),
    ("split_vertical", "prefix+v"),
    ("split_horizontal", "prefix+minus"),
    ("close_pane", "prefix+x"),
    ("zoom", "prefix+z"),
    ("rename_pane", "prefix+shift+p"),
    ("copy_mode", "prefix+["),
    ("edit_scrollback", "prefix+e"),
    ("resize_mode", "prefix+r"),
    ("toggle_sidebar", "prefix+b"),
    ("cycle_pane_next", "prefix+tab"),
    ("cycle_pane_previous", "prefix+shift+tab"),
    ("focus_pane_left", "prefix+h"),
    ("focus_pane_down", "prefix+j"),
    ("focus_pane_up", "prefix+k"),
    ("focus_pane_right", "prefix+l"),
    ("swap_pane_left", "prefix+shift+h"),
    ("swap_pane_down", "prefix+shift+j"),
    ("swap_pane_up", "prefix+shift+k"),
    ("swap_pane_right", "prefix+shift+l"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModStyle {
    Symbol,
    Text,
}

impl ModStyle {
    /// "symbol" or "text" from config; otherwise symbols on macOS, text elsewhere.
    pub fn resolve(setting: &str) -> Self {
        match setting {
            "text" => ModStyle::Text,
            "symbol" => ModStyle::Symbol,
            _ if cfg!(target_os = "macos") => ModStyle::Symbol,
            _ => ModStyle::Text,
        }
    }

    fn modifier(self, name: &str) -> Option<&'static str> {
        let (symbol, text) = match name {
            "ctrl" | "control" => ("^", "Ctrl+"),
            "alt" | "opt" | "option" => ("⌥", "Alt+"),
            "shift" => ("⇧", "Shift+"),
            "cmd" => ("⌘", "Cmd+"),
            "super" => ("⌘", "Super+"),
            "meta" => ("⌘", "Meta+"),
            _ => return None,
        };
        Some(if self == ModStyle::Symbol { symbol } else { text })
    }
}

fn named_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "semicolon" => ";",
        "comma" => ",",
        "period" => ".",
        "minus" => "-",
        "plus" => "+",
        "backslash" => "\\",
        "slash" => "/",
        "quote" => "'",
        "backtick" => "`",
        "equal" => "=",
        _ => return None,
    })
}

const WORD_KEYS: [&str; 9] = ["enter", "tab", "space", "esc", "escape", "up", "down", "left", "right"];

pub fn render_chord(chord: &str, style: ModStyle) -> String {
    let mut parts: Vec<String> = chord.split('+').map(String::from).collect();
    // A shifted letter reads better as the capital you actually type.
    let last_is_single_char = parts.last().is_some_and(|p| p.chars().count() == 1);
    if last_is_single_char && parts.iter().any(|p| p.eq_ignore_ascii_case("shift")) {
        let key = parts.pop().unwrap_or_default().to_uppercase();
        parts.retain(|p| !p.eq_ignore_ascii_case("shift"));
        parts.push(key);
    }
    parts
        .iter()
        .map(|part| {
            let low = part.to_lowercase();
            if let Some(m) = style.modifier(&low) {
                m.to_string()
            } else if let Some(k) = named_key(&low) {
                k.to_string()
            } else if WORD_KEYS.contains(&low.as_str()) {
                low
            } else {
                part.clone()
            }
        })
        .collect()
}

pub struct Keymap {
    raw: HashMap<String, String>,
    leader: String,
    style: ModStyle,
}

impl Keymap {
    /// An empty `leader` falls back to the rendered prefix chord.
    pub fn new(config: &Table, leader: &str, style: ModStyle) -> Self {
        let mut raw: HashMap<String, String> =
            DEFAULT_KEYS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(keys) = config.get("keys").and_then(Value::as_table) {
            for (name, value) in keys {
                match value {
                    Value::String(s) => {
                        raw.insert(name.clone(), s.clone());
                    }
                    Value::Array(items) if items.iter().all(Value::is_str) => {
                        let first = items.first().and_then(Value::as_str).unwrap_or("");
                        raw.insert(name.clone(), first.to_string());
                    }
                    _ => {}
                }
            }
        }
        let prefix = render_chord(raw.get("prefix").map_or("ctrl+b", String::as_str), style);
        let leader = if leader.is_empty() { prefix } else { leader.to_string() };
        Keymap { raw, leader, style }
    }

    pub fn chord(&self, name: Option<&str>) -> String {
        match name.and_then(|n| self.raw.get(n)) {
            Some(value) if !value.is_empty() => self.render(value),
            _ => String::new(),
        }
    }

    pub fn render(&self, value: &str) -> String {
        if let Some(rest) = value.strip_prefix("prefix+") {
            format!("{} {}", self.leader, render_chord(rest, self.style))
        } else if value == "prefix" {
            self.leader.clone()
        } else {
            render_chord(value, self.style)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap(toml_text: &str, leader: &str, style: ModStyle) -> Keymap {
        Keymap::new(&toml::from_str::<Table>(toml_text).unwrap(), leader, style)
    }

    #[test]
    fn renders_modifiers_as_symbols_or_text() {
        assert_eq!(render_chord("ctrl+b", ModStyle::Symbol), "^b");
        assert_eq!(render_chord("ctrl+alt+x", ModStyle::Text), "Ctrl+Alt+x");
    }

    #[test]
    fn shifted_letter_becomes_capital() {
        assert_eq!(render_chord("shift+n", ModStyle::Symbol), "N");
        assert_eq!(render_chord("ctrl+shift+n", ModStyle::Symbol), "^N");
    }

    #[test]
    fn shift_kept_for_word_keys() {
        assert_eq!(render_chord("shift+tab", ModStyle::Symbol), "⇧tab");
    }

    #[test]
    fn named_keys_become_punctuation() {
        assert_eq!(render_chord("minus", ModStyle::Text), "-");
        assert_eq!(render_chord("ctrl+semicolon", ModStyle::Symbol), "^;");
    }

    #[test]
    fn word_keys_lowercase() {
        assert_eq!(render_chord("Enter", ModStyle::Text), "enter");
    }

    #[test]
    fn prefix_chords_use_rendered_prefix_by_default() {
        let map = keymap("", "", ModStyle::Symbol);
        assert_eq!(map.chord(Some("split_horizontal")), "^b -");
        assert_eq!(map.chord(Some("new_workspace")), "^b N");
    }

    #[test]
    fn custom_leader_and_prefix() {
        let map = keymap("[keys]\nprefix = \"ctrl+semicolon\"", "", ModStyle::Symbol);
        assert_eq!(map.chord(Some("zoom")), "^; z");
        let map = keymap("", "✦", ModStyle::Symbol);
        assert_eq!(map.chord(Some("zoom")), "✦ z");
    }

    #[test]
    fn user_binding_overrides_default() {
        let map = keymap("[keys]\nzoom = \"alt+z\"", "", ModStyle::Text);
        assert_eq!(map.chord(Some("zoom")), "Alt+z");
    }

    #[test]
    fn array_binding_uses_first_entry() {
        let map = keymap("[keys]\nzoom = [\"alt+z\", \"alt+y\"]", "", ModStyle::Text);
        assert_eq!(map.chord(Some("zoom")), "Alt+z");
    }

    #[test]
    fn unbound_and_empty_names_have_no_chord() {
        let map = keymap("", "", ModStyle::Text);
        assert_eq!(map.chord(Some("last_pane")), "");
        assert_eq!(map.chord(None), "");
    }

    #[test]
    fn bare_prefix_renders_leader() {
        let map = keymap("", "", ModStyle::Symbol);
        assert_eq!(map.render("prefix"), "^b");
    }
}
