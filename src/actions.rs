//! Every row the palette can show, and what running each one does.

use crate::config::{settings, status_rank, tilde, RESET};
use crate::context::Context;
use crate::herdr::{display, get_str, herdr, items, HerdrError, Result};
use crate::keymap::Keymap;
use crate::render::Spec;
use crate::ui::{ask, confirm, pick};
use serde_json::Value;
use std::process::{Command, Stdio};
use toml::Table;

pub type Runner = Box<dyn Fn(&Context) -> Result<()>>;

pub struct Action {
    pub id: String,
    pub title: String,
    pub category: String,
    /// Config key name this action is bound to.
    pub key: Option<String>,
    /// `None` for client-side modes the palette can only describe.
    pub run: Option<Runner>,
    /// Pre-rendered, for dynamic rows with no config key.
    pub chord: String,
    pub chord_color: String,
    /// Dim trailing text, already coloured.
    pub note: String,
    /// Keep the title verbatim instead of stripping the category word.
    pub raw_title: bool,
}

impl Action {
    fn new(id: impl Into<String>, title: impl Into<String>, category: &str) -> Self {
        Action {
            id: id.into(),
            title: title.into(),
            category: category.to_string(),
            key: None,
            run: None,
            chord: String::new(),
            chord_color: String::new(),
            note: String::new(),
            raw_title: false,
        }
    }

    fn key(mut self, name: impl Into<String>) -> Self {
        self.key = Some(name.into());
        self
    }

    fn run(mut self, run: impl Fn(&Context) -> Result<()> + 'static) -> Self {
        self.run = Some(Box::new(run));
        self
    }

    pub fn teach_only(&self) -> bool {
        self.run.is_none()
    }

    /// Flatten to plain data so re-rendering per keystroke stays cheap.
    pub fn spec(&self, keymap: &Keymap) -> Spec {
        Spec {
            id: self.id.clone(),
            cat: self.category.clone(),
            title: if self.raw_title {
                self.title.clone()
            } else {
                crate::render::strip_category(&self.title, &self.category)
            },
            keys: if self.chord.is_empty() {
                keymap.chord(self.key.as_deref())
            } else {
                self.chord.clone()
            },
            keycolor: self.chord_color.clone(),
            note: self.note.clone(),
            teach: self.teach_only(),
        }
    }
}

/// Live state fetched once at startup, shared by every dynamic group.
#[derive(Clone)]
pub struct Snapshot {
    pub agents: Value,
    pub workspaces: Value,
    pub plugin_actions: Value,
}

fn call<S: AsRef<str>>(args: &[S]) -> Result<()> {
    herdr(args).map(|_| ())
}

fn need<'a>(value: &'a Option<String>, what: &str) -> Result<&'a str> {
    value.as_deref().ok_or_else(|| HerdrError(format!("no {what} in context")))
}

/// Focus the previous/next tab or workspace by position in the live list.
fn relative_focus(kind: &str, offset: isize) -> Result<()> {
    let (list_key, id_key) = if kind == "tab" { ("tabs", "tab_id") } else { ("workspaces", "workspace_id") };
    let entries = items(&herdr(&[kind, "list"])?, list_key);
    if entries.is_empty() {
        return Err(HerdrError(format!("no {kind}s")));
    }
    let focused = entries
        .iter()
        .position(|e| e.get("focused").and_then(Value::as_bool).unwrap_or(false))
        .unwrap_or(0);
    let target = (focused as isize + offset).rem_euclid(entries.len() as isize) as usize;
    call(&[kind, "focus", get_str(&entries[target], id_key)])
}

fn numbered_rows(entries: &[Value], id_key: &str) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|e| {
            let id = get_str(e, id_key);
            let label = Some(get_str(e, "label")).filter(|l| !l.is_empty()).unwrap_or(id);
            (id.to_string(), format!("{}  {label}", display(e.get("number"))))
        })
        .collect()
}

fn rename_pane(ctx: &Context) -> Result<()> {
    match ask("New pane name", "") {
        Some(label) => call(&["pane", "rename", need(&ctx.pane_id, "pane")?, &label]),
        None => Ok(()),
    }
}

