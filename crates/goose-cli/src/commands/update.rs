// On riscv64, update() bails early because no release artifacts are published,
// so the implementation below (and most of this module) is cfg-disabled and
// would otherwise trigger dead_code/unused warnings that fail clippy.
#![cfg_attr(
    target_arch = "riscv64",
    allow(dead_code, unused_imports, unused_variables)
)]

use anyhow::{bail, Context, Result};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    StatusCode,
};
use sha2::{Digest, Sha256};
use sigstore_verify::trust_root::{TrustedRoot, SIGSTORE_PRODUCTION_TRUSTED_ROOT};
use sigstore_verify::types::{Bundle, Sha256Hash};
use sigstore_verify::VerificationPolicy;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Asset name for this platform (compile-time).
fn asset_name() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "goose-aarch64-apple-darwin.tar.bz2"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "goose-x86_64-apple-darwin.tar.bz2"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    {
        "goose-x86_64-unknown-linux-gnu.tar.bz2"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        "goose-aarch64-unknown-linux-gnu.tar.bz2"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
    {
        "goose-x86_64-unknown-linux-musl.tar.bz2"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
    {
        "goose-aarch64-unknown-linux-musl.tar.bz2"
    }
    // RISC-V builds compile with this asset name, but update() rejects the
    // platform until release artifacts are published. See update() below.
    #[cfg(all(target_os = "linux", target_arch = "riscv64", target_env = "gnu"))]
    {
        "goose-riscv64gc-unknown-linux-gnu.tar.bz2"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64", feature = "cuda"))]
    {
        "goose-x86_64-pc-windows-msvc-cuda.zip"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64", not(feature = "cuda")))]
    {
        "goose-x86_64-pc-windows-msvc.zip"
    }
}

/// Binary name for this platform.
fn binary_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "goose.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "goose"
    }
}

// ---------------------------------------------------------------------------
// Sigstore / SLSA provenance verification
// ---------------------------------------------------------------------------

/// Compute the SHA-256 hex digest of a byte slice.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    goose::utils::bytes_to_hex(hasher.finalize())
}

#[derive(serde::Deserialize)]
struct AttestationResponse {
    attestations: Vec<AttestationEntry>,
}

#[derive(serde::Deserialize)]
struct AttestationEntry {
    #[serde(default)]
    bundle: Option<serde_json::Value>,
    #[serde(default)]
    bundle_url: Option<String>,
}

const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";

fn sanitized_token(token: Option<&str>) -> Option<&str> {
    token.map(str::trim).filter(|tok| !tok.is_empty())
}

fn authorization_header_value(token: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!("Bearer {token}")).ok()
}

fn github_token() -> Option<String> {
    env::var("GITHUB_TOKEN")
        .ok()
        .and_then(|tok| sanitized_token(Some(&tok)).map(str::to_owned))
        .or_else(|| {
            env::var("GH_TOKEN")
                .ok()
                .and_then(|tok| sanitized_token(Some(&tok)).map(str::to_owned))
        })
}

fn should_retry_attestations_without_token(status: StatusCode, token: Option<&str>) -> bool {
    sanitized_token(token)
        .and_then(authorization_header_value)
        .is_some()
        && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

async fn fetch_attestations(digest: &str, token: Option<&str>) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://api.github.com/repos/aaif-goose/goose/attestations/sha256:{digest}\
         ?per_page=30&predicate_type=https://slsa.dev/provenance/v1"
    );

    let client = reqwest::Client::new();
    let token = sanitized_token(token);
    let resp = fetch_attestations_response(&client, &url, token).await?;

    let resp = if should_retry_attestations_without_token(resp.status(), token) {
        fetch_attestations_response(&client, &url, None).await?
    } else {
        resp
    };

    if !resp.status().is_success() {
        bail!("GitHub attestation API returned HTTP {}", resp.status());
    }

    let body: AttestationResponse = resp
        .json()
        .await
        .context("Failed to parse attestation response")?;

    // GitHub no longer embeds bundles in attestation list responses; they are
    // served from blob storage via bundle_url instead.
    let mut bundles = Vec::with_capacity(body.attestations.len());
    for entry in body.attestations {
        match (entry.bundle, entry.bundle_url) {
            (Some(bundle), _) if !bundle.is_null() => bundles.push(bundle),
            (_, Some(url)) => bundles.push(fetch_bundle(&client, &url).await?),
            _ => bail!("Attestation has neither a bundle nor a bundle URL"),
        }
    }

    Ok(bundles)
}

