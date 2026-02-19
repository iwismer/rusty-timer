// rt-updater: Self-update helpers for rusty-timer services.
//
// Checks GitHub Releases for new versions, downloads and verifies release
// archives, and stages updated binaries for atomic replacement.

use std::io::Write;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Status of an update check / download cycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UpdateStatus {
    UpToDate,
    Available { version: String },
    Downloaded { version: String },
    Failed { error: String },
}

// ---------------------------------------------------------------------------
// UpdateChecker
// ---------------------------------------------------------------------------

/// Checks for, downloads, and applies updates from GitHub Releases.
///
/// Releases are expected to be tagged per-service, e.g. `forwarder-v0.1.0`,
/// with assets named like `forwarder-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`.
pub struct UpdateChecker {
    repo_owner: String,
    repo_name: String,
    service_name: String,
    current_version: Version,
}

impl UpdateChecker {
    /// Create a new `UpdateChecker`.
    ///
    /// # Errors
    ///
    /// Returns an error if `current_version_str` is not valid semver.
    pub fn new(
        repo_owner: impl Into<String>,
        repo_name: impl Into<String>,
        service_name: impl Into<String>,
        current_version_str: &str,
    ) -> Result<Self, semver::Error> {
        let current_version = Version::parse(current_version_str)?;
        Ok(Self {
            repo_owner: repo_owner.into(),
            repo_name: repo_name.into(),
            service_name: service_name.into(),
            current_version,
        })
    }

    /// Check GitHub for a newer release of this service.
    ///
    /// # Errors
    ///
    /// Returns an error if the GitHub API call fails.
    pub async fn check(&self) -> Result<UpdateStatus, Box<dyn std::error::Error + Send + Sync>> {
        let owner = self.repo_owner.clone();
        let name = self.repo_name.clone();
        let service = self.service_name.clone();
        let current = self.current_version.clone();

        tokio::task::spawn_blocking(move || check_blocking(&owner, &name, &service, &current))
            .await?
    }

    /// Download the release matching `version`, verify its checksum, and stage
    /// the binary next to the running executable.
    ///
    /// Returns the path to the staged binary.
    ///
    /// # Errors
    ///
    /// Returns an error if the release cannot be found, downloaded, or verified.
    pub async fn download(
        &self,
        version: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        let owner = self.repo_owner.clone();
        let name = self.repo_name.clone();
        let service = self.service_name.clone();
        let version = version.to_owned();

        tokio::task::spawn_blocking(move || download_blocking(&owner, &name, &service, &version))
            .await?
    }

    /// Replace the running binary with the staged binary and exit the process.
    ///
    /// # Errors
    ///
    /// Returns an error if the replacement fails.
    pub fn apply_and_exit(
        staged_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(path = %staged_path.display(), "replacing running binary with staged update");
        self_replace::self_replace(staged_path)?;

        // Clean up staged file (best-effort).
        let _ = std::fs::remove_file(staged_path);

        info!("binary replaced successfully — exiting for restart");
        std::process::exit(0);
    }
}

// ---------------------------------------------------------------------------
// Helpers – tag parsing
// ---------------------------------------------------------------------------

/// Given a release tag like `forwarder-v0.1.0` and a service name like
/// `forwarder`, return the parsed semver `Version`.
fn parse_version_from_tag(tag: &str, service_name: &str) -> Option<Version> {
    let prefix = format!("{service_name}-v");
    let version_str = tag.strip_prefix(&prefix)?;
    Version::parse(version_str).ok()
}

fn stage_root_dir(exe_dir: &Path) -> PathBuf {
    stage_root_dir_from(std::env::var_os("RT_UPDATER_STAGE_DIR"), exe_dir)
}

fn stage_root_dir_from(env_value: Option<std::ffi::OsString>, exe_dir: &Path) -> PathBuf {
    match env_value {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => exe_dir.to_path_buf(),
    }
}

// ---------------------------------------------------------------------------
// Blocking implementations (run inside spawn_blocking)
// ---------------------------------------------------------------------------

fn check_blocking(
    repo_owner: &str,
    repo_name: &str,
    service_name: &str,
    current_version: &Version,
) -> Result<UpdateStatus, Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!(
        service = service_name,
        current = %current_version,
        "checking for updates"
    );

    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(repo_owner)
        .repo_name(repo_name)
        .build()?
        .fetch()?;

    // Find the latest release whose tag matches our service prefix.
    let mut best: Option<(Version, String)> = None;
    for release in &releases {
        if let Some(ver) = parse_version_from_tag(&release.version, service_name) {
            if best.as_ref().is_none_or(|(v, _)| ver > *v) {
                best = Some((ver, release.version.clone()));
            }
        }
    }

    match best {
        Some((ver, _tag)) if ver > *current_version => {
            info!(latest = %ver, current = %current_version, "update available");
            Ok(UpdateStatus::Available {
                version: ver.to_string(),
            })
        }
        _ => {
            tracing::debug!("already up to date");
            Ok(UpdateStatus::UpToDate)
        }
    }
}