fn rename_tab(ctx: &Context) -> Result<()> {
    match ask("New tab name", "") {
        Some(label) => call(&["tab", "rename", need(&ctx.tab_id, "tab")?, &label]),
        None => Ok(()),
    }
}

fn switch_tab(_: &Context) -> Result<()> {
    let rows = numbered_rows(&items(&herdr(&["tab", "list"])?, "tabs"), "tab_id");
    match pick(&rows, "switch to tab") {
        Some(tab_id) => call(&["tab", "focus", &tab_id]),
        None => Ok(()),
    }
}

fn new_workspace(_: &Context) -> Result<()> {
    let mut args = vec!["workspace", "create", "--focus"];
    let label = ask("Workspace name (blank for default)", "");
    if let Some(label) = label.as_deref() {
        args.extend(["--label", label]);
    }
    call(&args)
}

fn rename_workspace(ctx: &Context) -> Result<()> {
    match ask("New workspace name", "") {
        Some(label) => call(&["workspace", "rename", need(&ctx.workspace_id, "workspace")?, &label]),
        None => Ok(()),
    }
}

fn switch_workspace(_: &Context) -> Result<()> {
    let rows = numbered_rows(&items(&herdr(&["workspace", "list"])?, "workspaces"), "workspace_id");
    match pick(&rows, "switch to workspace") {
        Some(workspace_id) => call(&["workspace", "focus", &workspace_id]),
        None => Ok(()),
    }
}

fn close_workspace(ctx: &Context) -> Result<()> {
    let workspace_id = need(&ctx.workspace_id, "workspace")?;
    if confirm(&format!("Close workspace {workspace_id}?")) {
        call(&["workspace", "close", workspace_id])?;
    }
    Ok(())
}

fn with_workspace<'a>(mut args: Vec<&'a str>, ctx: &'a Context) -> Vec<&'a str> {
    if let Some(workspace_id) = ctx.workspace_id.as_deref() {
        args.extend(["--workspace", workspace_id]);
    }
    args
}

fn new_worktree(ctx: &Context) -> Result<()> {
    let Some(branch) = ask("Branch name", "") else { return Ok(()) };
    call(&with_workspace(vec!["worktree", "create", "--branch", &branch, "--focus"], ctx))
}

fn worktree_rows(ctx: &Context) -> Result<Vec<(String, String)>> {
    let listing = herdr(&with_workspace(vec!["worktree", "list", "--json"], ctx))?;
    let dim = &settings().colors.dim;
    Ok(items(&listing, "worktrees")
        .iter()
        .map(|tree| {
            let path = get_str(tree, "path");
            let branch = Some(get_str(tree, "branch")).filter(|b| !b.is_empty()).unwrap_or(path);
            (branch.to_string(), format!("{branch}  {dim}{path}{RESET}"))
        })
        .collect())
}

fn open_worktree(ctx: &Context) -> Result<()> {
    match pick(&worktree_rows(ctx)?, "open worktree") {
        Some(branch) => call(&with_workspace(vec!["worktree", "open", "--branch", &branch, "--focus"], ctx)),
        None => Ok(()),
    }
}

fn remove_worktree(ctx: &Context) -> Result<()> {
    let workspace_id = need(&ctx.workspace_id, "workspace")?;
    if confirm(&format!("Remove worktree checkout for {workspace_id}?")) {
        call(&["worktree", "remove", "--workspace", workspace_id])?;
    }
    Ok(())
}

pub struct AgentEntry {
    pub pane_id: String,
    pub number: String,
    pub label: String,
    pub cwd: String,
    pub status: String,
    pub title: String,
}

