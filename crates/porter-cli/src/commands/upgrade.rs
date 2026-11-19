//! `porter upgrade`: Homebrew installs go through `brew`, anything else self-replaces from
//! the umi-labs/porter GitHub release for this target triple.

use crate::output::print_json;
use anyhow::{Result, anyhow, bail};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum InstallKind {
    Brew,
    Binary,
}

#[derive(Debug, PartialEq, Eq)]
enum Gate {
    Go,
    Ask,
    Refuse,
}

/// Homebrew keeps binaries in a `Cellar` and symlinks them from its prefix.
pub fn install_kind(exe: &Path) -> InstallKind {
    let in_cellar = exe.components().any(|c| c.as_os_str() == "Cellar");
    let in_prefix = exe.starts_with("/opt/homebrew") || exe.starts_with("/home/linuxbrew");
    if in_cellar || in_prefix {
        InstallKind::Brew
    } else {
        InstallKind::Binary
    }
}

/// Replacing the binary needs `--yes`, or `--interactive` with a TTY to ask on.
fn gate(yes: bool, interactive: bool, tty: bool) -> Gate {
    if yes {
        Gate::Go
    } else if interactive && tty {
        Gate::Ask
    } else {
        Gate::Refuse
    }
}

const REPO: (&str, &str) = ("umi-labs", "porter");
// Formula names to try, tap-qualified first (as 1.x did).
const FORMULAE: [&str; 2] = ["umi-labs/tap/porter", "porter"];

fn brew(args: &[&str]) -> Result<std::process::Output> {
    Command::new("brew").args(args).output().map_err(|e| {
        anyhow!(
            "could not run brew {}: {e}. Is Homebrew installed?",
            args[0]
        )
    })
}

fn latest_release() -> Result<self_update::update::Release> {
    let releases = self_update::backends::github::Update::configure()
        .repo_owner(REPO.0)
        .repo_name(REPO.1)
        .bin_name("porter")
        .current_version(env!("CARGO_PKG_VERSION"))
        .build()?
        .get_latest_release()?;
    releases
        .latest()
        .cloned()
        .ok_or_else(|| anyhow!("no releases found"))
}

/// (name, download url)
type Asset = (String, String);

/// The release archive for `target` (name ends with `ext`, contains the triple) and its
/// `<archive>.sha256`. Never matches the checksum file as the archive.
fn select_assets(assets: &[Asset], target: &str, ext: &str) -> Result<(Asset, Asset)> {
    let archive = assets
        .iter()
        .find(|(n, _)| n.ends_with(ext) && n.contains(target))
        .ok_or_else(|| anyhow!("no {ext} release asset for {target}"))?;
    let want = format!("{}.sha256", archive.0);
    let sha = assets
        .iter()
        .find(|(n, _)| *n == want)
        .ok_or_else(|| anyhow!("release has no {want} to verify the download against"))?;
    Ok((archive.clone(), sha.clone()))
}

/// `sha_text` is `<hex>` or `<hex>  <filename>`, as `sha256sum` writes it.
fn verify_sha256(bytes: &[u8], sha_text: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let want = sha_text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    let got = format!("{:x}", Sha256::digest(bytes));
    if want != got {
        bail!("checksum mismatch (expected {want}, got {got}); binary left untouched");
    }
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    self_update::Download::from_url(url)
        .request_header("Accept", "application/octet-stream")
        .request_header("User-Agent", "porter-upgrade")
        .download_to(&mut buf)?;
    Ok(buf)
}

fn confirm() -> Result<bool> {
    eprint!("replace the installed porter? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn brew_upgrade(force: bool) -> Result<bool> {
    eprintln!("updating Homebrew...");
    let o = brew(&["update"])?;
    if !o.status.success() {
        bail!("brew update failed: {}", String::from_utf8_lossy(&o.stderr));
    }
    let outdated = FORMULAE.iter().any(|f| {
        brew(&["outdated", f])
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    });
    if !outdated && !force {
        eprintln!("porter is already up to date (use --force to reinstall)");
        return Ok(false);
    }
    for f in FORMULAE {
        let o = brew(&["upgrade", f])?;
        if o.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&o.stdout).trim());
            return Ok(true);
        }
    }
    bail!("brew upgrade porter failed; try `brew upgrade umi-labs/tap/porter` yourself")
}

fn binary_upgrade(release: &self_update::update::Release) -> Result<bool> {
    let (ext, bin) = if cfg!(windows) {
        (".zip", "porter.exe")
    } else {
        (".tar.xz", "porter")
    };
    let target = self_update::get_target();
    let assets: Vec<Asset> = release
        .assets()
        .iter()
        .map(|a| (a.name().to_string(), a.download_url().to_string()))
        .collect();
    let (archive, sha) = select_assets(&assets, target, ext)?;
    let bytes = download(&archive.1)?;
    verify_sha256(&bytes, &String::from_utf8_lossy(&download(&sha.1)?))?;
    let tmp = tempfile::tempdir()?;
    let file = tmp.path().join(&archive.0);
    std::fs::write(&file, &bytes)?;
    // dist archives hold one folder named after the archive, minus the extension.
    let inner = format!("{}/{bin}", archive.0.trim_end_matches(ext));
    self_update::Extract::from_source(&file).extract_file(tmp.path(), &inner)?;
    self_replace::self_replace(tmp.path().join(inner))?;
    Ok(true)
}

