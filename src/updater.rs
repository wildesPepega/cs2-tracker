//! Self-updater: checks GitHub releases on startup, periodically and on
//! shutdown, swaps the binary in place and re-executes itself (same PID, so
//! Pterodactyl/systemd don't notice).

use crate::Ctx;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tracing::{info, warn};

const REPO: &str = "wildesPepega/cs2-tracker";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Hosts like Pterodactyl kill the process if stopping takes too long.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub async fn run(ctx: Arc<Ctx>, exe: PathBuf) {
    if !ctx.config.auto_update {
        info!("Auto-update disabled");
        return std::future::pending().await;
    }
    // The startup check already ran, so wait a full interval first.
    loop {
        tokio::time::sleep(ctx.config.update_interval).await;
        match install_latest(&ctx.http, &exe).await {
            Ok(Some(version)) => {
                info!("Updated to v{version}, restarting");
                if let Err(e) = restart(&exe) {
                    warn!("{e:#}");
                }
            }
            Ok(None) => {}
            Err(e) => warn!("Update check failed: {e:#}"),
        }
    }
}

/// Runs before anything connects: installs a newer release and re-executes
/// into it right away.
pub async fn on_startup(http: &reqwest::Client, exe: &Path) {
    match tokio::time::timeout(STARTUP_TIMEOUT, install_latest(http, exe)).await {
        Ok(Ok(Some(version))) => {
            info!("Updated to v{version} on startup, restarting");
            if let Err(e) = restart(exe) {
                warn!("{e:#}");
            }
        }
        Ok(Ok(None)) => info!("Up to date (v{VERSION})"),
        Ok(Err(e)) => warn!("Startup update check failed: {e:#}"),
        Err(_) => warn!("Startup update check timed out"),
    }
}

/// Runs on shutdown: installs a newer release so the next start uses it.
pub async fn on_shutdown(http: &reqwest::Client, exe: &Path) {
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, install_latest(http, exe)).await {
        Ok(Ok(Some(version))) => info!("Installed v{version}, it will run on next start"),
        Ok(Ok(None)) => info!("Up to date (v{VERSION})"),
        Ok(Err(e)) => warn!("Shutdown update check failed: {e:#}"),
        Err(_) => warn!("Shutdown update check timed out"),
    }
}

/// Downloads and installs the latest release if it is newer than this
/// binary. Returns the installed version.
async fn install_latest(http: &reqwest::Client, exe: &Path) -> Result<Option<String>> {
    let release: Release = http
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let latest = release.tag_name.trim_start_matches('v');
    if !is_newer(latest, VERSION) {
        return Ok(None);
    }

    let asset_name = format!("cs2-tracker-linux-{}", std::env::consts::ARCH);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .with_context(|| format!("release {} has no asset {asset_name}", release.tag_name))?;
    info!("Updating v{VERSION} -> v{latest}");

    let bytes = http
        .get(&asset.browser_download_url)
        .timeout(Duration::from_secs(300))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let tmp = exe.with_file_name(".cs2-tracker.update");
    tokio::fs::write(&tmp, &bytes).await?;
    install(&tmp, exe, latest).await?;
    Ok(Some(latest.to_owned()))
}

/// Makes `tmp` executable, sanity-checks it and moves it over `exe`.
async fn install(tmp: &Path, exe: &Path, expected: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755)).await?;
    }
    let output = tokio::process::Command::new(tmp)
        .arg("--version")
        .output()
        .await
        .context("new binary does not run")?;
    let reported = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !reported.contains(expected) {
        let _ = tokio::fs::remove_file(tmp).await;
        bail!(
            "new binary reported {:?}, expected {expected}",
            reported.trim()
        );
    }
    // Renaming over a running executable is fine on Linux.
    tokio::fs::rename(tmp, exe).await?;
    Ok(())
}

#[cfg(unix)]
fn restart(exe: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    Err(err).context("re-exec failed")
}

#[cfg(not(unix))]
fn restart(_exe: &Path) -> Result<()> {
    bail!("in-place restart is only supported on Unix, restart manually")
}

/// Compares dotted versions numerically (`0.10.0` > `0.9.3`).
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parse(candidate) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.1", "0.3.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.0"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
    }
}
