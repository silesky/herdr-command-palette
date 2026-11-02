//! The pane the palette was opened from.

use crate::herdr::{get_str, herdr_quiet};
use serde_json::Value;
use std::env;

#[derive(Debug, Default, Clone)]
pub struct Context {
    pub pane_id: Option<String>,
    pub tab_id: Option<String>,
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
}

fn env_var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

/// Opened as a plugin pane, the invoking pane arrives as JSON in the env.
fn plugin_context() -> Value {
    env_var("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or(Value::Null)
}

fn plugin_field(plugin: &Value, key: &str) -> Option<String> {
    Some(get_str(plugin, key)).filter(|v| !v.is_empty()).map(String::from)
}

/// Three launch paths, three shapes: custom keybindings export HERDR_ACTIVE_*,
/// plugin actions export the shorter HERDR_* names, and a plugin pane gets
/// HERDR_PLUGIN_CONTEXT_JSON instead of either.
pub fn load_context() -> Context {
    let plugin = plugin_context();
    let mut ctx = Context {
        pane_id: env_var("HERDR_ACTIVE_PANE_ID")
            .or_else(|| env_var("HERDR_PANE_ID"))
            .or_else(|| plugin_field(&plugin, "pane_id")),
        tab_id: env_var("HERDR_ACTIVE_TAB_ID")
            .or_else(|| env_var("HERDR_TAB_ID"))
            .or_else(|| plugin_field(&plugin, "tab_id")),
        workspace_id: env_var("HERDR_ACTIVE_WORKSPACE_ID")
            .or_else(|| env_var("HERDR_WORKSPACE_ID"))
            .or_else(|| plugin_field(&plugin, "workspace_id")),
        cwd: env_var("HERDR_ACTIVE_PANE_CWD").or_else(|| plugin_field(&plugin, "workspace_cwd")),
    };
    if ctx.pane_id.is_none() {
        let current = herdr_quiet(&["pane", "current"]);
        let pane = current.get("pane").cloned().unwrap_or(Value::Null);
        ctx.pane_id = plugin_field(&pane, "pane_id");
        ctx.tab_id = ctx.tab_id.or_else(|| plugin_field(&pane, "tab_id"));
        ctx.workspace_id = ctx.workspace_id.or_else(|| plugin_field(&pane, "workspace_id"));
    }
    ctx
}
