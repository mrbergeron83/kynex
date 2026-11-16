//! Kynex's hooks in the shells it starts, PowerShell and bash: at each prompt and as each
//! command starts the shell sets a title [`super::marker`] reads, and links what the command
//! prints. They print nothing and leave the user's prompt as it was. Other shells (zsh, fish…)
//! run without, and `knx pane run --wait` types its marker instead. `KYNEX_NO_SHELL_HOOKS`
//! set turns them off.

use std::path::PathBuf;

use base64::Engine;

const POWERSHELL: &str = include_str!("hooks/kynex.ps1");
const BASH: &str = include_str!("hooks/kynex.bash");

/// Whether shells get the hooks.
pub fn enabled() -> bool {
    std::env::var_os("KYNEX_NO_SHELL_HOOKS").is_none()
}

/// Whether `spec` starts its shell with the hooks: it will say when a command ends once its
/// first prompt is up, and a typed marker would be drowned by its titles.
pub fn has_hooks(spec: &super::ShellSpec) -> bool {
    spec.args.iter().any(|a| a == "-EncodedCommand" || a == "--init-file")
}

/// PowerShell's arguments with the hooks: run as a command after the profile, so no script
/// file and no execution policy is involved.
pub fn powershell_args() -> Vec<String> {
    let utf16: Vec<u8> = POWERSHELL.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    vec!["-NoLogo".into(), "-NoExit".into(), "-EncodedCommand".into(), encoded]
}

/// A shell's arguments, and variables to set for it.
pub type ArgsAndEnv = (Vec<String>, Vec<(String, String)>);

/// bash's arguments and variables with the hooks, as a login shell (the script reads the
/// login files, as `--login` would). `None` if the script can't be written. Writes a file.
pub fn bash_args() -> Option<ArgsAndEnv> {
    let file = script("bash", BASH)?;
    // Git Bash reads `C:/…` paths; elsewhere there are no backslashes.
    let path = file.to_string_lossy().replace('\\', "/");
    Some((vec!["--init-file".into(), path, "-i".into()], vec![("KYNEX_LOGIN".into(), "1".into())]))
}

/// The hooks script `text` for shell `ext` in the data dir (the user's own, so nobody else can
/// put words in it), written if it isn't there. Named by its text, so Kynex versions sharing
/// a data dir never rewrite one another's, nor one a shell is reading. Blocks on the file.
fn script(ext: &str, text: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(text.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    let dir = crate::paths::data_dir().join("shell");
    let file = dir.join(format!("kynex-{hash}.{ext}"));
    if std::fs::read_to_string(&file).is_ok_and(|t| t == text) {
        return Some(file);
    }
    std::fs::create_dir_all(&dir).ok()?;
    // Two Kynex starting shells at once mustn't read a half-written file.
    let part = dir.join(format!("kynex-{hash}.{ext}.{}", std::process::id()));
    std::fs::write(&part, text).ok()?;
    if std::fs::rename(&part, &file).is_err() {
        let _ = std::fs::remove_file(&part);
        // Another Kynex wrote it meanwhile.
        return std::fs::read_to_string(&file).is_ok_and(|t| t == text).then_some(file);
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_gets_the_hooks_as_an_encoded_command() {
        let args = powershell_args();
        assert_eq!(&args[..3], ["-NoLogo", "-NoExit", "-EncodedCommand"]);
        let bytes = base64::engine::general_purpose::STANDARD.decode(&args[3]).unwrap();
        let utf16: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&utf16).unwrap(), POWERSHELL);
    }

    #[test]
    fn the_scripts_say_the_marks_marker_reads() {
        for text in [POWERSHELL, BASH] {
            assert!(text.contains("knx-prompt:") && text.contains("knx-exec:") && text.contains("knx:out:"));
        }
    }
}
