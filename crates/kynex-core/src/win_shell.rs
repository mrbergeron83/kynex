//! Kynex in the Windows shell: the "needs you" dot over its taskbar button,
//! and its app ID on its own shortcuts.
//!
//! Notifications need the process to have an explicit AppUserModelID
//! ([`crate::attention::APP_ID`]). The taskbar groups windows by that ID, and a
//! shortcut without one stands for the exe's default ID, so a pinned Kynex
//! would show apart from the running window. [`stamp_shortcuts`] gives the
//! shortcuts to this exe the same ID.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile, STGM_READWRITE,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, ITaskbarList3, ShellLink, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, HICON};
use windows::core::{HSTRING, Interface};

/// Puts the amber dot over the taskbar button of window `hwnd`, or takes it
/// off. Call it on the window's (UI) thread, where COM is already set up.
pub fn set_overlay(hwnd: isize, on: bool) {
    let set = || -> windows::core::Result<()> {
        // SAFETY: plain COM calls on a live window handle; the icon outlives them.
        unsafe {
            let taskbar: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
            taskbar.HrInit()?;
            let hwnd = HWND(hwnd as *mut _);
            if on {
                taskbar.SetOverlayIcon(hwnd, dot()?, &HSTRING::from("A session needs you"))
            } else {
                taskbar.SetOverlayIcon(hwnd, HICON::default(), &HSTRING::new())
            }
        }
    };
    let _ = set();
}

/// A 16×16 amber dot, made once (the overlay draws it at 16×16).
fn dot() -> windows::core::Result<HICON> {
    static DOT: OnceLock<isize> = OnceLock::new();
    if let Some(&icon) = DOT.get() {
        return Ok(HICON(icon as *mut _));
    }
    const SIZE: i32 = 16;
    let mut bgra = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Edge softened over a pixel, with a thin dark ring so it reads on any taskbar.
            let d = (((x as f32 + 0.5 - 8.).powi(2) + (y as f32 + 0.5 - 8.).powi(2)).sqrt() - 6.5).clamp(-1., 1.);
            let alpha = ((1. - (d + 1.) / 2.) * 255.) as u8;
            let ring = d > -0.8;
            let (r, g, b) = if ring { (0x5c, 0x3b, 0x00) } else { (0xf5, 0xa6, 0x23) };
            bgra.extend_from_slice(&[b, g, r, alpha]);
        }
    }
    let mask = vec![0u8; (SIZE * SIZE / 8) as usize];
    // SAFETY: the buffers are SIZE×SIZE at 32 and 1 bits per pixel, as stated.
    let icon = unsafe { CreateIcon(None, SIZE, SIZE, 1, 32, mask.as_ptr(), bgra.as_ptr())? };
    Ok(HICON(*DOT.get_or_init(|| icon.0 as isize) as *mut _))
}

/// Where Kynex's shortcuts are: the Start menu (`install.ps1` puts one in
/// Programs), the desktop, and the taskbar's pinned items.
pub fn shortcut_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        out.push(roaming.join(r"Microsoft\Windows\Start Menu\Programs"));
        out.push(roaming.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"));
    }
    out.extend(dirs::desktop_dir());
    out
}

/// Gives every shortcut in `folders` that starts `exe` the app ID `id`, so
/// the taskbar keeps them with the running window. Returns the ones changed.
/// Blocks on COM and the files; run it off the UI thread.
pub fn stamp_shortcuts(folders: &[PathBuf], exe: &Path, id: &str) -> Vec<PathBuf> {
    // SAFETY: COM for this thread, as the shell objects below need; a thread
    // that has it already gets S_FALSE, which is fine too.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let exe = exe.to_string_lossy().to_lowercase();
    let links = folders
        .iter()
        .filter_map(|f| std::fs::read_dir(f).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")));
    links.filter(|link| matches!(stamp(link, &exe, id), Ok(true))).collect()
}

/// Sets `id` on `link` if it starts `exe` (lowercase) and has another ID; true if it did.
fn stamp(link: &Path, exe: &str, id: &str) -> windows::core::Result<bool> {
    // SAFETY: COM objects used and dropped on this thread; the path buffer is ours.
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = shell.cast()?;
        let path = HSTRING::from(link.as_os_str());
        file.Load(&path, STGM_READWRITE)?;
        let mut target = [0u16; 1024];
        shell.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        let len = target.iter().position(|c| *c == 0).unwrap_or(target.len());
        if String::from_utf16_lossy(&target[..len]).to_lowercase() != exe {
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

    /// A shortcut to `target` in `dir`, as `install.ps1` makes them.
    fn link(dir: &Path, name: &str, target: &str) -> PathBuf {
        let at = dir.join(name);
        // SAFETY: as in `stamp`.
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
        // Already stamped: left alone.
        assert!(stamp_shortcuts(&folders, &exe, "Kynex.Test").is_empty());
        assert_eq!(stamp_shortcuts(&folders, &exe, "Kynex.Other"), [ours]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn makes_the_dot() {
        assert!(dot().is_ok_and(|i| !i.is_invalid()));
    }
}
