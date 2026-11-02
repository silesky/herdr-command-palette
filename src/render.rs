//! Turns action specs into the tab-separated rows fzf displays.

use crate::config::{Colors, RESET};
use serde::{Deserialize, Serialize};
use std::{env, fs};

pub const SEPARATOR: &str = "__group__";
const KEY_WIDTH: usize = 16;

/// An action flattened to plain data, so re-rendering per keystroke stays cheap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub cat: String,
    pub title: String,
    pub keys: String,
    pub keycolor: String,
    pub note: String,
    pub teach: bool,
}

/// What the per-keystroke `--rows` reload reads back from disk.
#[derive(Serialize, Deserialize)]
pub struct Cache {
    pub specs: Vec<Spec>,
    pub colors: Colors,
}

pub type Row = (String, String);

pub fn terminal_width() -> usize {
    // COLUMNS first: the --rows reload runs with no tty of its own, and fzf
    // exports the popup's width into the child's environment.
    env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .or_else(|| terminal_size::terminal_size().map(|(w, _)| w.0 as usize))
        .unwrap_or(100)
}

pub fn strip_ansi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && chars.get(i + 1) == Some(&'[') {
            let mut j = i + 2;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';') {
                j += 1;
            }
            if chars.get(j) == Some(&'m') {
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Regex `\b` semantics: true where a word character meets a non-word one.
fn boundary(chars: &[char], at: usize) -> bool {
    let before = at > 0 && is_word(chars[at - 1]);
    let after = at < chars.len() && chars.get(at).is_some_and(|c| is_word(*c));
    before != after
}

/// Drop every whole-word, case-insensitive `stem` or `stems` from `text`.
fn remove_words(text: &str, stem: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let stem: Vec<char> = stem.chars().collect();
    let n = stem.len();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let matches_stem = n > 0
            && i + n <= chars.len()
            && boundary(&chars, i)
            && chars[i..i + n].iter().zip(&stem).all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()));
        if matches_stem {
            let end = i + n;
            let plural = chars.get(end).is_some_and(|c| c.eq_ignore_ascii_case(&'s'))
                && boundary(&chars, end + 1);
            if plural {
                i = end + 1;
                continue;
            }
            if boundary(&chars, end) {
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn collapse_whitespace_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j - i >= 2 {
                out.push(' ');
            } else {
                out.push(chars[i]);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn hug_punctuation(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if matches!(c, '…' | ':' | '.') {
            out.truncate(out.trim_end().len());
        }
        out.push(c);
    }
    out
}

/// "Split pane right" under the "pane" heading reads as "Split right".
pub fn strip_category(title: &str, category: &str) -> String {
    let trimmed = category.trim_end_matches('s');
    let stem = if trimmed.is_empty() { category } else { trimmed };
    let stripped = hug_punctuation(&collapse_whitespace_runs(&remove_words(title, stem)));
    let stripped = stripped.trim();
    let mut chars = stripped.chars();
    match chars.next() {
        // A title that is only its category, e.g. "Workspaces", stays as it was.
        None => title.to_string(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Substring per term, falling back to subsequence, AND across terms.
pub fn matches(query: &str, haystack: &str, loose: bool) -> bool {
    let haystack = haystack.to_lowercase();
    query.to_lowercase().split_whitespace().all(|term| {
        if haystack.contains(term) {
            return true;
        }
        if !loose {
            return false;
        }
        let mut remaining = haystack.chars();
        term.chars().all(|c| remaining.any(|h| h == c))
    })
}

pub fn render_rows(specs: &[Spec], query: &str, width: usize, colors: &Colors) -> Vec<Row> {
    // Column widths come from the full set so they don't jitter while typing.
    let longest = specs.iter().map(|s| s.title.chars().count()).max().unwrap_or(24);
    let title_w = (longest + 2).clamp(24, 46);
    let haystacks: Vec<String> = specs
        .iter()
        .map(|s| format!("{} {} {} {}", s.cat, s.title, s.keys, strip_ansi(&s.note)))
        .collect();
    // Subsequence matching over long agent titles and paths hits almost
    // anything, so it only kicks in when no row contains the query outright.
    let loose = !haystacks.iter().any(|h| matches(query, h, false));
    let mut rows: Vec<Row> = Vec::new();
    let mut group: Option<&str> = None;
    for (spec, haystack) in specs.iter().zip(&haystacks) {
        if !query.is_empty() && !matches(query, haystack, loose) {
            continue;
        }
        if group != Some(spec.cat.as_str()) {
            group = Some(&spec.cat);
            rows.push((SEPARATOR.to_string(), String::new()));
            let rule_len = (title_w + KEY_WIDTH).saturating_sub(spec.cat.chars().count() + 4).max(4);
            rows.push((
                SEPARATOR.to_string(),
                format!("{}── {} {}{RESET}", colors.cat, spec.cat, "─".repeat(rule_len)),
            ));
        }
        let keys = if spec.keys.is_empty() { "—" } else { spec.keys.as_str() };
        let keys_color = if !spec.keycolor.is_empty() {
            spec.keycolor.as_str()
        } else if spec.keys.is_empty() {
            colors.dim.as_str()
        } else {
            colors.key.as_str()
        };
        let note = if spec.teach {
            format!("{} key only{RESET}", colors.dim)
        } else {
            spec.note.clone()
        };
        let display = format!(
            "{:<title_w$}{keys_color}{keys:<KEY_WIDTH$}{RESET}{note}",
            spec.title
        );
        rows.push((spec.id.clone(), display.chars().take(width + 64).collect()));
    }
    rows
}

pub fn encode_rows(rows: &[Row]) -> String {
    rows.iter().map(|(value, display)| format!("{value}\t{display}")).collect::<Vec<_>>().join("\n")
}

/// `--rows <query>`: what fzf reloads on every keystroke.
pub fn emit_rows(query: &str) -> i32 {
    let Some(path) = env::var("HERDR_PALETTE_SPECS").ok().filter(|p| !p.is_empty()) else {
        return 1;
    };
    let Some(cache) =
        fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Cache>(&text).ok())
    else {
        return 1;
    };
    println!("{}", encode_rows(&render_rows(&cache.specs, query, terminal_width(), &cache.colors)));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, cat: &str, title: &str, keys: &str) -> Spec {
        Spec {
            id: id.into(),
            cat: cat.into(),
            title: title.into(),
            keys: keys.into(),
            keycolor: String::new(),
            note: String::new(),
            teach: false,
        }
    }

    fn colors() -> Colors {
        Colors { key: "<K>".into(), dim: "<D>".into(), cat: "<C>".into() }
    }

    #[test]
    fn strip_category_removes_the_heading_word() {
        assert_eq!(strip_category("Split pane right", "pane"), "Split right");
        assert_eq!(strip_category("Toggle pane zoom", "pane"), "Toggle zoom");
        assert_eq!(strip_category("Rename tab…", "tab"), "Rename…");
        assert_eq!(strip_category("Switch to workspace…", "workspace"), "Switch to…");
    }

    #[test]
    fn strip_category_handles_plurals_and_case() {
        assert_eq!(strip_category("Workspaces list", "workspace"), "List");
        assert_eq!(strip_category("PANE one", "pane"), "One");
    }

    #[test]
    fn strip_category_respects_word_boundaries() {
        assert_eq!(strip_category("Panels", "pane"), "Panels");
        assert_eq!(strip_category("Spaned", "pane"), "Spaned");
    }

    #[test]
    fn strip_category_keeps_title_that_is_only_the_category() {
        assert_eq!(strip_category("Workspaces", "workspaces"), "Workspaces");
    }

    #[test]
    fn strip_category_uses_stem_of_plural_category() {
        assert_eq!(strip_category("Focus agent…", "agents"), "Focus…");
    }

    #[test]
    fn strip_category_leaves_unrelated_titles() {
        assert_eq!(strip_category("Reload config", "session"), "Reload config");
    }

    #[test]
    fn matches_requires_every_term() {
        assert!(matches("split right", "pane split right", false));
        assert!(!matches("split left", "pane split right", false));
    }

    #[test]
    fn matches_is_case_insensitive() {
        assert!(matches("SPLIT", "pane split", false));
    }

    #[test]
    fn matches_falls_back_to_subsequence_only_when_loose() {
        assert!(!matches("spr", "split right", false));
        assert!(matches("spr", "split right", true));
        assert!(!matches("zq", "split right", true));
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences_only() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi("\x1b[38;2;1;2;3mx"), "x");
        assert_eq!(strip_ansi("a\x1b[Kb"), "a\x1b[Kb");
    }

    #[test]
    fn rows_group_under_headings_with_blank_separator() {
        let specs = vec![
            spec("a", "pane", "Zoom", "^b z"),
            spec("b", "pane", "Close", ""),
            spec("c", "tab", "New", "^b c"),
        ];
        let rows = render_rows(&specs, "", 100, &colors());
        let ids: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(ids, [SEPARATOR, SEPARATOR, "a", "b", SEPARATOR, SEPARATOR, "c"]);
        assert_eq!(rows[0].1, "");
        assert!(rows[1].1.starts_with("<C>── pane "));
        assert!(rows[1].1.ends_with(RESET));
    }

    #[test]
    fn row_pads_title_and_keys_columns() {
        let rows = render_rows(&[spec("a", "pane", "Zoom", "^b z")], "", 100, &colors());
        assert_eq!(rows[2].1, format!("{:<24}<K>{:<16}{RESET}", "Zoom", "^b z"));
    }

    #[test]
    fn unbound_row_shows_dash_in_dim() {
        let rows = render_rows(&[spec("a", "pane", "Zoom", "")], "", 100, &colors());
        assert_eq!(rows[2].1, format!("{:<24}<D>{:<16}{RESET}", "Zoom", "—"));
    }

    #[test]
    fn key_only_rows_carry_a_dim_marker() {
        let mut s = spec("a", "pane", "Copy mode", "^b [");
        s.teach = true;
        let rows = render_rows(&[s], "", 100, &colors());
        assert!(rows[2].1.ends_with(&format!("<D> key only{RESET}")));
    }

    #[test]
    fn explicit_key_color_wins() {
        let mut s = spec("a", "agents", "x", "working");
        s.keycolor = "<Y>".into();
        let rows = render_rows(&[s], "", 100, &colors());
        assert!(rows[2].1.contains("<Y>working"));
    }

    #[test]
    fn query_filters_rows_and_drops_empty_groups() {
        let specs = vec![spec("a", "pane", "Zoom", "^b z"), spec("c", "tab", "New", "^b c")];
        let rows = render_rows(&specs, "new", 100, &colors());
        let ids: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(ids, [SEPARATOR, SEPARATOR, "c"]);
    }

    #[test]
    fn query_matches_category_and_note() {
        let mut s = spec("a", "agents", "● 1  fix", "idle");
        s.note = "\x1b[2mrepo  ~/proj\x1b[0m".into();
        let rows = render_rows(&[s], "proj", 100, &colors());
        assert_eq!(rows.len(), 3);
        let rows = render_rows(&[spec("b", "agents", "x", "")], "agents", 100, &colors());
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn subsequence_match_only_when_no_substring_hit() {
        let specs = vec![spec("a", "pane", "Split right", ""), spec("b", "pane", "Spend", "")];
        // "spr": no row contains it outright, so subsequence matching applies.
        let rows = render_rows(&specs, "spr", 100, &colors());
        assert!(rows.iter().any(|r| r.0 == "a"));
        // "spl" is a real substring of one row, so the loose match stays off.
        let rows = render_rows(&specs, "spl", 100, &colors());
        assert!(!rows.iter().any(|r| r.0 == "b"));
    }

    #[test]
    fn rows_truncate_to_width_plus_ansi_slack() {
        let long = "x".repeat(200);
        let rows = render_rows(&[spec("a", "pane", &long, "")], "", 20, &colors());
        assert_eq!(rows[2].1.chars().count(), 84);
    }

    #[test]
    fn encode_joins_value_and_display_with_tabs() {
        let rows = vec![("a".to_string(), "x".to_string()), ("b".to_string(), "y".to_string())];
        assert_eq!(encode_rows(&rows), "a\tx\nb\ty");
    }

    #[test]
    fn cache_round_trips_through_json() {
        let cache = Cache { specs: vec![spec("a", "pane", "Zoom", "^b z")], colors: colors() };
        let json = serde_json::to_string(&cache).unwrap();
        let back: Cache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.specs, cache.specs);
        assert_eq!(back.colors.key, "<K>");
    }
}
