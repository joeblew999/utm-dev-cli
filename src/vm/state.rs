use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmState {
    pub uuid: String,
    pub display_name: String,
}

fn state_dir() -> Result<PathBuf> {
    // Store per-project state in .mise/state/ relative to cwd, matching TypeScript behaviour.
    let mut p = std::env::current_dir()?;
    p.push(".mise");
    p.push("state");
    std::fs::create_dir_all(&p)?;
    Ok(p)
}

fn state_file(vm_name: &str) -> Result<PathBuf> {
    Ok(state_dir()?.join(format!("vm-{vm_name}.json")))
}

pub fn load(vm_name: &str) -> Result<VmState> {
    let path = state_file(vm_name)?;
    if !path.exists() {
        bail!(
            "No VM state for '{}' — run: utm-dev vm up --name {}",
            vm_name,
            vm_name
        );
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| "parsing VM state")
}

pub fn save(vm_name: &str, state: &VmState) -> Result<()> {
    let path = state_file(vm_name)?;
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

pub fn exists(vm_name: &str) -> bool {
    state_file(vm_name).map(|p| p.exists()).unwrap_or(false)
}

#[allow(dead_code)]
pub fn clear(vm_name: &str) -> Result<()> {
    let path = state_file(vm_name)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