// The bundle URL is pre-signed for blob storage, so no API credentials are sent.
async fn fetch_bundle(client: &reqwest::Client, url: &str) -> Result<serde_json::Value> {
    let resp = client
        .get(url)
        .header("User-Agent", "goose-cli")
        .send()
        .await
        .context("Failed to fetch attestation bundle")?;

    if !resp.status().is_success() {
        bail!(
            "Attestation bundle download returned HTTP {}",
            resp.status()
        );
    }

    let body = resp
        .bytes()
        .await
        .context("Failed to read attestation bundle")?;
    parse_bundle_bytes(&body)
}

fn parse_bundle_bytes(body: &[u8]) -> Result<serde_json::Value> {
    if let Ok(bundle) = serde_json::from_slice(body) {
        return Ok(bundle);
    }

    // Offloaded bundles are served as snappy-compressed JSON.
    let decompressed = snap::raw::Decoder::new()
        .decompress_vec(body)
        .context("Failed to decompress attestation bundle")?;
    serde_json::from_slice(&decompressed).context("Failed to parse attestation bundle")
}

async fn fetch_attestations_response(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> Result<reqwest::Response> {
    let mut req = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "goose-cli");

    if let Some(value) = token.and_then(authorization_header_value) {
        req = req.header(AUTHORIZATION, value);
    }

    req.send().await.context("Failed to fetch attestations")
}

