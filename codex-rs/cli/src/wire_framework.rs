//! Wire: the framework carried inside the binary.
//!
//! The Wire specifications, command catalogue, authoring skills, lane agents,
//! Studio and the Wire CLI are packed at build time (build-agent.sh sets
//! WIRE_FRAMEWORK_ARCHIVE to a .tar.gz of the Codex plugin) and compiled in
//! with `include_bytes!`. `wire-agent framework path` unpacks them once per
//! version under `$CODEX_HOME/framework/<version>/` and prints that folder, so
//! an engagement repository needs no copy of the plugin: the Wire CLI registers
//! the folder as a local plugin marketplace in the agent's own home.
//! A build without the variable carries nothing and says so.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

static ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wire-framework.tar.gz"));
const VERSION: &str = env!("CARGO_PKG_VERSION");
const COMPLETE: &str = ".wire-framework-complete";

#[derive(Debug, Parser)]
pub struct FrameworkCommand {
    #[command(subcommand)]
    pub action: FrameworkAction,
}

#[derive(Debug, clap::Subcommand)]
pub enum FrameworkAction {
    /// Unpack the Wire framework this binary carries (once per version) and print its folder.
    Path,
    /// Print the version of the Wire framework this binary carries.
    Version,
}

pub fn run(cmd: FrameworkCommand) -> anyhow::Result<()> {
    if ARCHIVE.is_empty() {
        anyhow::bail!("this build carries no Wire framework (built without WIRE_FRAMEWORK_ARCHIVE)");
    }
    match cmd.action {
        FrameworkAction::Version => println!("{VERSION}"),
        FrameworkAction::Path => {
            let home = codex_core::config::find_codex_home().context("finding the agent's home")?;
            let dir = unpack(home.as_path())?;
            println!("{}", dir.display());
        }
    }
    Ok(())
}

/// `<home>/framework/<version>/`, unpacked if it is not complete. Unpacks into a
/// temporary folder and renames it into place, so a half-written folder is never used.
fn unpack(home: &Path) -> anyhow::Result<PathBuf> {
    let root = home.join("framework");
    let dir = root.join(VERSION);
    if dir.join(COMPLETE).is_file() {
        return Ok(dir);
    }
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let partial = root.join(format!(".{VERSION}.partial-{}", std::process::id()));
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(ARCHIVE));
    archive.set_preserve_permissions(true);
    archive
        .unpack(&partial)
        .with_context(|| format!("unpacking the Wire framework into {}", partial.display()))?;
    let mut marker = fs::File::create(partial.join(COMPLETE))?;
    writeln!(marker, "{VERSION}")?;
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::rename(&partial, &dir).with_context(|| format!("moving the framework into {}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_the_stamped_workspace_version() {
        assert!(!super::VERSION.is_empty());
    }
}
