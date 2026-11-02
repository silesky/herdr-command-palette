//! Prompts and the second-stage picker.

use crate::config::{settings, RESET};
use crate::picker;
use crate::render::{matches, strip_ansi, Row};
use std::io::{self, Write};

fn read_line() -> Option<String> {
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

fn prompt_for(text: &str) {
    print!("{text}");
    let _ = io::stdout().flush();
}

pub fn ask(prompt: &str, default: &str) -> Option<String> {
    let suffix = if default.is_empty() { String::new() } else { format!(" [{default}]") };
    prompt_for(&format!("{prompt}{suffix}: "));
    let value = read_line()?;
    Some(value).filter(|v| !v.is_empty()).or_else(|| Some(default.to_string()).filter(|d| !d.is_empty()))
}

pub fn confirm(prompt: &str) -> bool {
    prompt_for(&format!("{prompt} [y/N]: "));
    read_line().is_some_and(|answer| matches!(answer.to_lowercase().as_str(), "y" | "yes"))
}

/// Pause so a popup's message stays readable before the pane closes.
pub fn wait_for_enter(message: &str) {
    prompt_for(&format!("{}{message}{RESET}", settings().colors.dim));
    let _ = read_line();
}

/// Rows whose text matches the query; subsequence matching only when no row matches outright.
fn filter_rows(rows: &[Row], query: &str) -> Vec<Row> {
    let texts: Vec<String> = rows.iter().map(|(_, display)| strip_ansi(display)).collect();
    let loose = !texts.iter().any(|text| matches(query, text, false));
    rows.iter()
        .zip(&texts)
        .filter(|(_, text)| matches(query, text, loose))
        .map(|(row, _)| row.clone())
        .collect()
}

/// Second-stage picker; rows are (value, display).
pub fn pick(rows: &[Row], header: &str) -> Option<String> {
    if rows.is_empty() {
        println!("{}nothing to pick{RESET}", settings().colors.dim);
        return None;
    }
    picker::run(header, None, |query, _| filter_rows(rows, query))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            ("w1".into(), "1  alpha".into()),
            ("w2".into(), "2  \x1b[2mbeta\x1b[0m".into()),
            ("w3".into(), "3  gamma".into()),
        ]
    }

    fn ids(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.0.as_str()).collect()
    }

    #[test]
    fn empty_query_keeps_every_row() {
        assert_eq!(ids(&filter_rows(&rows(), "")), ["w1", "w2", "w3"]);
    }

    #[test]
    fn query_matches_visible_text_ignoring_colors() {
        assert_eq!(ids(&filter_rows(&rows(), "beta")), ["w2"]);
        assert_eq!(ids(&filter_rows(&rows(), "m2")), Vec::<&str>::new());
    }

    #[test]
    fn falls_back_to_subsequence_when_nothing_matches_outright() {
        assert_eq!(ids(&filter_rows(&rows(), "gma")), ["w3"]);
    }
}