/// Live agents, most demanding first. `agent list` reports workspace_id only;
/// the human-readable number and label live on the workspace record.
pub fn agent_entries(snapshot: &Snapshot) -> Vec<AgentEntry> {
    let workspaces = items(&snapshot.workspaces, "workspaces");
    let mut entries: Vec<AgentEntry> = items(&snapshot.agents, "agents")
        .iter()
        .filter_map(|agent| {
            let pane_id = Some(get_str(agent, "pane_id")).filter(|p| !p.is_empty())?;
            let workspace_id = get_str(agent, "workspace_id");
            let ws = workspaces.iter().find(|w| get_str(w, "workspace_id") == workspace_id);
            let label = ws.map(|w| get_str(w, "label")).filter(|l| !l.is_empty()).unwrap_or(workspace_id);
            let cwd = Some(get_str(agent, "foreground_cwd"))
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| get_str(agent, "cwd"));
            let status = Some(get_str(agent, "agent_status")).filter(|s| !s.is_empty()).unwrap_or("unknown");
            Some(AgentEntry {
                pane_id: pane_id.to_string(),
                number: ws.map(|w| display(w.get("number"))).unwrap_or_default(),
                label: label.to_string(),
                cwd: tilde(cwd),
                status: status.to_string(),
                title: get_str(agent, "terminal_title_stripped").to_string(),
            })
        })
        .collect();
    // Workspace order within a status, so a row doesn't move unless its
    // status actually changed.
    entries.sort_by(|a, b| {
        let number = |e: &AgentEntry| e.number.parse::<u64>().unwrap_or(1 << 30);
        (status_rank(&a.status), number(a), &a.label).cmp(&(status_rank(&b.status), number(b), &b.label))
    });
    entries
}

fn focus_agent_picker(snapshot: Snapshot) -> impl Fn(&Context) -> Result<()> {
    move |_| {
        let entries = agent_entries(&snapshot);
        let width = |f: fn(&AgentEntry) -> &str| entries.iter().map(|e| f(e).chars().count()).max().unwrap_or(0);
        let (number_w, label_w, title_w, cwd_w) =
            (width(|e| &e.number), width(|e| &e.label), width(|e| &e.title), width(|e| &e.cwd));
        let colors = &settings().colors;
        let rows: Vec<(String, String)> = entries
            .iter()
            .map(|e| {
                let (glyph, color) = settings().glyphs.get(&e.status);
                (
                    e.pane_id.clone(),
                    format!(
                        "{:<number_w$}  {:<label_w$}  {:<title_w$}  {}{:<cwd_w$}{RESET}  {color}{glyph} {}{RESET}",
                        e.number, e.label, e.title, colors.dim, e.cwd, e.status
                    ),
                )
            })
            .collect();
        match pick(&rows, "focus agent") {
            Some(target) => call(&["agent", "focus", &target]),
            None => Ok(()),
        }
    }
}

const DIRECTIONS: [&str; 4] = ["left", "down", "up", "right"];