pub async fn run(
    json: bool,
    interactive: bool,
    check: bool,
    yes: bool,
    force: bool,
) -> Result<i32> {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| anyhow!("cannot locate the porter binary: {e}"))?;
    let kind = install_kind(&exe);
    let current = env!("CARGO_PKG_VERSION");
    let release = tokio::task::spawn_blocking(latest_release).await??;
    let latest = release.version().trim_start_matches('v').to_string();
    let newer = self_update::version::bump_is_greater(current, &latest).unwrap_or(false);

    let outcome = |upgraded: bool| {
        if json {
            print_json(&serde_json::json!({
                "v": 1, "event": "upgrade", "from": current, "to": latest, "upgraded": upgraded,
            }));
        }
    };
    if json {
        print_json(&serde_json::json!({
            "v": 1, "event": "upgrade-check", "current": current, "latest": latest,
            "update_available": newer, "install": format!("{kind:?}").to_lowercase(),
        }));
    }
    eprintln!("porter {current}, latest {latest}");
    if check {
        return Ok(0);
    }
    if !newer && !force {
        eprintln!("already up to date");
        outcome(false);
        return Ok(0);
    }
    match gate(yes, interactive, std::io::stdin().is_terminal()) {
        Gate::Go => {}
        Gate::Ask if confirm()? => {}
        Gate::Ask => {
            eprintln!("not upgrading");
            outcome(false);
            return Ok(0);
        }
        Gate::Refuse => {
            bail!("refusing to replace porter without --yes (or --interactive on a terminal)")
        }
    }
    let upgraded = tokio::task::spawn_blocking(move || match kind {
        InstallKind::Brew => brew_upgrade(force),
        InstallKind::Binary => binary_upgrade(&release),
    })
    .await??;
    outcome(upgraded);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_detects_brew_prefix() {
        for p in [
            "/opt/homebrew/Cellar/porter/2.0.0/bin/porter",
            "/usr/local/Cellar/porter/2.0.0/bin/porter",
            "/home/linuxbrew/.linuxbrew/Cellar/porter/2.0.0/bin/porter",
            "/opt/homebrew/bin/porter",
        ] {
            assert_eq!(install_kind(Path::new(p)), InstallKind::Brew, "{p}");
        }
        for p in ["/usr/local/bin/porter", "/home/me/.cargo/bin/porter"] {
            assert_eq!(install_kind(Path::new(p)), InstallKind::Binary, "{p}");
        }
    }

    #[test]
    fn replacing_needs_yes_or_a_tty_prompt() {
        assert_eq!(gate(true, false, false), Gate::Go);
        assert_eq!(gate(false, false, true), Gate::Refuse);
        assert_eq!(gate(false, true, false), Gate::Refuse);
        assert_eq!(gate(false, true, true), Gate::Ask);
    }

    fn a(n: &str) -> Asset {
        (n.to_string(), format!("https://x/{n}"))
    }

    #[test]
    fn selects_archive_not_its_checksum() {
        let t = "x86_64-unknown-linux-gnu";
        // Checksum listed first: must not be picked as the archive.
        let assets = [
            a(&format!("porter-cli-{t}.tar.xz.sha256")),
            a("porter-cli-aarch64-apple-darwin.tar.xz"),
            a(&format!("porter-cli-{t}.tar.xz")),
            a("porter-cli.rb"),
        ];
        let (arc, sha) = select_assets(&assets, t, ".tar.xz").unwrap();
        assert_eq!(arc.0, format!("porter-cli-{t}.tar.xz"));
        assert_eq!(sha.0, format!("porter-cli-{t}.tar.xz.sha256"));
        assert!(select_assets(&assets, "riscv64-unknown-linux-gnu", ".tar.xz").is_err());
        assert!(select_assets(&assets[1..], t, ".tar.xz").is_err());
        let no_sha = [a(&format!("porter-cli-{t}.tar.xz"))];
        assert!(select_assets(&no_sha, t, ".tar.xz").is_err());
    }

    #[test]
    fn checksum_verification() {
        // sha256("abc")
        let h = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256(b"abc", h).is_ok());
        assert!(verify_sha256(b"abc", &format!("{}  porter.tar.xz\n", h.to_uppercase())).is_ok());
        assert!(verify_sha256(b"abd", h).is_err());
        assert!(verify_sha256(b"abc", "").is_err());
    }
}
