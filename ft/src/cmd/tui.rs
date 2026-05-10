use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use ft_core::vault::Vault;

use crate::tui;

#[derive(Args)]
pub struct TuiArgs;

pub fn run(_args: TuiArgs, vault_flag: Option<PathBuf>) -> Result<()> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;
    tui::run(vault)
}