pub fn build_actions(snapshot: Snapshot) -> Vec<Action> {
    let mut actions = vec![
        // --focus follows the split: you asked for a new pane, so you want to be in it.
        Action::new("split_right", "Split pane right", "pane")
            .key("split_vertical")
            .run(|_| call(&["pane", "split", "--current", "--direction", "right", "--focus"])),
        Action::new("split_down", "Split pane down", "pane")
            .key("split_horizontal")
            .run(|_| call(&["pane", "split", "--current", "--direction", "down", "--focus"])),
        Action::new("zoom", "Toggle pane zoom", "pane")
            .key("zoom")
            .run(|c| call(&["pane", "zoom", "--pane", need(&c.pane_id, "pane")?])),
        Action::new("close_pane", "Close pane", "pane")
            .key("close_pane")
            .run(|c| call(&["pane", "close", need(&c.pane_id, "pane")?])),
        Action::new("rename_pane", "Rename pane…", "pane").key("rename_pane").run(rename_pane),
    ];
    for direction in DIRECTIONS {
        actions.push(
            Action::new(format!("focus_pane_{direction}"), format!("Focus pane {direction}"), "pane")
                .key(format!("focus_pane_{direction}"))
                .run(move |_| call(&["pane", "focus", "--current", "--direction", direction])),
        );
    }
    for direction in DIRECTIONS {
        actions.push(
            Action::new(format!("swap_pane_{direction}"), format!("Swap pane {direction}"), "pane")
                .key(format!("swap_pane_{direction}"))
                .run(move |_| call(&["pane", "swap", "--current", "--direction", direction])),
        );
    }
    // Client-side modes: the palette can only point at their shortcut.
    for (id, title, category) in [
        ("copy_mode", "Copy mode", "pane"),
        ("resize_mode", "Resize mode", "pane"),
        ("edit_scrollback", "Edit scrollback", "pane"),
        ("cycle_pane_next", "Cycle to next pane", "pane"),
        ("cycle_pane_previous", "Cycle to previous pane", "pane"),
        ("last_pane", "Last pane", "pane"),
    ] {
        actions.push(Action::new(id, title, category).key(id));
    }
    actions.extend([
        Action::new("new_tab", "New tab", "tab").key("new_tab").run(|_| call(&["tab", "create", "--focus"])),
        Action::new("rename_tab", "Rename tab…", "tab").key("rename_tab").run(rename_tab),
        Action::new("close_tab", "Close tab", "tab")
            .key("close_tab")
            .run(|c| call(&["tab", "close", need(&c.tab_id, "tab")?])),
        Action::new("switch_tab", "Switch to tab…", "tab").key("switch_tab").run(switch_tab),
        Action::new("previous_tab", "Previous tab", "tab").key("previous_tab").run(|_| relative_focus("tab", -1)),
        Action::new("next_tab", "Next tab", "tab").key("next_tab").run(|_| relative_focus("tab", 1)),
        Action::new("new_workspace", "New workspace…", "workspace").key("new_workspace").run(new_workspace),
        Action::new("rename_workspace", "Rename workspace…", "workspace")
            .key("rename_workspace")
            .run(rename_workspace),
        Action::new("switch_workspace", "Switch to workspace…", "workspace")
            .key("workspace_picker")
            .run(switch_workspace),
        Action::new("previous_workspace", "Previous workspace", "workspace")
            .key("previous_workspace")
            .run(|_| relative_focus("workspace", -1)),
        Action::new("next_workspace", "Next workspace", "workspace")
            .key("next_workspace")
            .run(|_| relative_focus("workspace", 1)),
        Action::new("close_workspace", "Close workspace", "workspace").key("close_workspace").run(close_workspace),
        Action::new("new_worktree", "New worktree…", "worktree").key("new_worktree").run(new_worktree),
        Action::new("open_worktree", "Open worktree…", "worktree").key("open_worktree").run(open_worktree),
        Action::new("remove_worktree", "Remove worktree", "worktree").key("remove_worktree").run(remove_worktree),
        Action::new("focus_agent", "Focus agent…", "agent").key("focus_agent").run(focus_agent_picker(snapshot)),
        Action::new("previous_agent", "Previous agent", "agent").key("previous_agent"),
        Action::new("next_agent", "Next agent", "agent").key("next_agent"),
        Action::new("goto", "Session navigator", "session").key("goto"),
        Action::new("help", "Keybinding help", "session").key("help"),
        Action::new("settings", "Settings", "session").key("settings"),
        Action::new("toggle_sidebar", "Toggle sidebar", "session").key("toggle_sidebar"),
        Action::new("detach", "Detach session", "session").key("detach"),
        Action::new("open_notification_target", "Open notification target", "session")
            .key("open_notification_target"),
        Action::new("reload_config", "Reload config", "session")
            .key("reload_config")
            .run(|_| call(&["server", "reload-config"])),
    ]);
    actions
}

