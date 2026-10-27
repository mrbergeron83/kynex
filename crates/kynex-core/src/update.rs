//! Kynex updating itself from its GitHub releases. A release is one archive per
//! platform (`kynex-<target>.zip` / `.tar.gz`, see .github/workflows/release.yml)
//! plus `SHA256SUMS`. Updating downloads the archive next to the running exe,
//! checks it, unpacks it with the OS's `tar` and swaps the files in; the running
//! exe is renamed out of the way first (Windows can't overwrite it, but can
//! rename it), and the old copy is deleted on the next start. Like `cli`, it
//! talks HTTP through `curl`, and everything here blocks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::cli::is_newer;

pub const REPO: &str = "mrbergeron83/kynex";

/// Where the running exe is put aside during an update.
const OLD_SUFFIX: &str = "old";
/// The download and unpack folder, next to the exe so the final moves are renames.
const STAGING: &str = ".kynex-update";

/// This platform's release archive, if releases are built for it.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("kynex-x86_64-pc-windows-msvc.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("kynex-aarch64-apple-darwin.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("kynex-x86_64-unknown-linux-gnu.tar.gz")
    } else {
        None
    }
}

fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn run(cmd: &mut Command, what: &str) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("failed to {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!("failed to {what}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// The newest published (non-prerelease) version, without the `v`.
pub fn latest_version() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = run(
        command("curl").args(["-fsSL", "--max-time", "15", "-H", "Accept: application/vnd.github+json", &url]),
        "check for Kynex updates",
    )?;
    parse_latest(&body)
}

pub(crate) fn parse_latest(body: &[u8]) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("bad release info: {e}"))?;
    v["tag_name"]
        .as_str()
        .map(|t| t.trim_start_matches('v').to_string())
        .ok_or_else(|| "the latest release has no tag".to_string())
}

/// The newest version, if it's ahead of `current` and built for this platform.
pub fn check(current: &str) -> Result<Option<String>, String> {
    if asset_name().is_none() {
        return Ok(None);
    }
    let latest = latest_version()?;
    Ok(is_newer(&latest, current).then_some(latest))
}

/// The expected hash of `asset` in a `SHA256SUMS` file (`<hex>  <name>` lines).
pub(crate) fn expected_sum(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, name) = l.trim().split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == asset && hash.len() == 64).then(|| hash.to_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The system `tar`: bsdtar on Windows and macOS reads zip archives too. On
/// Windows, Git's GNU tar can come first on PATH and can't, so name System32's.
fn tar() -> PathBuf {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let t = Path::new(&root).join("System32").join("tar.exe");
        if t.exists() {
            return t;
        }
    }
    PathBuf::from("tar")
}

/// The running exe, unless it's a development build that shouldn't replace itself.
pub fn installed_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find the Kynex executable: {e}"))?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        return Err("this is a development build; it doesn't update itself".into());
    }
    Ok(exe)
}

