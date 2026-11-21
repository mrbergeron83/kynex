//! Kynex in the Windows shell: stamping the AppUserModelID on Kynex's shortcuts (required for
//! Windows OS notifications and taskbar grouping) and refreshing cached shell icons on updates.

use std::path::{Path, PathBuf};

use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile, STGM_READ,
    STGM_READWRITE,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    IShellLinkW, SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW, SHChangeNotify, ShellLink,
};
use windows::core::{HSTRING, Interface};

pub fn shortcut_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        out.push(roaming.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    out.extend(dirs::desktop_dir());
    out
}

pub fn stamp_shortcuts(folders: &[PathBuf], exe: &Path, id: &str) -> Vec<PathBuf> {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let exe = same_path(exe);
    let links = folders
        .iter()
        .filter_map(|f| std::fs::read_dir(f).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")));
    links.filter(|link| matches!(stamp(link, &exe, id), Ok(true))).collect()
}

pub fn icon_folders() -> Vec<PathBuf> {
    let mut out = shortcut_folders();
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        out.push(roaming.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"));
    }
    out
}

pub fn refresh_icons(folders: &[PathBuf], exe: &Path) {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let target = same_path(exe);
    let links = folders
        .iter()
        .filter_map(|f| std::fs::read_dir(f).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")))
        .filter(|link| link_target(link).is_ok_and(|t| same_path(Path::new(&t)) == target));
    for path in std::iter::once(exe.to_path_buf()).chain(links) {
        let wide = HSTRING::from(path.as_os_str());
        unsafe { SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(wide.as_ptr().cast()), None) };
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn link_target(link: &Path) -> windows::core::Result<String> {
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = shell.cast()?;
        file.Load(&HSTRING::from(link.as_os_str()), STGM_READ)?;
        let mut target = [0u16; 1024];
        shell.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        let len = target.iter().position(|c| *c == 0).unwrap_or(target.len());
        Ok(String::from_utf16_lossy(&target[..len]))
    }
}

fn same_path(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().to_lowercase()
}

fn stamp(link: &Path, exe: &str, id: &str) -> windows::core::Result<bool> {
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = shell.cast()?;
        let path = HSTRING::from(link.as_os_str());
        file.Load(&path, STGM_READWRITE)?;
        let mut target = [0u16; 1024];
        shell.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        let len = target.iter().position(|c| *c == 0).unwrap_or(target.len());
        if same_path(Path::new(&String::from_utf16_lossy(&target[..len]))) != exe {
            return Ok(false);
        }
        let store: IPropertyStore = shell.cast()?;
        if store.GetValue(&PKEY_AppUserModel_ID)?.to_string() == id {
            return Ok(false);
        }
        store.SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(id))?;
        store.Commit()?;
        file.Save(&path, true)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(dir: &Path, name: &str, target: &str) -> PathBuf {
        let at = dir.join(name);
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            shell.SetPath(&HSTRING::from(target)).unwrap();
            shell.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(at.as_os_str()), true).unwrap();
        }
        at
    }

    #[test]
    fn stamps_only_shortcuts_to_this_exe_once() {
        let dir = std::env::temp_dir().join(format!("kynex-shortcuts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("kynex.exe");
        std::fs::write(&exe, b"").unwrap();
        let ours = link(&dir, "Kynex.lnk", &exe.to_string_lossy());
        link(&dir, "Other.lnk", r"C:\Windows\notepad.exe");
        let folders = [dir.clone()];
        assert_eq!(stamp_shortcuts(&folders, &exe, "Kynex.Test"), std::slice::from_ref(&ours));
        assert!(stamp_shortcuts(&folders, &exe, "Kynex.Test").is_empty());
        assert_eq!(stamp_shortcuts(&folders, &exe, "Kynex.Other"), [ours]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