fn download_blocking(
    repo_owner: &str,
    repo_name: &str,
    service_name: &str,
    version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let tag = format!("{service_name}-v{version}");
    let target = self_update::get_target();

    info!(tag = %tag, target = %target, "downloading release");

    // Fetch the release list and find the matching release.
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(repo_owner)
        .repo_name(repo_name)
        .build()?
        .fetch()?;

    let release = releases
        .iter()
        .find(|r| r.version == tag)
        .ok_or_else(|| format!("release not found for tag {tag}"))?;

    let asset = select_archive_asset(&release.assets, target)
        .ok_or_else(|| format!("no archive asset found for target {target} in release {tag}"))?;

    // Download and stage on the same filesystem as the final staged path.
    let current_exe = std::env::current_exe()?;
    let exe_dir = current_exe
        .parent()
        .ok_or("cannot determine executable directory")?;
    let stage_root = stage_root_dir(exe_dir);
    std::fs::create_dir_all(&stage_root)?;
    let tmp_dir = tempfile::tempdir_in(&stage_root)?;
    let tmp_archive = tmp_dir.path().join(&asset.name);

    {
        let mut out = std::fs::File::create(&tmp_archive)?;
        self_update::Download::from_url(&asset.download_url)
            .set_header(reqwest::header::ACCEPT, "application/octet-stream".parse()?)
            .download_to(&mut out)?;
        out.flush()?;
    }

    // SHA-256 verification (optional sidecar).
    verify_sha256(&release.assets, &asset.name, &tmp_archive)?;

    // Extract the binary from the archive.
    let extract_dir = tmp_dir.path().join("extracted");
    std::fs::create_dir_all(&extract_dir)?;
    self_update::Extract::from_source(&tmp_archive).extract_into(&extract_dir)?;

    // Find the extracted binary — it should be the service name (or the only file).
    let staged_bin = find_extracted_binary(&extract_dir, service_name)?;

    // Stage as `.{service_name}-staged` under the configured stage root.
    let staged_path = stage_root.join(format!(".{service_name}-staged"));
    std::fs::copy(&staged_bin, &staged_path)?;

    // Make executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged_path, std::fs::Permissions::from_mode(0o755))?;
    }

    info!(path = %staged_path.display(), "binary staged successfully");
    Ok(staged_path)
}

/// Download the `.sha256` sidecar and verify the archive's hash.
/// The sidecar is required.
fn verify_sha256(
    assets: &[self_update::update::ReleaseAsset],
    asset_name: &str,
    archive_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sha_asset_name = format!("{asset_name}.sha256");

    let sha_asset = assets
        .iter()
        .find(|a| a.name == sha_asset_name)
        .ok_or_else(|| format!("missing required sha256 sidecar asset: {sha_asset_name}"))?;

    // Download the sidecar.
    let mut sha_buf: Vec<u8> = Vec::new();
    self_update::Download::from_url(&sha_asset.download_url)
        .set_header(reqwest::header::ACCEPT, "application/octet-stream".parse()?)
        .download_to(&mut sha_buf)?;

    let sha_text = String::from_utf8(sha_buf)?;
    let expected_hash = sha_text
        .split_whitespace()
        .next()
        .ok_or("empty .sha256 sidecar file")?
        .to_lowercase();

    // Compute actual hash.
    let archive_bytes = std::fs::read(archive_path)?;
    let actual_hash = hex::encode(Sha256::digest(&archive_bytes));

    if actual_hash != expected_hash {
        return Err(format!("sha256 mismatch: expected {expected_hash}, got {actual_hash}").into());
    }

    info!("sha256 verification passed");
    Ok(())
}

fn select_archive_asset<'a>(
    assets: &'a [self_update::update::ReleaseAsset],
    target: &str,
) -> Option<&'a self_update::update::ReleaseAsset> {
    assets.iter().find(|asset| {
        let name = asset.name.as_str();
        name.contains(target) && is_supported_archive_name(name)
    })
}

