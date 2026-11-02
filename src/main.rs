//! Fuzzy command palette for Herdr: search every action, see its shortcut, run it.

mod actions;
mod config;
mod context;
mod herdr;
mod keymap;
mod render;
mod ui;

use actions::{group_actions, Action, Snapshot};
use config::{
    apply_theme_colors, herdr_config_path, init_settings, load_toml, settings, Colors, PluginConfig,
    Settings, RESET,
};
use herdr::{herdr_bin, herdr_quiet, is_runnable, which};
use keymap::{Keymap, ModStyle};
use render::{render_rows, terminal_width, Cache, SEPARATOR};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::time::Instant;
use std::{env, fs, thread};

// Set HERDR_PALETTE_TRACE=/tmp/some.log to time startup from process start to first paint.
fn trace(label: &str) {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    let Some(path) = env::var("HERDR_PALETTE_TRACE").ok().filter(|p| !p.is_empty()) else {
        return;
    };
    let line = format!("{:7.1}ms  {label}\n", start.elapsed().as_secs_f64() * 1000.0);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// A popup pane is torn down the moment this process exits, and stderr goes
/// with it, so returning 1 here reads as the palette flashing and vanishing
/// rather than as an error. Hold the frame until a keypress instead.
fn fatal(problem: &str, remedy: &str) -> ExitCode {
    println!("\n  {problem}\n\n  {remedy}\n");
    println!("  PATH={}\n", env::var("PATH").unwrap_or_default());
    if std::io::stdin().is_terminal() {
        println!("  Press Enter to close.");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    ExitCode::FAILURE
}

/// Plugin panes inherit the Herdr server's environment. Started as a service
/// (launchd, systemd) that is a bare /usr/bin:/bin:/usr/sbin:/sbin, so neither
/// fzf nor herdr itself is reachable and the popup would die before it can paint.
/// Called before any thread starts: it mutates the process environment.
fn repair_path() {
    let home = env::var("HOME").unwrap_or_default();
    let candidates = [
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/local/sbin".to_string(),
        "/home/linuxbrew/.linuxbrew/bin".to_string(),
        format!("{home}/.nix-profile/bin"),
        format!("{home}/.local/bin"),
    ];
    let mut dirs: Vec<std::path::PathBuf> =
        candidates.iter().map(std::path::PathBuf::from).filter(|d| d.is_dir()).collect();
    dirs.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    if let Ok(joined) = env::join_paths(dirs) {
        env::set_var("PATH", joined);
    }
    // Anything the list above cannot guess (asdf, a custom prefix) is worth a
    // login shell, but only once the free lookup has already missed.
    if which("fzf").is_none() {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let output = Command::new(shell).args(["-lc", "printf %s \"$PATH\""]).output();
        if let Ok(output) = output {
            let path = String::from_utf8_lossy(&output.stdout).to_string();
            if !path.is_empty() {
                env::set_var("PATH", path);
            }
        }
    }
}

fn prefetch(args: &'static [&'static str]) -> thread::JoinHandle<Value> {
    thread::spawn(move || herdr_quiet(args))
}

fn collect(handle: thread::JoinHandle<Value>) -> Value {
    handle.join().unwrap_or_default()
}

fn run() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("--rows") {
        let query = args.get(2).map_or("", String::as_str);
        return if render::emit_rows(query) == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }

    repair_path();
    if which("fzf").is_none() {
        return fatal(
            "herdr-palette requires fzf on PATH.",
            "Install fzf, or start the Herdr server from a login shell so it inherits your PATH.",
        );
    }
    // herdr_bin() falls back to a bare "herdr" when PATH is too thin to resolve it,
    // which would otherwise fail later as an empty palette rather than an error.
    if !is_runnable(herdr_bin()) {
        return fatal(
            "herdr-palette cannot find the herdr binary.",
            "Set HERDR_BIN_PATH to its full path, or start the Herdr server from a login shell so it inherits your PATH.",
        );
    }

    trace("main entered");
    // The startup reads don't depend on each other, so they overlap with config
    // parsing and with one another instead of running as serial round-trips.
    let agents = prefetch(&["agent", "list"]);
    let workspaces = prefetch(&["workspace", "list"]);
    let plugin_actions = prefetch(&["plugin", "action", "list", "--json"]);
    let config = load_toml(&herdr_config_path());
    let plugin_config = PluginConfig::load();

    let mut colors = Colors::default();
    apply_theme_colors(&config, &mut colors);
    init_settings(Settings {
        colors: colors.clone(),
        glyphs: plugin_config.glyphs(),
        fzf_colors: ui::fzf_theme(&config),
    });
    let keymap = Keymap::new(&config, &plugin_config.leader, ModStyle::resolve(&plugin_config.modifier_style));
    let ctx = context::load_context();
    trace("config + context");

    let snapshot = Snapshot {
        agents: collect(agents),
        workspaces: collect(workspaces),
        plugin_actions: collect(plugin_actions),
    };
    let mut actions: Vec<Action> = plugin_config
        .group_order
        .iter()
        .flat_map(|group| group_actions(group, &config, &keymap, &snapshot))
        .collect();
    if !plugin_config.show_key_only {
        actions.retain(|a| !a.teach_only());
    }
    // A later action with the same id wins, as in a dict.
    let index: HashMap<String, usize> =
        actions.iter().enumerate().map(|(i, a)| (a.id.clone(), i)).collect();
    trace("actions built");

    let specs: Vec<_> = actions.iter().map(|a| a.spec(&keymap)).collect();
    let rows = render_rows(&specs, "", terminal_width(), &colors);
    // Bake resolved colors into the cache so the --rows reload path uses them.
    let cache = Cache { specs, colors: colors.clone() };
    trace("rows rendered, handing off to fzf");
    let focus_id = ctx.pane_id.as_ref().map(|p| format!("agent:{p}"));

    // Group headings are selectable in fzf; treat picking one as "keep looking".
    let chosen = loop {
        match ui::run_fzf(&rows, "", Some(&cache), focus_id.as_deref()) {
            None => return ExitCode::SUCCESS,
            Some(id) if id == SEPARATOR => continue,
            Some(id) => break id,
        }
    };
    let Some(action) = index.get(&chosen).map(|&i| &actions[i]) else {
        return ExitCode::FAILURE;
    };

    let colors = &settings().colors;
    let Some(run) = action.run.as_ref() else {
        let chord = action.spec(&keymap).keys;
        if chord.is_empty() {
            println!("{} is a client-side action with no binding configured.", action.title);
        } else {
            println!("{} is a client-side action: press {}{chord}{RESET}", action.title, colors.key);
        }
        ui::wait_for_enter("enter to close");
        return ExitCode::SUCCESS;
    };

    match run(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("\x1b[31m{err}{RESET}");
            ui::wait_for_enter("enter to close");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    run()
}
