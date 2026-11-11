//! Self-update from GitHub Releases.
//!
//! On start-up the app asks GitHub for the newest release (in the background). When it is
//! newer than this build, the UI offers "Update now": the matching release asset is downloaded,
//! the running executable is replaced in place, and the app restarts.
//!
//! Release requirements (the GitHub Actions workflow in `.github/workflows/release.yml`
//! produces exactly this):
//!   * tag `vX.Y.Z` matching `version` in Cargo.toml,
//!   * an asset whose name contains the target triple, e.g.
//!     `UtilBee-PDF-Studio-v0.2.0-x86_64-pc-windows-msvc.zip`,
//!   * `util_bee.exe` at the root of that zip.

use std::time::Duration;

use anyhow::{Context, Result};
use self_update::backends::github::Update;

pub const REPO_OWNER: &str = "smbhathiya";
pub const REPO_NAME: &str = "util_bee_pdf_studio";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BIN_NAME: &str = "util_bee";

/// Set this environment variable to skip the automatic check (useful while developing).
pub const DISABLE_ENV: &str = "UTILBEE_NO_UPDATE_CHECK";

#[derive(Clone, Debug)]
pub struct Available {
    pub version: String,
    pub notes_url: String,
}

pub fn releases_page() -> String {
    format!("https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/latest")
}

fn updater(timeout_secs: u64) -> Result<Update> {
    Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(CURRENT_VERSION)
        .timeout(Duration::from_secs(timeout_secs))
        .show_download_progress(false)
        .show_output(false)
        .no_confirm(true)
        .build()
        .context("Could not set up the updater")
}

/// Ask GitHub whether a newer release exists. `Ok(None)` = already up to date.
pub fn check() -> Result<Option<Available>> {
    let release = updater(10)?
        .is_update_available()
        .context("Could not reach GitHub to check for updates")?;
    Ok(release.map(|r| Available {
        version: r.version().trim_start_matches('v').to_string(),
        notes_url: r
            .release_notes_url()
            .map(str::to_string)
            .unwrap_or_else(releases_page),
    }))
}

/// Download the newest release and replace the running executable. Returns the new version.
pub fn install() -> Result<String> {
    let status = updater(120)?
        .update()
        .context("The update could not be installed")?;
    Ok(status.version().trim_start_matches('v').to_string())
}

/// Start the (now replaced) executable again; the caller should then close the window.
pub fn relaunch() -> Result<()> {
    let exe = std::env::current_exe().context("Could not find the app's location")?;
    std::process::Command::new(exe)
        .spawn()
        .context("Could not restart the app")?;
    Ok(())
}