fn is_supported_archive_name(name: &str) -> bool {
    if name.to_ascii_lowercase().ends_with(".tar.gz") {
        return true;
    }
    std::path::Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

/// Walk the extraction directory and find the service binary.
fn find_extracted_binary(
    extract_dir: &Path,
    service_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let exe_name = format!("{service_name}.exe");

    // First, look for a file whose name matches the service name exactly,
    // or `{service_name}.exe` on Windows.
    for entry in std::fs::read_dir(extract_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == service_name || name == exe_name {
                    return Ok(path);
                }
            }
        }
    }

    // Fallback: return the first (only) file.
    for entry in std::fs::read_dir(extract_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            return Ok(path);
        }
    }

    Err(format!("no binary found in extracted archive for {service_name}").into())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    #[test]
    fn parse_version_from_tag_strips_prefix() {
        let ver = parse_version_from_tag("forwarder-v0.2.1", "forwarder");
        assert_eq!(ver, Some(Version::new(0, 2, 1)));
    }

    #[test]
    fn ignores_other_service_tags() {
        let ver = parse_version_from_tag("receiver-v0.3.0", "forwarder");
        assert_eq!(ver, None);
    }

    #[test]
    fn ignores_malformed_tags() {
        assert_eq!(parse_version_from_tag("not-a-version", "forwarder"), None);
        assert_eq!(parse_version_from_tag("forwarder-vbad", "forwarder"), None);
        assert_eq!(parse_version_from_tag("", "forwarder"), None);
    }

    #[test]
    fn version_comparison_newer() {
        let v1 = Version::new(0, 2, 0);
        let v2 = Version::new(0, 1, 0);
        assert!(v1 > v2);
    }

    #[test]
    fn version_comparison_same() {
        let v1 = Version::new(0, 1, 0);
        let v2 = Version::new(0, 1, 0);
        assert_eq!(v1, v2);
    }

    #[test]
    fn version_comparison_older() {
        let v1 = Version::new(0, 1, 0);
        let v2 = Version::new(0, 2, 0);
        assert!(v1 < v2);
    }

    #[test]
    fn new_checker_parses_valid_version() {
        let checker = UpdateChecker::new("owner", "repo", "forwarder", "0.1.0");
        assert!(checker.is_ok());
        let checker = checker.unwrap();
        assert_eq!(checker.current_version, Version::new(0, 1, 0));
    }

    #[test]
    fn new_checker_rejects_invalid_version() {
        let checker = UpdateChecker::new("owner", "repo", "forwarder", "not.a.version");
        assert!(checker.is_err());
    }

    #[test]
    fn staging_dir_prefers_explicit_env() {
        let stage_dir = stage_root_dir_from(
            Some(OsString::from("/var/lib/rusty-timer")),
            Path::new("/usr/local/bin"),
        );
        assert_eq!(stage_dir, PathBuf::from("/var/lib/rusty-timer"));
    }

    #[test]
    fn staging_dir_falls_back_to_exe_dir_when_unset() {
        let exe_dir = Path::new("/usr/local/bin");
        let stage_dir = stage_root_dir_from(None, exe_dir);
        assert_eq!(stage_dir, exe_dir);
    }

    fn release_asset(name: &str) -> self_update::update::ReleaseAsset {
        self_update::update::ReleaseAsset {
            download_url: format!("https://example.invalid/{name}"),
            name: name.to_owned(),
        }
    }

    #[test]
    fn select_archive_asset_prefers_real_archive_over_sidecar() {
        let target = "x86_64-unknown-linux-gnu";
        let assets = vec![
            release_asset("forwarder-v1.2.3-x86_64-unknown-linux-gnu.tar.gz.sha256"),
            release_asset("forwarder-v1.2.3-x86_64-unknown-linux-gnu.tar.gz"),
        ];

        let selected = select_archive_asset(&assets, target).expect("archive selected");
        assert_eq!(
            selected.name,
            "forwarder-v1.2.3-x86_64-unknown-linux-gnu.tar.gz"
        );
    }

    #[test]
    fn select_archive_asset_supports_zip_assets() {
        let target = "x86_64-pc-windows-msvc";
        let assets = vec![
            release_asset("receiver-v1.2.3-x86_64-pc-windows-msvc.zip.sha256"),
            release_asset("receiver-v1.2.3-x86_64-pc-windows-msvc.ZIP"),
        ];

        let selected = select_archive_asset(&assets, target).expect("archive selected");
        assert_eq!(selected.name, "receiver-v1.2.3-x86_64-pc-windows-msvc.ZIP");
    }

    #[test]
    fn verify_sha256_errors_when_sidecar_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp
            .path()
            .join("forwarder-v1.2.3-x86_64-unknown-linux-gnu.tar.gz");
        fs::write(&archive, b"test-archive-content").expect("write archive");

        let error = verify_sha256(
            &[],
            "forwarder-v1.2.3-x86_64-unknown-linux-gnu.tar.gz",
            &archive,
        )
        .expect_err("missing sidecar should fail");
        assert!(
            error
                .to_string()
                .contains("missing required sha256 sidecar asset"),
            "unexpected error: {error}"
        );
    }
}
