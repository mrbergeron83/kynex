//! The environment as it is now, for shells and sessions started after Kynex.
//!
//! A process's environment is a copy taken when it starts, so a tool installed
//! while Kynex runs wouldn't be on the PATH of the terminals and sessions it
//! opens. On Windows installers write the new PATH to the registry; [`vars`]
//! reads it back for each new child, as Windows Terminal does per tab.
//!
//! macOS and Linux need nothing here: their terminals start login shells, which
//! read the user's startup files, and `claude` runs its Bash tool from a
//! snapshot of the user's shell, taken at the session's start.

use std::ffi::OsString;

/// What a child started now should have on top of Kynex's own environment:
/// the PATH the registry has now (with anything Kynex's own PATH has besides),
/// and variables added since Kynex started. Variables Kynex already has keep
/// their values, so ones Windows sets per logon (`USERNAME`, `TEMP`…) stay
/// right. Empty on other platforms, or when nothing changed. Blocking: reads
/// the registry (well under a millisecond).
pub fn vars() -> Vec<(OsString, OsString)> {
    #[cfg(windows)]
    {
        let process: Vec<(String, String)> =
            std::env::vars_os().filter_map(|(n, v)| Some((n.into_string().ok()?, v.into_string().ok()?))).collect();
        let machine = registry::read(registry::Hive::Machine);
        let user = registry::read(registry::Hive::User);
        return combine(&machine, &user, &process).into_iter().map(|(n, v)| (n.into(), v.into())).collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// What changed, from the registry's machine and user variables (raw,
/// `%VAR%`s unexpanded) and the process's environment. Names compare without
/// case, as Windows does.
#[cfg_attr(not(windows), allow(dead_code))]
fn combine(
    machine: &[(String, String)],
    user: &[(String, String)],
    process: &[(String, String)],
) -> Vec<(String, String)> {
    let is_path = |n: &str| n.eq_ignore_ascii_case("Path");
    let get = |vars: &[(String, String)], name: &str| {
        vars.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    };
    // The user's variables win over the machine's; Path is both, machine first.
    let mut fresh: Vec<(String, String)> = machine.iter().filter(|(n, _)| !is_path(n)).cloned().collect();
    for (name, value) in user.iter().filter(|(n, _)| !is_path(n)) {
        fresh.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        fresh.push((name.clone(), value.clone()));
    }
    let lookup = |name: &str| get(&fresh, name).or_else(|| get(process, name));
    let mut out: Vec<(String, String)> =
        fresh.iter().filter(|(n, _)| get(process, n).is_none()).map(|(n, v)| (n.clone(), expand(v, &lookup))).collect();

    let registry_path = [get(machine, "Path"), get(user, "Path")].into_iter().flatten().collect::<Vec<_>>().join(";");
    let current = get(process, "Path").unwrap_or_default();
    let path = merge_path(&expand(&registry_path, &lookup), &current);
    if !registry_path.trim().is_empty() && path != current {
        let name = process.iter().find(|(n, _)| is_path(n)).map_or("Path".to_string(), |(n, _)| n.clone());
        out.push((name, path));
    }
    out
}

/// `%NAME%`s replaced by `lookup`'s values; unknown ones, and a lone `%`, stay.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand(text: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match lookup(&after[..end]) {
                Some(value) => {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('%');
                    out.push_str(&after[..end]);
                    rest = &after[end..];
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The registry's folders first, then the ones only the current PATH has
/// (Kynex's parent may have added some); each once, compared without case or
/// a trailing `\`.
#[cfg_attr(not(windows), allow(dead_code))]
fn merge_path(registry: &str, current: &str) -> String {
    let key = |d: &str| d.trim().trim_end_matches(['\\', '/']).to_lowercase();
    let mut seen: Vec<String> = Vec::new();
    let mut dirs: Vec<&str> = Vec::new();
    for dir in registry.split(';').chain(current.split(';')) {
        let k = key(dir);
        if !k.is_empty() && !seen.contains(&k) {
            seen.push(k);
            dirs.push(dir.trim());
        }
    }
    dirs.join(";")
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumValueW,
        RegOpenKeyExW,
    };

    pub enum Hive {
        Machine,
        User,
    }

    /// The string variables under the hive's environment key, unexpanded.
    pub fn read(hive: Hive) -> Vec<(String, String)> {
        let (root, path) = match hive {
            Hive::Machine => (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
            Hive::User => (HKEY_CURRENT_USER, "Environment"),
        };
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and `key` is written on success only.
        if unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut vars = Vec::new();
        let mut name = vec![0u16; 16_384];
        let mut data = vec![0u8; 64 * 1024];
        for index in 0.. {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: the buffers' sizes are passed alongside them.
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if status != ERROR_SUCCESS {
                // ERROR_NO_MORE_ITEMS; a value too big for the buffer would end
                // the list early too, but environment values are capped at 32K chars.
                break;
            }
            if kind != REG_SZ && kind != REG_EXPAND_SZ {
                continue;
            }
            let units: Vec<u16> =
                data[..data_len as usize].as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
            let value = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
            vars.push((String::from_utf16_lossy(&name[..name_len as usize]), value));
        }
        // SAFETY: `key` was opened above.
        unsafe { RegCloseKey(key) };
        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    #[test]
    fn expands_known_names_and_keeps_the_rest() {
        let lookup = |n: &str| (n.eq_ignore_ascii_case("home")).then(|| r"C:\Users\me".to_string());
        assert_eq!(expand(r"%HOME%\bin;%home%\x", &lookup), r"C:\Users\me\bin;C:\Users\me\x");
        assert_eq!(expand("%NOPE%;100%;%%", &lookup), "%NOPE%;100%;%%");
        assert_eq!(expand("a%NOPE%HOME%", &lookup), r"a%NOPEC:\Users\me");
    }

    #[test]
    fn registry_path_first_then_what_only_the_process_has() {
        let merged = merge_path(r"C:\Windows;C:\new\bin\", r"c:\windows\;C:\kynex-added;C:\new\bin");
        assert_eq!(merged, r"C:\Windows;C:\new\bin\;C:\kynex-added");
    }

    #[test]
    fn picks_up_a_new_install_without_touching_logon_variables() {
        let machine = pairs(&[("Path", r"%SystemRoot%\system32"), ("USERNAME", "SYSTEM"), ("TOOL_HOME", r"C:\m")]);
        let user = pairs(&[
            ("Path", r"%USERPROFILE%\.cargo\bin;%TOOL_HOME%\bin"),
            ("TEMP", r"%USERPROFILE%\AppData\Local\Temp"),
            ("tool_home", r"C:\u"),
        ]);
        let process = pairs(&[
            ("PATH", r"C:\Windows\system32;C:\dev"),
            ("SystemRoot", r"C:\Windows"),
            ("USERPROFILE", r"C:\Users\me"),
            ("USERNAME", "me"),
            ("TEMP", r"C:\Temp"),
        ]);
        let out = combine(&machine, &user, &process);
        assert_eq!(
            out,
            pairs(&[("tool_home", r"C:\u"), ("PATH", r"C:\Windows\system32;C:\Users\me\.cargo\bin;C:\u\bin;C:\dev"),])
        );
    }

    #[test]
    fn nothing_when_nothing_changed() {
        let machine = pairs(&[("Path", r"C:\a")]);
        let process = pairs(&[("Path", r"C:\a;C:\b")]);
        assert!(combine(&machine, &[], &process).is_empty());
        assert!(combine(&[], &[], &process).is_empty());
    }

    #[test]
    fn reads_without_panicking() {
        let _ = vars();
    }
}
