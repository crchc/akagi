//! Download a release zip (direct or via mirrors), verify integrity,
//! extract the binary and Web UI, replace them in place, then restart.
//!
//! Integrity policy: the SHA-256 digest from release metadata is checked
//! when present, but the *trust anchor* is the minisign signature —
//! metadata and bytes may both have come through an untrusted
//! accelerator mirror. When any part of the flow touched a mirror, a
//! valid signature is mandatory; a fully-direct download of an old
//! unsigned release keeps the historical digest-only behaviour.
//!
//! The bundled Python+UV runtime tree stays untouched. That's fine for
//! normal patch releases; runtime-bump releases may require a full reinstall.

use crate::config::NetworkConfig;
use crate::github::mirror::{self, Source};
use crate::github::{
    build_client, download_with_fallback, extract_zip_safe, fetch_text_with_fallback, signing,
};
use crate::updater::check::UpdateInfo;
use crate::updater::error::UpdateError;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use tracing::{info as log_info, warn};

/// Download and apply an update. On success, starts the new binary and exits.
pub async fn download_and_apply(info: &UpdateInfo, net: &NetworkConfig) -> Result<(), UpdateError> {
    // Determine the running exe path and verify its parent is writable
    // before we burn bandwidth on a download we can't apply.
    let current_exe = std::env::current_exe()
        .context("std::env::current_exe failed")
        .map_err(UpdateError::from)?;
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("current_exe has no parent"))
        .map_err(UpdateError::from)?
        .to_path_buf();
    if !is_dir_writable(&install_dir) {
        return Err(UpdateError::ReadOnlyInstall { path: install_dir });
    }

    // Stream-download the asset into a tempfile that lives next to the
    // exe so the rename in `self_replace::self_replace` stays on the
    // same filesystem.
    let tempdir = tempfile::Builder::new()
        .prefix(".akagi-update-")
        .tempdir_in(&install_dir)
        .context("create staging dir next to current exe")
        .map_err(UpdateError::from)?;
    let zip_path = tempdir.path().join("release.zip");

    let client = build_client().map_err(UpdateError::from)?;
    let zip_candidates = mirror::candidates(net, &info.asset_url);
    let (zip_source, downloaded_digest) =
        download_with_fallback(&client, &zip_candidates, &zip_path)
            .await
            .map_err(UpdateError::from)?;
    log_info!(
        "update zip downloaded via {zip_source:?} ({} bytes expected)",
        info.asset_size
    );

    if let Some(expected) = info.asset_digest_sha256.as_deref() {
        if !downloaded_digest.eq_ignore_ascii_case(expected) {
            return Err(UpdateError::DigestMismatch);
        }
    }

    // Signature policy. `mirror_involved` is the union of every place
    // untrusted bytes could have entered: the metadata fetch (digest and
    // asset URL fields) and the zip download itself.
    let mirror_involved = info.meta_source == Source::Mirror || zip_source == Source::Mirror;
    match info.sig_url.as_deref() {
        Some(sig_url) => {
            let sig_candidates = mirror::candidates(net, sig_url);
            let (sig_text, sig_source) = fetch_text_with_fallback(&client, &sig_candidates)
                .await
                .context("fetch release signature")
                .map_err(UpdateError::from)?;
            log_info!("release signature fetched via {sig_source:?}");
            // The expected trusted comment is computed from the version
            // the release CLAIMS to be and our own platform triple —
            // never from the metadata's asset name. Forged metadata that
            // pairs a fake newer tag with a genuine (validly signed)
            // older zip then fails right here: no signature exists whose
            // trusted comment names the fake version. This ties the
            // signature to `scripts/package-zip.sh`'s naming; if a
            // release's tag and Cargo version ever disagree, the update
            // fails closed to the release page.
            let triple = crate::updater::check::triple_for_current_platform()
                .ok_or(UpdateError::UnsupportedPlatform)?;
            let expected_name =
                crate::updater::check::expected_asset_name(&info.latest_version, triple);
            if let Err(e) = signing::verify_release_asset(&zip_path, &sig_text, &expected_name) {
                warn!("release signature verification failed: {e:#}");
                return Err(UpdateError::SignatureInvalid);
            }
        }
        None if mirror_involved => {
            // Unsigned release + untrusted transport: refuse. The user
            // can still update manually from the release page.
            return Err(UpdateError::SignatureMissing);
        }
        None => {
            // Old unsigned release over a direct connection — the
            // pre-signing trust level, digest check above still applies.
        }
    }

    // The release zip has one wrapping directory around the binary and Web UI.
    let extract_dir = tempdir.path().join("extracted");
    std::fs::create_dir(&extract_dir)
        .with_context(|| format!("mkdir {}", extract_dir.display()))
        .map_err(UpdateError::from)?;
    extract_zip_safe(&zip_path, &extract_dir)
        .context("extract release zip")
        .map_err(UpdateError::from)?;

    let binary_name = current_exe
        .file_name()
        .ok_or_else(|| anyhow!("current_exe has no file name"))
        .map_err(UpdateError::from)?
        .to_owned();
    let new_binary = find_binary(&extract_dir, &binary_name).ok_or(UpdateError::NoMatchingAsset)?;
    let new_frontend = new_binary
        .parent()
        .map(|parent| parent.join("frontend"))
        .filter(|path| path.join("index.html").is_file())
        .ok_or(UpdateError::NoMatchingAsset)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new_binary, std::fs::Permissions::from_mode(0o755));
    }

    // Copy hashed assets first and index.html last, so the browser never sees
    // an entry point that refers to files which have not arrived yet.
    copy_frontend(&new_frontend, &install_dir.join("frontend"))
        .context("replace Web UI assets")
        .map_err(UpdateError::from)?;

    // Atomic in-place binary swap. On Unix this is a `rename` over the
    // currently-running ELF/Mach-O (the kernel keeps the running inode
    // alive). On Windows it's the documented copy + delete-on-close
    // dance — see the self_replace crate docs for the gory details.
    self_replace::self_replace(&new_binary)
        .context("self_replace::self_replace")
        .map_err(UpdateError::from)?;

    let exe = std::env::current_exe()
        .context("resolve updated executable")
        .map_err(UpdateError::from)?;
    std::process::Command::new(exe)
        .spawn()
        .context("restart updated executable")
        .map_err(UpdateError::from)?;
    std::process::exit(0);
}

