//! Prompts and the fzf front end.

use crate::config::{settings, theme_color, RESET};
use crate::render::{encode_rows, Cache, Row, SEPARATOR};
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::{env, fs};
use toml::Table;

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

/// Second-stage picker; rows are (value, display).
pub fn pick(rows: &[Row], header: &str) -> Option<String> {
    if rows.is_empty() {
        println!("{}nothing to pick{RESET}", settings().colors.dim);
        return None;
    }
    run_fzf(rows, header, None, None)
}

/// Match fzf to [theme.custom] so the popup reads as one surface. Without this
/// fzf paints its own background over Herdr's popup panel, which shows up as a
/// band of the wrong navy inside the border.
pub fn fzf_theme(config: &Table) -> Vec<String> {
    let color = |name: &str| theme_color(config, name);
    let Some(bg) = color("panel_bg").or_else(|| color("surface0")) else {
        return Vec::new();
    };
    let spec = [
        ("bg", Some(bg)),
        ("bg+", color("surface1")),
        ("fg", color("text")),
        ("fg+", color("text")),
        ("hl", color("accent")),
        ("hl+", color("accent")),
        ("prompt", color("overlay0")),
        ("info", color("subtext0")),
        ("border", Some(bg)),
        ("pointer", color("accent")),
        ("marker", color("mauve")),
        // The rail matches the popup background so only the pointer shows; the
        // scrollbar stays one step up, visible but not a stripe.
        ("gutter", Some(bg)),
        ("scrollbar", color("surface1").or(Some(bg))),
    ];
    let pairs: Vec<String> = spec
        .iter()
        .filter_map(|(name, value)| value.map(|v| format!("{name}:{v}")))
        .collect();
    vec![format!("--color={}", pairs.join(","))]
}

pub fn shell_quote(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\"'\"'"))
    }
}

/// fzf's own matcher would drop the group headings along with the rows they
/// label, so matching happens in `--rows` instead and headings are redrawn for
/// whatever survives. Every group opens with a blank row and a heading, so one
/// step can land on at most two separators; a third check on the way up steps
/// back down when the first group's heading sits at the very top.
fn reload_binds(script: &str, initial_pos: usize) -> Vec<String> {
    let on_sep = |then: &str| format!("+transform([ {{1}} = {SEPARATOR} ] && echo {then} || true)");
    let down = format!("down{}", on_sep("down").repeat(2));
    let up = format!("up{}{}", on_sep("up").repeat(2), on_sep("down+down"));
    vec![
        "--disabled".to_string(),
        format!("--bind=change:reload({script} --rows {{q}})"),
        // `result` fires after every reload. Only the unfiltered list opens on
        // the caller's agent row: once the user types, its old index points at
        // an unrelated row, so focus the first match instead.
        format!("--bind=result:transform:[ -z {{q}} ] && echo 'pos({initial_pos})' || echo 'pos(3)'"),
        format!("--bind=down,ctrl-j,ctrl-n:{down}"),
        format!("--bind=up,ctrl-k,ctrl-p:{up}"),
    ]
}

/// When the invoking pane is a live agent, open on its row rather than the top.
fn initial_position(rows: &[Row], focus_id: Option<&str>) -> usize {
    focus_id
        .and_then(|id| rows.iter().position(|(row_id, _)| row_id == id))
        .map_or(3, |index| index + 1)
}