// Verify a single attestation bundle against the artifact digest and workflow.
fn verify_bundle(
    bundle_json: &serde_json::Value,
    artifact_digest: Sha256Hash,
    policy: &VerificationPolicy,
    trusted_root: &TrustedRoot,
    workflow: &str,
) -> Result<()> {
    let bundle_str = serde_json::to_string(bundle_json)?;
    let bundle = Bundle::from_json(&bundle_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse bundle: {e}"))?;

    let result = sigstore_verify::verify(artifact_digest, &bundle, policy, trusted_root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let identity = result
        .identity
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No identity in certificate"))?;

    let expected = format!("/.github/workflows/{workflow}");
    if !identity.contains(&expected) {
        bail!("Workflow mismatch: expected {workflow}, got {identity}");
    }

    Ok(())
}

/// Returns `Ok(())` when the downloaded archive has verified provenance.
async fn verify_provenance(archive_data: &[u8], tag: &str) -> Result<()> {
    let digest = sha256_hex(archive_data);
    println!("Archive SHA-256: {digest}");

    let workflow = match tag {
        "canary" => "canary.yml",
        _ => "release.yml",
    };

    let token = github_token();

    println!("Verifying SLSA provenance via Sigstore...");

    let bundles = fetch_attestations(&digest, token.as_deref())
        .await
        .context(
            "Sigstore provenance check could not complete; refusing to install unverifiable update",
        )?;

    if bundles.is_empty() {
        bail!("No Sigstore attestation found for downloaded archive; refusing to install unverifiable update");
    }

    let trusted_root = TrustedRoot::from_json(SIGSTORE_PRODUCTION_TRUSTED_ROOT)
        .context("Failed to load Sigstore trusted root")?;
    let policy = VerificationPolicy::with_issuer(GITHUB_ACTIONS_ISSUER);
    let artifact_digest =
        Sha256Hash::from_hex(&digest).context("Failed to parse artifact digest")?;

    // One passing attestation is sufficient.
    let mut last_err = None;
    for bundle_json in &bundles {
        match verify_bundle(
            bundle_json,
            artifact_digest,
            &policy,
            &trusted_root,
            workflow,
        ) {
            Ok(()) => {
                println!("Sigstore provenance verification passed.");
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }

    Err(anyhow::anyhow!(
        "Sigstore verification failed: {}\n\nAborting update due to security check failure.",
        last_err.unwrap()
    ))
}

/// Update the goose binary to the latest release.
///
/// Downloads the platform-appropriate archive from GitHub releases, verifies
/// its SLSA provenance via Sigstore, extracts it with path-traversal
/// hardening, and replaces the current binary in-place.
pub async fn update(canary: bool, reconfigure: bool) -> Result<()> {
    #[cfg(feature = "disable-update")]
    {
        bail!("Update is disabled in this build.");
    }

    // RISC-V release artifacts are not published yet, so reject self-update
    // rather than downloading a nonexistent asset.
    #[cfg(all(target_arch = "riscv64", not(feature = "disable-update")))]
    {
        bail!("Self-update is not supported on riscv64: no release artifacts are published for this platform.");
    }

    #[cfg(all(not(target_arch = "riscv64"), not(feature = "disable-update")))]
    {
        let tag = if canary { "canary" } else { "stable" };
        let asset = asset_name();
        let url = format!("https://github.com/aaif-goose/goose/releases/download/{tag}/{asset}");

        println!("Downloading {asset} from {tag} release...");

        // --- Download -----------------------------------------------------------
        let response = reqwest::get(&url)
            .await
            .context("Failed to download release archive")?;

        if !response.status().is_success() {
            bail!(
                "Download failed with HTTP status {}. URL: {}",
                response.status(),
                url
            );
        }

        let bytes = response
            .bytes()
            .await
            .context("Failed to read response body")?;

        println!("Downloaded {} bytes.", bytes.len());

        // --- Verify SLSA provenance via Sigstore --------------------------------
        verify_provenance(&bytes, tag).await?;

        // --- Extract to temp dir (hardened against path traversal) --------------
        let tmp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

        #[cfg(target_os = "windows")]
        extract_zip(&bytes, tmp_dir.path())?;

        #[cfg(not(target_os = "windows"))]
        extract_tar_bz2(&bytes, tmp_dir.path())?;

        // --- Locate the binary in the extracted archive -------------------------
        let binary = binary_name();
        let extracted_binary = find_binary(tmp_dir.path(), binary)
            .with_context(|| format!("Could not find {binary} in extracted archive"))?;

        // --- Replace the current binary -----------------------------------------
        let current_exe =
            env::current_exe().context("Failed to determine current executable path")?;

        replace_binary(&extracted_binary, &current_exe)
            .context("Failed to replace current binary")?;

        // --- Copy DLLs on Windows -----------------------------------------------
        #[cfg(target_os = "windows")]
        copy_dlls(&extracted_binary, &current_exe)?;

        println!("goose updated successfully (verified with Sigstore SLSA provenance).");

        // --- Reconfigure if requested -------------------------------------------
        if reconfigure {
            println!("Running goose configure...");
            let status = Command::new(current_exe)
                .arg("configure")
                .status()
                .context("Failed to run goose configure")?;
            if !status.success() {
                eprintln!("Warning: goose configure exited with {status}");
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------

/// Extract a .zip archive with path-traversal hardening (Windows).
///
/// Iterates entries individually and uses `enclosed_name()` to reject any
/// path that escapes the destination directory (zip-slip protection).
#[cfg(target_os = "windows")]
fn extract_zip(data: &[u8], dest: &Path) -> Result<()> {
    use std::io::Cursor;
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).context("Failed to open zip archive")?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("Failed to read zip entry at index {i}"))?;

        let safe_path = match entry.enclosed_name() {
            Some(p) => p.to_owned(),
            None => bail!("Zip entry has unsafe path: {}", entry.name()),
        };

        let target = dest.join(&safe_path);

        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }

    Ok(())
}

/// Validate that an archive entry path is safe (no absolute paths, no `..`).
fn validate_entry_path(path: &Path) -> Result<()> {
    if path.is_absolute() {
        bail!("Tar entry has absolute path: {}", path.display());
    }
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            bail!("Tar entry contains path traversal: {}", path.display());
        }
    }
    Ok(())
}

/// Extract a .tar.bz2 archive with path-traversal hardening (macOS / Linux).
///
/// Iterates entries individually, rejecting any entry whose path is absolute
/// or contains `..` components (tar-slip protection).
#[cfg(not(target_os = "windows"))]
fn extract_tar_bz2(data: &[u8], dest: &Path) -> Result<()> {
    use bzip2::read::BzDecoder;
    let decoder = BzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().context("Failed to read tar entries")? {
        let mut entry = entry.context("Failed to read tar entry")?;
        let path = entry
            .path()
            .context("Failed to read entry path")?
            .into_owned();

        validate_entry_path(&path)?;

        // Block symlinks and hardlinks whose targets escape the destination directory.
        // Use entry.link_name() (not entry.header().link_name()) so GNU/PAX extended
        // metadata (linkpath) is resolved; the header field alone may be truncated.
        let link_target_opt = entry
            .link_name()
            .context("Failed to read link name from tar entry")?;
        if let Some(link_target) = link_target_opt {
            validate_entry_path(&link_target)?;
        }

        let target = dest.join(&path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        entry
            .unpack(&target)
            .with_context(|| format!("Failed to extract: {}", path.display()))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Binary location
// ---------------------------------------------------------------------------

/// Find the binary inside the extracted archive.
///
/// The archive may place it in:
///   1. A `goose-package/` subdirectory (Windows releases)
///   2. Directly at the top level
///   3. In some other single subdirectory
fn find_binary(extract_dir: &Path, binary_name: &str) -> Option<PathBuf> {
    // 1. Check goose-package subdir (matches download_cli.sh / download_cli.ps1)
    let package_dir = extract_dir.join("goose-package");
    if package_dir.is_dir() {
        let p = package_dir.join(binary_name);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. Check top level
    let p = extract_dir.join(binary_name);
    if p.exists() {
        return Some(p);
    }

    // 3. Search one level of subdirectories
    if let Ok(entries) = fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let candidate = entry.path().join(binary_name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Binary replacement
// ---------------------------------------------------------------------------

/// Replace the current binary with the newly downloaded one.
///
/// On Windows we must rename the running exe (Windows allows rename but not
/// delete/overwrite of a locked file) then copy the new file in.
///
/// On Unix we can simply copy over the existing binary.
fn replace_binary(new_binary: &Path, current_exe: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let old_exe = current_exe.with_extension("exe.old");

        // Clean up leftover from a previous update
        if old_exe.exists() {
            fs::remove_file(&old_exe).with_context(|| {
                format!(
                    "Failed to remove old backup {}. Is another goose process running?",
                    old_exe.display()
                )
            })?;
        }

        // Rename the running binary out of the way
        fs::rename(current_exe, &old_exe).with_context(|| {
            format!(
                "Failed to rename running binary to {}. Try closing Goose Desktop if it's open.",
                old_exe.display()
            )
        })?;

        // Copy the new binary into place
        fs::copy(new_binary, current_exe).with_context(|| {
            // Try to restore the old binary
            let _ = fs::rename(&old_exe, current_exe);
            format!("Failed to copy new binary to {}", current_exe.display())
        })?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        let old_exe = current_exe.with_extension("old");

        // Rename current binary to avoid ETXTBSY on Linux
        if current_exe.exists() {
            fs::rename(current_exe, &old_exe).with_context(|| {
                format!("Failed to rename {} before update", current_exe.display())
            })?;
        }

        if let Err(e) = fs::copy(new_binary, current_exe) {
            // Restore old binary if copy fails
            let _ = fs::rename(&old_exe, current_exe);
            return Err(e).with_context(|| {
                format!("Failed to copy new binary to {}", current_exe.display())
            });
        }

        // Delete the old backup binary
        let _ = fs::remove_file(&old_exe);

        // Ensure the binary is executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(current_exe)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(current_exe, perms)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// DLL handling (Windows only)
// ---------------------------------------------------------------------------

/// Copy any .dll files from the extracted archive alongside the installed binary.
#[cfg(target_os = "windows")]
fn copy_dlls(extracted_binary: &Path, current_exe: &Path) -> Result<()> {
    let source_dir = extracted_binary
        .parent()
        .context("Extracted binary has no parent directory")?;
    let dest_dir = current_exe
        .parent()
        .context("Current executable has no parent directory")?;

    if let Ok(entries) = fs::read_dir(source_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("dll") {
                    let file_name = path.file_name().unwrap();
                    let dest = dest_dir.join(file_name);
                    // Remove existing DLL first (it may be locked by another process)
                    if dest.exists() {
                        let _ = fs::remove_file(&dest);
                    }
                    fs::copy(&path, &dest).with_context(|| {
                        format!("Failed to copy {} to {}", path.display(), dest.display())
                    })?;
                    println!("  Copied {}", file_name.to_string_lossy());
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_asset_name_valid() {
        let name = asset_name();
        assert!(!name.is_empty());
        assert!(name.starts_with("goose-"));
        #[cfg(target_os = "windows")]
        assert!(name.ends_with(".zip"));
        #[cfg(not(target_os = "windows"))]
        assert!(name.ends_with(".tar.bz2"));
    }

    #[test]
    fn test_binary_name() {
        let name = binary_name();
        #[cfg(target_os = "windows")]
        assert_eq!(name, "goose.exe");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(name, "goose");
    }

    #[test]
    fn test_find_binary_in_package_subdir() {
        let tmp = tempdir().unwrap();
        let pkg = tmp.path().join("goose-package");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join(binary_name()), b"fake").unwrap();

        let found = find_binary(tmp.path(), binary_name());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with(binary_name()));
    }

    #[test]
    fn test_find_binary_top_level() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join(binary_name()), b"fake").unwrap();

        let found = find_binary(tmp.path(), binary_name());
        assert!(found.is_some());
        assert_eq!(found.unwrap(), tmp.path().join(binary_name()));
    }

    #[test]
    fn test_find_binary_nested_subdir() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("some-dir");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(binary_name()), b"fake").unwrap();

        let found = find_binary(tmp.path(), binary_name());
        assert!(found.is_some());
    }

    #[test]
    fn test_find_binary_not_found() {
        let tmp = tempdir().unwrap();
        let found = find_binary(tmp.path(), binary_name());
        assert!(found.is_none());
    }

    #[test]
    fn test_replace_binary_basic() {
        let tmp = tempdir().unwrap();
        let new_bin = tmp.path().join("new_goose");
        let current = tmp.path().join("current_goose");

        fs::write(&new_bin, b"new version").unwrap();
        fs::write(&current, b"old version").unwrap();

        replace_binary(&new_bin, &current).unwrap();

        let content = fs::read_to_string(&current).unwrap();
        assert_eq!(content, "new version");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_replace_binary_windows_rename_away() {
        let tmp = tempdir().unwrap();
        let current = tmp.path().join("goose.exe");
        let new_bin = tmp.path().join("new_goose.exe");

        fs::write(&current, b"old version").unwrap();
        fs::write(&new_bin, b"new version").unwrap();

        replace_binary(&new_bin, &current).unwrap();

        // Current should now have new content
        let content = fs::read_to_string(&current).unwrap();
        assert_eq!(content, "new version");

        // Old backup should exist
        let old = current.with_extension("exe.old");
        assert!(old.exists());
        let old_content = fs::read_to_string(&old).unwrap();
        assert_eq!(old_content, "old version");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_replace_binary_windows_cleanup_old() {
        let tmp = tempdir().unwrap();
        let current = tmp.path().join("goose.exe");
        let old = current.with_extension("exe.old");
        let new_bin = tmp.path().join("new_goose.exe");

        // Simulate a previous update left .old behind
        fs::write(&current, b"version 2").unwrap();
        fs::write(&old, b"version 1").unwrap();
        fs::write(&new_bin, b"version 3").unwrap();

        replace_binary(&new_bin, &current).unwrap();

        let content = fs::read_to_string(&current).unwrap();
        assert_eq!(content, "version 3");

        // Old should now contain version 2 (not version 1)
        let old_content = fs::read_to_string(&old).unwrap();
        assert_eq!(old_content, "version 2");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_extract_zip_with_package_dir() {
        use std::io::Cursor;
        use std::io::Write;

        let tmp = tempdir().unwrap();

        // Create a zip in memory with goose-package/ structure
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            writer.add_directory("goose-package/", options).unwrap();
            writer
                .start_file("goose-package/goose.exe", options)
                .unwrap();
            writer.write_all(b"fake goose binary").unwrap();
            writer
                .start_file("goose-package/libtest.dll", options)
                .unwrap();
            writer.write_all(b"fake dll").unwrap();
            writer.finish().unwrap();
        }

        extract_zip(&buf, tmp.path()).unwrap();

        let binary = find_binary(tmp.path(), "goose.exe");
        assert!(binary.is_some());

        let content = fs::read_to_string(binary.unwrap()).unwrap();
        assert_eq!(content, "fake goose binary");

        // DLL should be in goose-package too
        assert!(tmp.path().join("goose-package/libtest.dll").exists());
    }

    // -----------------------------------------------------------------------
    // SHA-256 digest tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sha256_hex_known_value() {
        let digest = sha256_hex(b"hello world");
        assert_eq!(
            digest,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_sha256_hex_empty() {
        let digest = sha256_hex(b"");
        assert_eq!(
            digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sanitized_token_trims_blank_values() {
        assert_eq!(sanitized_token(None), None);
        assert_eq!(sanitized_token(Some("")), None);
        assert_eq!(sanitized_token(Some("   ")), None);
        assert_eq!(sanitized_token(Some(" token\n")), Some("token"));
    }

    #[test]
    fn test_authorization_header_value_rejects_malformed_tokens() {
        assert!(authorization_header_value("token").is_some());
        assert!(authorization_header_value("bad\ntoken").is_none());
    }

    #[test]
    fn test_attestation_lookup_retries_auth_failures_without_token() {
        assert!(should_retry_attestations_without_token(
            StatusCode::UNAUTHORIZED,
            Some("token")
        ));
        assert!(should_retry_attestations_without_token(
            StatusCode::FORBIDDEN,
            Some("token")
        ));
        assert!(!should_retry_attestations_without_token(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some("token")
        ));
        assert!(!should_retry_attestations_without_token(
            StatusCode::UNAUTHORIZED,
            Some("")
        ));
        assert!(!should_retry_attestations_without_token(
            StatusCode::UNAUTHORIZED,
            Some("bad\ntoken")
        ));
        assert!(!should_retry_attestations_without_token(
            StatusCode::UNAUTHORIZED,
            None
        ));
    }

    #[test]
    fn test_attestation_entry_parses_embedded_bundle() {
        let response: AttestationResponse = serde_json::from_str(
            r#"{"attestations":[{"repository_id":1,"bundle":{"mediaType":"application/vnd.dev.sigstore.bundle.v0.3+json"}}]}"#,
        )
        .unwrap();
        assert!(response.attestations[0].bundle.is_some());
        assert!(response.attestations[0].bundle_url.is_none());
    }

    #[test]
    fn test_attestation_entry_parses_offloaded_bundle() {
        let response: AttestationResponse = serde_json::from_str(
            r#"{"attestations":[{"repository_id":1,"bundle_url":"https://example.com/bundle.json.sn","initiator":"user","bundle":null}]}"#,
        )
        .unwrap();
        assert!(response.attestations[0].bundle.is_none());
        assert_eq!(
            response.attestations[0].bundle_url.as_deref(),
            Some("https://example.com/bundle.json.sn")
        );
    }

    #[test]
    fn test_parse_bundle_bytes_plain_json() {
        let bundle =
            parse_bundle_bytes(br#"{"mediaType":"application/vnd.dev.sigstore.bundle.v0.3+json"}"#)
                .unwrap();
        assert_eq!(
            bundle["mediaType"],
            "application/vnd.dev.sigstore.bundle.v0.3+json"
        );
    }

    #[test]
    fn test_parse_bundle_bytes_snappy_compressed() {
        let json = br#"{"mediaType":"application/vnd.dev.sigstore.bundle.v0.3+json"}"#;
        let compressed = snap::raw::Encoder::new().compress_vec(json).unwrap();
        let bundle = parse_bundle_bytes(&compressed).unwrap();
        assert_eq!(
            bundle["mediaType"],
            "application/vnd.dev.sigstore.bundle.v0.3+json"
        );
    }

    #[test]
    fn test_parse_bundle_bytes_rejects_garbage() {
        assert!(parse_bundle_bytes(&[0xff, 0x00, 0x12]).is_err());
    }

    // -----------------------------------------------------------------------
    // Path validation and extraction hardening tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_entry_path_accepts_safe_paths() {
        assert!(validate_entry_path(Path::new("goose")).is_ok());
        assert!(validate_entry_path(Path::new("goose-package/goose")).is_ok());
        assert!(validate_entry_path(Path::new("subdir/nested/file.txt")).is_ok());
    }

    #[test]
    fn test_validate_entry_path_rejects_absolute() {
        let result = validate_entry_path(Path::new("/etc/malicious"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("absolute path"));
    }

    #[test]
    fn test_validate_entry_path_rejects_traversal() {
        let result = validate_entry_path(Path::new("../../escape.txt"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn test_validate_entry_path_rejects_nested_traversal() {
        let result = validate_entry_path(Path::new("safe/../../escape"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_extract_tar_bz2_safe_archive() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;

        let tmp = tempdir().unwrap();

        let mut builder_buf = Vec::new();
        {
            let encoder = BzEncoder::new(&mut builder_buf, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let data = b"goose binary content";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "goose-package/goose", &data[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }

        extract_tar_bz2(&builder_buf, tmp.path()).unwrap();

        let extracted = tmp.path().join("goose-package/goose");
        assert!(extracted.exists());
        assert_eq!(
            fs::read_to_string(extracted).unwrap(),
            "goose binary content"
        );
    }

    // -----------------------------------------------------------------------
    // Sigstore provenance verification test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_verify_provenance_fails_closed_when_unverifiable() {
        let result = verify_provenance(b"not a real archive", "stable").await;
        assert!(
            result.is_err(),
            "verify_provenance must fail closed when provenance cannot be verified"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_extract_tar_bz2_blocks_symlink_escape() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;

        let tmp = tempdir().unwrap();

        let mut builder_buf = Vec::new();
        {
            let encoder = BzEncoder::new(&mut builder_buf, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            // Symlink whose target escapes the destination directory.
            builder
                .append_link(&mut header, "evil_link", "../../etc/passwd")
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }

        let result = extract_tar_bz2(&builder_buf, tmp.path());
        assert!(
            result.is_err(),
            "extraction should fail when a symlink target escapes the destination"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("path traversal"),
            "error should mention path traversal, got: {err_msg}"
        );
    }
}
