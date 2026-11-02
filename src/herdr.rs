//! Thin wrapper over the `herdr` CLI.

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::{env, fmt};

#[derive(Debug)]
pub struct HerdrError(pub String);

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HerdrError {}

pub type Result<T> = std::result::Result<T, HerdrError>;

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

pub fn is_runnable(program: &str) -> bool {
    which(program).is_some() || is_executable(Path::new(program))
}

/// Resolved on first use, so callers fix up PATH before the first herdr call.
pub fn herdr_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        env::var("HERDR_BIN_PATH")
            .ok()
            .filter(|p| !p.is_empty())
            .or_else(|| which("herdr").map(|p| p.display().to_string()))
            .unwrap_or_else(|| "herdr".to_string())
    })
}

pub fn herdr<S: AsRef<str>>(args: &[S]) -> Result<Value> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let out = Command::new(herdr_bin())
        .args(&args)
        .output()
        .map_err(|e| HerdrError(format!("cannot run {}: {e}", herdr_bin())))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let message = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
        return Err(HerdrError(if message.is_empty() {
            format!("herdr {} failed", args.join(" "))
        } else {
            message.to_string()
        }));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = stdout.trim();
    if text.is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("result").cloned())
        .unwrap_or_else(|| json!({})))
}

/// Best-effort call for optional data (plugin lists, agent lists).
pub fn herdr_quiet<S: AsRef<str>>(args: &[S]) -> Value {
    herdr(args).unwrap_or_else(|_| json!({}))
}

pub fn items(value: &Value, key: &str) -> Vec<Value> {
    value.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

pub fn get_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Strings and numbers print as-is; anything else (missing, null) is empty.
pub fn display(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}