pub fn run_fzf(
    rows: &[Row],
    header: &str,
    specs: Option<&Cache>,
    focus_id: Option<&str>,
) -> Option<String> {
    let mut command = Command::new("fzf");
    // A user's FZF_DEFAULT_OPTS (--border, --height 40%) would fight the flags below.
    command.env("FZF_DEFAULT_OPTS", "").env("FZF_DEFAULT_OPTS_FILE", "");
    command.args([
        "--ansi",
        "--delimiter=\t",
        "--with-nth=2..",
        "--layout=reverse",
        "--info=inline",
        "--no-multi",
        "--height=100%",
        // Herdr's popup already draws a frame; a second one from fzf reads as a bug.
        "--border=none",
        "--margin=0",
        "--padding=0",
    ]);
    if !header.is_empty() {
        command.arg(format!("--header={header}"));
    }
    command.args(&settings().fzf_colors);

    // Herdr gives the plugin a state dir; the pid keeps concurrent popups apart.
    let mut cache_path = None;
    if let Some(cache) = specs {
        let state = env::var("HERDR_PLUGIN_STATE_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let path = format!("{state}/specs-{}.json", std::process::id());
        fs::write(&path, serde_json::to_string(cache).ok()?).ok()?;
        command.env("HERDR_PALETTE_SPECS", &path);
        // The reload runs on every keystroke, so it re-executes this very binary.
        let exe = env::current_exe().ok()?;
        command.args(reload_binds(&shell_quote(&exe.to_string_lossy()), initial_position(rows, focus_id)));
        cache_path = Some(path);
    }
    command.arg("--prompt=› ");

    let output = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(encode_rows(rows).as_bytes());
            }
            child.wait_with_output()
        });
    if let Some(path) = cache_path {
        let _ = fs::remove_file(path);
    }
    let output = output.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || stdout.trim().is_empty() {
        return None;
    }
    stdout.split('\t').next().map(|id| id.trim_end_matches('\n').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_leaves_safe_strings_alone() {
        assert_eq!(shell_quote("/usr/local/bin/herdr-palette"), "/usr/local/bin/herdr-palette");
    }

    #[test]
    fn shell_quote_wraps_spaces_and_escapes_quotes() {
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn fzf_theme_is_empty_without_a_background() {
        let config: Table = toml::from_str("[theme.custom]\naccent = \"#fff\"").unwrap();
        assert!(fzf_theme(&config).is_empty());
        assert!(fzf_theme(&Table::new()).is_empty());
    }

    #[test]
    fn fzf_theme_prefers_panel_bg_and_skips_missing_tokens() {
        let config: Table = toml::from_str(
            "[theme.custom]\npanel_bg = \"#111111\"\nsurface0 = \"#222222\"\naccent = \"#abcdef\"",
        )
        .unwrap();
        let args = fzf_theme(&config);
        assert_eq!(args.len(), 1);
        let arg = &args[0];
        assert!(arg.starts_with("--color=bg:#111111,"));
        assert!(arg.contains("hl:#abcdef"));
        assert!(arg.contains("border:#111111"));
        assert!(arg.contains("scrollbar:#111111"));
        assert!(!arg.contains("fg:"));
    }

    #[test]
    fn initial_position_defaults_to_first_selectable_row() {
        let rows = vec![("a".to_string(), String::new())];
        assert_eq!(initial_position(&rows, None), 3);
        assert_eq!(initial_position(&rows, Some("zzz")), 3);
    }

    #[test]
    fn initial_position_is_one_based_index_of_focus_row() {
        let rows: Vec<Row> = ["x", "y", "agent:p1"].iter().map(|id| (id.to_string(), String::new())).collect();
        assert_eq!(initial_position(&rows, Some("agent:p1")), 3);
        assert_eq!(initial_position(&rows, Some("y")), 2);
    }

    #[test]
    fn reload_binds_skip_group_headings() {
        let binds = reload_binds("/bin/p", 7);
        assert!(binds.contains(&"--disabled".to_string()));
        assert!(binds.contains(&"--bind=change:reload(/bin/p --rows {q})".to_string()));
        assert!(binds.iter().any(|b| b.contains("pos(7)") && b.contains("pos(3)")));
        let down = binds.iter().find(|b| b.starts_with("--bind=down,")).unwrap();
        assert_eq!(down.matches("+transform").count(), 2);
        assert!(down.contains("{1} = __group__"));
        let up = binds.iter().find(|b| b.starts_with("--bind=up,")).unwrap();
        assert_eq!(up.matches("+transform").count(), 3);
        assert!(up.ends_with("echo down+down || true)"));
    }
}
