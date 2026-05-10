//! `ft man` — render man pages from the clap definition.
//!
//! With no arguments, prints the top-level page (`ft.1`) to stdout. With
//! `--out DIR`, generates the top-level page plus one per subcommand into
//! that directory.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Command, CommandFactory};

#[derive(Args, Debug)]
pub struct ManArgs {
    /// Write all man pages (top-level + subcommands) to this directory
    /// instead of stdout.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

pub fn run(args: ManArgs) -> Result<()> {
    let cmd = crate::Cli::command();

    if let Some(dir) = args.out {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating output directory {}", dir.display()))?;
        write_all(&cmd, &dir)?;
        return Ok(());
    }

    let man = clap_mangen::Man::new(cmd);
    man.render(&mut std::io::stdout())
        .context("rendering top-level man page")?;
    Ok(())
}

fn write_all(cmd: &Command, dir: &std::path::Path) -> Result<()> {
    let bin = cmd.get_name();
    let top_path = dir.join(format!("{bin}.1"));
    let mut buf: Vec<u8> = Vec::new();
    clap_mangen::Man::new(cmd.clone())
        .render(&mut buf)
        .context("rendering top-level man page")?;
    std::fs::write(&top_path, buf).with_context(|| format!("writing {}", top_path.display()))?;

    for sub in cmd.get_subcommands() {
        let sub_name = sub.get_name();
        // Skip the meta-subcommands so we don't generate man pages for them.
        if sub_name == "completions" || sub_name == "man" || sub_name == "help" {
            continue;
        }
        let path = dir.join(format!("{bin}-{sub_name}.1"));
        let title = format!("{bin}-{sub_name}");
        let mut sub_buf: Vec<u8> = Vec::new();
        clap_mangen::Man::new(sub.clone())
            .title(title)
            .render(&mut sub_buf)
            .with_context(|| format!("rendering man page for {bin} {sub_name}"))?;
        std::fs::write(&path, sub_buf).with_context(|| format!("writing {}", path.display()))?;

        // Recurse into nested subcommands (e.g. `ft tasks list`).
        for nested in sub.get_subcommands() {
            let nested_name = nested.get_name();
            if nested_name == "help" {
                continue;
            }
            let path = dir.join(format!("{bin}-{sub_name}-{nested_name}.1"));
            let nested_title = format!("{bin}-{sub_name}-{nested_name}");
            let mut nested_buf: Vec<u8> = Vec::new();
            clap_mangen::Man::new(nested.clone())
                .title(nested_title)
                .render(&mut nested_buf)
                .with_context(|| {
                    format!("rendering man page for {bin} {sub_name} {nested_name}")
                })?;
            std::fs::write(&path, nested_buf)
                .with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(())
}
