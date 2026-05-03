//! `utm-dev mcp` — set up MCP servers for AI-assisted development.
//!
//! Writes:
//!   - `.mcp.json`            → server config (context7 + mise MCP servers)
//!   - `.claude/settings.json` → auto-allow permissions (so Claude Code
//!     doesn't prompt on every MCP call)
//!
//! Idempotent — merges into existing JSON, never clobbers unrelated keys.
//!
//! Ported from `joeblew999/utm-dev/.mise/tasks/mcp.ts` (was the only
//! feature in that legacy repo not yet in utm-dev-cli).

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

pub fn run() -> Result<()> {
    let root = std::env::current_dir().context("cwd")?;

    // Claude Code can't always find binaries via PATH (sandboxed shell), so
    // resolve to absolute paths via `mise where <tool>` first, then `which`,
    // then bare name as last-resort fallback.
    let bunx = resolve_bin("bunx", Some("bun"));
    let mise = resolve_bin("mise", None);

    let servers = vec![
        (
            "context7".to_string(),
            json!({
                "command": bunx,
                "args": ["@upstash/context7-mcp@latest"],
            }),
        ),
        (
            "mise".to_string(),
            json!({
                "command": mise,
                "args": ["mcp"],
                "env": { "MISE_EXPERIMENTAL": "true" },
            }),
        ),
    ];

    update_mcp_json(&root, &servers)?;
    update_claude_settings(&root, &servers)?;
    Ok(())
}

/// Add missing MCP servers to `.mcp.json`. Preserves any existing keys
/// (top-level or under `mcpServers`).
fn update_mcp_json(root: &Path, servers: &[(String, Value)]) -> Result<()> {
    let path = root.join(".mcp.json");
    let mut config = read_or_empty_object(&path)?;

    let map = config
        .as_object_mut()
        .context(".mcp.json must be a JSON object")?;
    let mcp_servers = map
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context(".mcp.json mcpServers must be an object")?;

    let mut added = 0;
    for (name, server) in servers {
        if mcp_servers.contains_key(name) {
            println!("✓ {name} (already configured)");
        } else {
            mcp_servers.insert(name.clone(), server.clone());
            println!("→ adding {name}");
            added += 1;
        }
    }

    if added > 0 {
        let mut out = serde_json::to_string_pretty(&config)?;
        out.push('\n');
        std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
        println!("✓ wrote {}", path.display());
    }
    Ok(())
}

/// Add MCP tool wildcard permissions to `.claude/settings.json` so Claude
/// Code doesn't prompt on every call. Creates `.claude/` if needed.
fn update_claude_settings(root: &Path, servers: &[(String, Value)]) -> Result<()> {
    let dir = root.join(".claude");
    let path = dir.join("settings.json");
    let mut settings = read_or_empty_object(&path)?;

    let map = settings
        .as_object_mut()
        .context(".claude/settings.json must be a JSON object")?;
    let permissions = map
        .entry("permissions".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("permissions must be an object")?;
    let allow = permissions
        .entry("allow".to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("permissions.allow must be an array")?;

    let mut added = 0;
    for (name, _) in servers {
        let perm = format!("mcp__{name}__*");
        if allow.iter().any(|v| v.as_str() == Some(&perm)) {
            continue;
        }
        allow.push(Value::String(perm.clone()));
        println!("→ allowing {perm}");
        added += 1;
    }

    if added > 0 {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut out = serde_json::to_string_pretty(&settings)?;
        out.push('\n');
        std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
        println!("✓ wrote {}", path.display());
    } else {
        println!("✓ permissions already configured");
    }
    Ok(())
}

/// Read JSON file or return `{}` if missing. Errors only on parse failure.
fn read_or_empty_object(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// Resolve a binary's absolute path: try `mise where <tool>`/bin/<name>,
/// then `which <name>`, then the bare name.
fn resolve_bin(name: &str, mise_tool: Option<&str>) -> String {
    if let Some(tool) = mise_tool
        && let Ok(out) = Command::new("mise").args(["where", tool]).output()
        && out.status.success()
    {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !dir.is_empty() {
            let full = Path::new(&dir).join("bin").join(name);
            if full.exists() {
                return full.to_string_lossy().into_owned();
            }
        }
    }
    if let Ok(p) = which::which(name) {
        return p.to_string_lossy().into_owned();
    }
    name.to_string()
}