fn plural(count: u64) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// One row per live agent, so focusing one is a single pick, not two.
pub fn live_agent_actions(snapshot: &Snapshot) -> Vec<Action> {
    let entries = agent_entries(snapshot);
    let number_w = entries.iter().map(|e| e.number.chars().count()).max().unwrap_or(0);
    // Leaves room for the glyph and number inside the palette's title column,
    // which render_rows caps; past it the status column would be shoved right.
    let title_room = (44 - number_w as isize - 4).max(1) as usize;
    let dim = &settings().colors.dim;
    entries
        .into_iter()
        .map(|e| {
            let (glyph, color) = settings().glyphs.get(&e.status);
            // The session title leads because it is what tells agents in one
            // workspace apart; the workspace label moves to the note and still matches.
            let name = if e.title.is_empty() { &e.label } else { &e.title };
            let name: String = if name.chars().count() > title_room {
                name.chars().take(title_room - 1).chain(['…']).collect()
            } else {
                name.clone()
            };
            let pane_id = e.pane_id.clone();
            let mut action = Action::new(
                format!("agent:{}", e.pane_id),
                // Padded here rather than by render_rows: the title column is sized
                // across every action, and these two sub-columns should line up
                // with each other, not with "Split pane right".
                format!("{glyph} {:<number_w$}  {name}", e.number),
                "agents",
            )
            .run(move |_| call(&["agent", "focus", &pane_id]));
            action.chord = e.status.clone();
            action.chord_color = color.clone();
            action.note = if e.title.is_empty() {
                format!("{dim}{}{RESET}", e.cwd)
            } else {
                format!("{dim}{}  {}{RESET}", e.label, e.cwd)
            };
            action.raw_title = true;
            action
        })
        .collect()
}

/// One row per open workspace, so switching skips the nested picker.
pub fn workspace_actions(snapshot: &Snapshot) -> Vec<Action> {
    let mut workspaces = items(&snapshot.workspaces, "workspaces");
    let number = |w: &Value| w.get("number").and_then(Value::as_i64).unwrap_or(1 << 30);
    workspaces.sort_by_key(number);
    let number_w = workspaces.iter().map(|w| display(w.get("number")).chars().count()).max().unwrap_or(0);
    let dim = &settings().colors.dim;
    workspaces
        .iter()
        .map(|ws| {
            let id = get_str(ws, "workspace_id").to_string();
            let status = get_str(ws, "agent_status");
            let (glyph, color) = settings().glyphs.get(status);
            let label = Some(get_str(ws, "label")).filter(|l| !l.is_empty()).unwrap_or(&id);
            let count = |key: &str| ws.get(key).and_then(Value::as_u64).unwrap_or(0);
            let (tabs, panes) = (count("tab_count"), count("pane_count"));
            let focus_id = id.clone();
            let mut action = Action::new(
                format!("workspace:{id}"),
                format!("{glyph} {:<number_w$}  {label}", display(ws.get("number"))),
                "workspaces",
            )
            .run(move |_| call(&["workspace", "focus", &focus_id]));
            action.chord = status.to_string();
            action.chord_color = color.clone();
            action.note =
                format!("{dim}{tabs} tab{} · {panes} pane{}{RESET}", plural(tabs), plural(panes));
            action.raw_title = true;
            action
        })
        .collect()
}

/// Every action contributed by other installed plugins, for free.
pub fn plugin_actions(snapshot: &Snapshot) -> Vec<Action> {
    items(&snapshot.plugin_actions, "actions")
        .iter()
        .filter(|item| get_str(item, "plugin_id") != "palette")
        .map(|item| {
            let plugin_id = get_str(item, "plugin_id").to_string();
            let action_id = get_str(item, "action_id").to_string();
            let title = Some(get_str(item, "title")).filter(|t| !t.is_empty()).unwrap_or(&action_id);
            Action::new(format!("plugin:{plugin_id}:{action_id}"), title, &format!("plugin:{plugin_id}")).run(
                move |_| call(&["plugin", "action", "invoke", &action_id, "--plugin", &plugin_id]),
            )
        })
        .collect()
}

fn shell_runner(command: String) -> impl Fn(&Context) -> Result<()> {
    move |ctx| {
        let mut process = Command::new("sh");
        process.arg("-c").arg(&command).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(cwd) = ctx.cwd.as_deref() {
            process.current_dir(cwd);
        }
        process.spawn().map(|_| ()).map_err(|e| HerdrError(format!("cannot run `{command}`: {e}")))
    }
}