fn copy_frontend(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else if entry.file_name() != "index.html" {
            std::fs::copy(from, to)?;
        }
    }
    std::fs::copy(source.join("index.html"), destination.join("index.html"))?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(from, to)?;
        }
    }
    Ok(())
}

/// Returns true if `dir` looks writable. We do an explicit write-probe
/// rather than relying on `metadata().permissions().readonly()` because
/// (a) on Unix permissions don't capture mount-level read-only flags
/// (squashfs, AppImage), and (b) on Windows readonly directories are
/// not actually a thing — ACLs are richer.
fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".akagi-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Walk `dir` looking for an entry whose file name matches `binary_name`.
/// The release zip wraps everything in a single top-level
/// `akagi-<version>-<triple>/` directory so we have to descend at least
/// one level — but we keep the walk bounded to avoid scanning the whole
/// runtime tree if the layout ever changes.
fn find_binary(dir: &Path, binary_name: &std::ffi::OsStr) -> Option<PathBuf> {
    // Direct hit at the top level — handles a hypothetical future
    // layout that drops the wrapping directory.
    let direct = dir.join(binary_name);
    if direct.is_file() {
        return Some(direct);
    }
    // Otherwise descend exactly one level. The release zip's wrapping
    // dir is the only thing we expect at the root, so this is enough.
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut z = ZipWriter::new(f);
        let opts = SimpleFileOptions::default();
        for (name, body) in entries {
            z.start_file(name.to_string(), opts).unwrap();
            z.write_all(body).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn find_binary_locates_inside_wrapped_dir() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("a.zip");
        make_zip(
            &zip,
            &[
                ("akagi-3.0.12-linux-x64/akagi", b"#!/bin/sh\nexit 0\n"),
                ("akagi-3.0.12-linux-x64/README.txt", b"hello"),
                (
                    "akagi-3.0.12-linux-x64/runtime/python/keep.txt",
                    b"not the binary",
                ),
            ],
        );
        let out = TempDir::new().unwrap();
        extract_zip_safe(&zip, out.path()).unwrap();
        let found = find_binary(out.path(), std::ffi::OsStr::new("akagi"))
            .expect("should find the binary one level down");
        assert!(found.ends_with("akagi-3.0.12-linux-x64/akagi"));
    }

    #[test]
    fn find_binary_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("akagi-3.0.12-linux-x64")).unwrap();
        std::fs::write(tmp.path().join("akagi-3.0.12-linux-x64/README.txt"), b"hi").unwrap();
        assert!(find_binary(tmp.path(), std::ffi::OsStr::new("akagi")).is_none());
    }

    #[test]
    fn copy_frontend_handles_nested_assets() {
        let source = TempDir::new().unwrap();
        let destination = TempDir::new().unwrap();
        std::fs::create_dir(source.path().join("assets")).unwrap();
        std::fs::write(source.path().join("index.html"), b"new index").unwrap();
        std::fs::write(source.path().join("assets/app.js"), b"new js").unwrap();
        std::fs::write(destination.path().join("index.html"), b"old index").unwrap();

        copy_frontend(source.path(), destination.path()).unwrap();

        assert_eq!(
            std::fs::read(destination.path().join("index.html")).unwrap(),
            b"new index"
        );
        assert_eq!(
            std::fs::read(destination.path().join("assets/app.js")).unwrap(),
            b"new js"
        );
    }

    #[test]
    fn find_binary_prefers_direct_hit() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("akagi"), b"top-level").unwrap();
        std::fs::create_dir_all(tmp.path().join("subdir")).unwrap();
        std::fs::write(tmp.path().join("subdir/akagi"), b"nested").unwrap();
        let found = find_binary(tmp.path(), std::ffi::OsStr::new("akagi")).unwrap();
        assert_eq!(found, tmp.path().join("akagi"));
    }

    #[test]
    fn is_dir_writable_for_tempdir() {
        let tmp = TempDir::new().unwrap();
        assert!(is_dir_writable(tmp.path()));
    }
}
