use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use ft_core::vault::Vault;

#[derive(Args)]
pub struct VaultArgs;

pub fn run(_args: VaultArgs, vault_flag: Option<PathBuf>) -> Result<()> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    println!("Vault: {}", vault.path.display());
    println!();
    println!("Config files (lowest → highest precedence):");
    for (i, src) in vault.config.sources.iter().enumerate() {
        let status = if src.present { "present" } else { "not found" };
        println!(
            "  [{}] {} ({}): {}",
            i + 1,
            src.path.display(),
            src.label,
            status
        );
    }

    println!();
    println!("Merged config:");
    let cfg = &vault.config.config;
    print_opt("default_vault", cfg.default_vault.as_deref());
    print_opt(
        "default_task_location",
        cfg.default_task_location.as_deref(),
    );
    println!("  daily_notes:");
    let source_label = match cfg.daily_notes.source {
        ft_core::config::DailySource::Core => "core",
        ft_core::config::DailySource::PeriodicNotes => "periodic-notes",
        ft_core::config::DailySource::Explicit => "explicit",
    };
    println!("    source = {:?}", source_label);
    print_opt("    path", cfg.daily_notes.path.as_deref());
    print_opt("    format", cfg.daily_notes.format.as_deref());
    if cfg.ignored_paths.is_empty() {
        println!("  ignored_paths = []");
    } else {
        println!("  ignored_paths = {:?}", cfg.ignored_paths);
    }
    if cfg.presets.is_empty() {
        println!("  presets = {{}}");
    } else {
        println!("  presets:");
        let mut keys: Vec<&String> = cfg.presets.keys().collect();
        keys.sort();
        for k in keys {
            println!("    {} = {:?}", k, cfg.presets[k]);
        }
    }

    Ok(())
}

fn print_opt(key: &str, val: Option<&str>) {
    match val {
        Some(v) => println!("  {} = {:?}", key, v),
        None => println!("  {} = (not set)", key),
    }
}