/// User-defined `[[keys.command]]` entries, shown with their shortcuts.
pub fn custom_commands(config: &Table, keymap: &Keymap) -> Vec<Action> {
    let entries = config
        .get("keys")
        .and_then(|k| k.get("command"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let text = |entry: &toml::Value, name: &str| entry.get(name).and_then(|v| v.as_str()).unwrap_or("").to_string();
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let command = text(entry, "command");
            if command.is_empty() || command.contains("--plugin palette") || command.contains("--plugin=palette") {
                return None;
            }
            let kind = Some(text(entry, "type")).filter(|t| !t.is_empty()).unwrap_or_else(|| "shell".into());
            let description = text(entry, "description");
            let key = text(entry, "key");
            let mut action = Action::new(
                format!("custom:{index}"),
                if description.is_empty() { command.clone() } else { description },
                &format!("custom:{kind}"),
            );
            if !key.is_empty() {
                action.chord = keymap.render(&key);
            }
            // Only `shell` commands are safe to run from inside the palette popup;
            // popup/pane types need the client to own the terminal, which it can't
            // while this popup is on screen.
            if kind == "shell" {
                action = action.run(shell_runner(command));
            }
            Some(action)
        })
        .collect()
}

pub fn group_actions(group: &str, config: &Table, keymap: &Keymap, snapshot: &Snapshot) -> Vec<Action> {
    match group {
        "agents" => live_agent_actions(snapshot),
        "workspaces" => workspace_actions(snapshot),
        "actions" => build_actions(snapshot.clone()),
        "custom" => custom_commands(config, keymap),
        "plugins" => plugin_actions(snapshot),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{init_settings, Colors, Glyphs, Settings};
    use crate::keymap::ModStyle;
    use serde_json::json;

    fn init() {
        init_settings(Settings { colors: Colors::default(), glyphs: Glyphs::default(), fzf_colors: vec![] });
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            agents: json!({"agents": [
                {"pane_id": "w1:p1", "workspace_id": "w1", "agent_status": "idle", "cwd": "/tmp/a",
                 "terminal_title_stripped": "fix bug"},
                {"pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "blocked", "cwd": "/tmp/b"},
                {"pane_id": "w1:p2", "workspace_id": "w1", "agent_status": "working", "cwd": "/tmp/c"},
                {"workspace_id": "w1", "agent_status": "idle"}
            ]}),
            workspaces: json!({"workspaces": [
                {"workspace_id": "w2", "number": 2, "label": "beta", "agent_status": "blocked",
                 "tab_count": 1, "pane_count": 2},
                {"workspace_id": "w1", "number": 1, "label": "alpha", "agent_status": "idle",
                 "tab_count": 3, "pane_count": 1}
            ]}),
            plugin_actions: json!({"actions": [
                {"plugin_id": "palette", "action_id": "open", "title": "Open palette"},
                {"plugin_id": "bar", "action_id": "open", "title": "Open bar"},
                {"plugin_id": "bar", "action_id": "stop"}
            ]}),
        }
    }

    fn keymap() -> Keymap {
        Keymap::new(&Table::new(), "", ModStyle::Text)
    }

    #[test]
    fn agents_sort_by_status_then_workspace_and_skip_paneless() {
        init();
        let entries = agent_entries(&snapshot());
        let order: Vec<(&str, &str)> = entries.iter().map(|e| (e.pane_id.as_str(), e.status.as_str())).collect();
        assert_eq!(order, [("w2:p1", "blocked"), ("w1:p2", "working"), ("w1:p1", "idle")]);
        assert_eq!(entries[0].label, "beta");
        assert_eq!(entries[0].number, "2");
    }

    #[test]
    fn agent_rows_lead_with_title_and_note_workspace() {
        init();
        let actions = live_agent_actions(&snapshot());
        let idle = actions.iter().find(|a| a.id == "agent:w1:p1").unwrap();
        assert!(idle.title.ends_with("fix bug"));
        assert!(idle.note.contains("alpha"));
        assert_eq!(idle.chord, "idle");
        assert!(idle.raw_title);
        let untitled = actions.iter().find(|a| a.id == "agent:w2:p1").unwrap();
        assert!(untitled.title.ends_with("beta"));
        assert!(!untitled.note.contains("beta"));
    }

    #[test]
    fn long_agent_titles_are_truncated_with_ellipsis() {
        init();
        let mut snap = snapshot();
        snap.agents = json!({"agents": [{"pane_id": "p", "workspace_id": "w1", "agent_status": "idle",
            "terminal_title_stripped": "x".repeat(100)}]});
        let actions = live_agent_actions(&snap);
        assert!(actions[0].title.ends_with('…'));
        assert!(actions[0].title.chars().count() < 60);
    }

    #[test]
    fn workspace_rows_sorted_by_number_with_counts() {
        init();
        let actions = workspace_actions(&snapshot());
        assert_eq!(actions[0].id, "workspace:w1");
        assert!(actions[0].note.contains("3 tabs · 1 pane"));
        assert!(actions[1].note.contains("1 tab · 2 panes"));
    }

    #[test]
    fn plugin_rows_skip_the_palette_itself() {
        let actions = plugin_actions(&snapshot());
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["plugin:bar:open", "plugin:bar:stop"]);
        assert_eq!(actions[1].title, "stop");
        assert_eq!(actions[0].category, "plugin:bar");
    }

    #[test]
    fn custom_commands_skip_palette_and_only_shell_is_runnable() {
        let config: Table = toml::from_str(
            r#"
            [[keys.command]]
            key = "prefix+space"
            command = "herdr plugin pane open --plugin palette --entrypoint palette"

            [[keys.command]]
            key = "prefix+t"
            command = "echo hi"
            description = "Say hi"

            [[keys.command]]
            command = "htop"
            type = "popup"
            "#,
        )
        .unwrap();
        let actions = custom_commands(&config, &keymap());
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].title, "Say hi");
        assert_eq!(actions[0].category, "custom:shell");
        assert_eq!(actions[0].chord, "Ctrl+b t");
        assert!(!actions[0].teach_only());
        assert_eq!(actions[1].title, "htop");
        assert!(actions[1].teach_only());
    }

    #[test]
    fn builtin_actions_cover_every_group_and_flag_key_only_rows() {
        init();
        let actions = build_actions(snapshot());
        let categories: std::collections::BTreeSet<&str> = actions.iter().map(|a| a.category.as_str()).collect();
        assert_eq!(
            categories.into_iter().collect::<Vec<_>>(),
            ["agent", "pane", "session", "tab", "workspace", "worktree"]
        );
        let find = |id: &str| actions.iter().find(|a| a.id == id).unwrap();
        assert!(find("copy_mode").teach_only());
        assert!(!find("split_right").teach_only());
        assert_eq!(find("split_right").key.as_deref(), Some("split_vertical"));
        let ids: std::collections::HashSet<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids.len(), actions.len());
    }

    #[test]
    fn spec_strips_category_unless_raw_and_resolves_chord() {
        init();
        let actions = build_actions(snapshot());
        let split = actions.iter().find(|a| a.id == "split_right").unwrap().spec(&keymap());
        assert_eq!(split.title, "Split right");
        assert_eq!(split.keys, "Ctrl+b v");
        assert!(!split.teach);
        let agent_rows = live_agent_actions(&snapshot());
        let spec = agent_rows[0].spec(&keymap());
        assert_eq!(spec.keys, "blocked");
        assert!(spec.title.starts_with('▲'));
    }

    #[test]
    fn need_reports_missing_context() {
        let err = need(&None, "pane").unwrap_err();
        assert_eq!(err.to_string(), "no pane in context");
        assert_eq!(need(&Some("w1:p1".into()), "pane").unwrap(), "w1:p1");
    }

    #[test]
    fn with_workspace_appends_flag_only_when_known() {
        let mut ctx = Context::default();
        assert_eq!(with_workspace(vec!["a"], &ctx), ["a"]);
        ctx.workspace_id = Some("w3".into());
        assert_eq!(with_workspace(vec!["a"], &ctx), ["a", "--workspace", "w3"]);
    }
}