/// Downloads `version`, checks it and swaps it in for the running exe. Kynex keeps
/// running the old code until it restarts.
pub fn install(version: &str) -> Result<(), String> {
    let asset = asset_name().ok_or("no Kynex builds for this platform")?;
    let exe = installed_exe()?;
    let dir = exe.parent().ok_or("the Kynex executable has no folder")?;
    let staging = dir.join(STAGING);
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("can't write to {}: {e}", dir.display()))?;
    let result = download_and_swap(version, asset, &exe, dir, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn download_and_swap(version: &str, asset: &str, exe: &Path, dir: &Path, staging: &Path) -> Result<(), String> {
    let base = format!("https://github.com/{REPO}/releases/download/v{version}");
    let archive = staging.join(asset);
    run(
        command("curl").args(["-fsSL", "--max-time", "300", "-o"]).arg(&archive).arg(format!("{base}/{asset}")),
        "download the update",
    )?;
    let sums =
        run(command("curl").args(["-fsSL", "--max-time", "30", &format!("{base}/SHA256SUMS")]), "download SHA256SUMS")?;
    let expected = expected_sum(&String::from_utf8_lossy(&sums), asset).ok_or("SHA256SUMS doesn't list this build")?;
    let bytes = fs::read(&archive).map_err(|e| format!("can't read the download: {e}"))?;
    if sha256_hex(&bytes) != expected {
        return Err("the download doesn't match its checksum".into());
    }

    let unpacked = staging.join("unpacked");
    fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    run(command(tar()).arg("-xf").arg(&archive).arg("-C").arg(&unpacked), "unpack the update")?;
    let new_exe = unpacked.join(exe.file_name().ok_or("bad executable name")?);
    if !new_exe.is_file() {
        return Err("the update doesn't contain the Kynex executable".into());
    }

    // Put the running exe aside (a previous update's copy may still be locked by an
    // old process; then pick another name), then move every file in.
    let old = (0..10)
        .map(|n| exe.with_extension(if n == 0 { OLD_SUFFIX.to_string() } else { format!("{OLD_SUFFIX}{n}") }))
        .find(|p| !p.exists() || fs::remove_file(p).is_ok())
        .ok_or("can't put the running Kynex aside")?;
    fs::rename(exe, &old).map_err(|e| format!("can't replace the running Kynex: {e}"))?;
    if let Err(e) = move_tree(&unpacked, dir) {
        // Put the running exe back so Kynex still starts.
        if !exe.exists() {
            let _ = fs::rename(&old, exe);
        }
        return Err(format!("couldn't install the update: {e}"));
    }
    Ok(())
}

/// Moves everything in `from` into `to`, replacing what's there.
fn move_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            move_tree(&entry.path(), &target)?;
        } else {
            if target.exists() {
                fs::remove_file(&target)?;
            }
            fs::rename(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Deletes the copies an earlier update put aside; call at startup.
pub fn clean_up() {
    let Ok(exe) = installed_exe() else { return };
    for n in 0..10 {
        let ext = if n == 0 { OLD_SUFFIX.to_string() } else { format!("{OLD_SUFFIX}{n}") };
        let _ = fs::remove_file(exe.with_extension(ext));
    }
}

/// Starts a fresh copy of the (updated) exe; the caller quits right after.
pub fn relaunch() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // On Linux the running exe's path gains " (deleted)" once it's replaced.
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    // Detached: on Windows a plain child joins Kynex's job object and is killed
    // with it when the caller quits, so the new Kynex would close straight away.
    crate::process::detach(&mut Command::new(&exe)).map(|_| ()).map_err(|e| format!("couldn't restart Kynex: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_latest_tag() {
        assert_eq!(parse_latest(br#"{"tag_name":"v0.3.1","name":"v0.3.1"}"#).unwrap(), "0.3.1");
        assert!(parse_latest(b"{}").is_err());
        assert!(parse_latest(b"not json").is_err());
    }

    #[test]
    fn finds_the_archive_checksum() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  kynex-x86_64-pc-windows-msvc.zip\n{b} *kynex-aarch64-apple-darwin.tar.gz\n");
        assert_eq!(expected_sum(&sums, "kynex-x86_64-pc-windows-msvc.zip"), Some(a));
        assert_eq!(expected_sum(&sums, "kynex-aarch64-apple-darwin.tar.gz"), Some("b".repeat(64)));
        assert_eq!(expected_sum(&sums, "kynex-x86_64-unknown-linux-gnu.tar.gz"), None);
    }

    #[test]
    fn hashes_like_sha256sum() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn moves_files_over_existing_ones() {
        let root = std::env::temp_dir().join(format!("kynex-move-{}", std::process::id()));
        let (from, to) = (root.join("from"), root.join("to"));
        fs::create_dir_all(from.join("fonts")).unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::write(from.join("kynex"), "new").unwrap();
        fs::write(from.join("fonts").join("OFL.txt"), "ofl").unwrap();
        fs::write(to.join("kynex"), "old").unwrap();
        move_tree(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("kynex")).unwrap(), "new");
        assert_eq!(fs::read_to_string(to.join("fonts").join("OFL.txt")).unwrap(), "ofl");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[ignore = "needs network"]
    fn checks_live() {
        assert!(!latest_version().unwrap().is_empty());
    }
}
