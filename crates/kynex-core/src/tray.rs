//! Kynex running on with its window closed, so long turns and background
//! tasks carry on.
//!
//! - Windows: an icon in the notification area. Closing the window hides it
//!   there; clicking the icon brings it back, and its menu has Show and Quit.
//! - macOS: closing hides the app, as macOS apps do; the Dock icon brings it back.
//! - Linux: no tray yet, so closing the window quits, as before. Kynex is
//!   never left running with no way back to it.

use serde::{Deserialize, Serialize};

/// Where a hidden Kynex lives, if anywhere on this OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haven {
    /// The notification area (Windows).
    Tray,
    /// The Dock (macOS).
    Dock,
}

impl Haven {
    pub const fn of_os() -> Option<Self> {
        if cfg!(windows) {
            Some(Self::Tray)
        } else if cfg!(target_os = "macos") {
            Some(Self::Dock)
        } else {
            None
        }
    }

    /// The setting's wording: what closing the window does instead of quitting.
    pub fn label(self) -> &'static str {
        match self {
            Self::Tray => "Keep running in the notification area",
            Self::Dock => "Keep running in the Dock",
        }
    }
}

/// The Close window setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnClose {
    /// Hide the window and keep running, where the OS has somewhere to come back from.
    #[default]
    KeepRunning,
    Quit,
}

impl OnClose {
    /// Whether closing the window hides it (rather than quitting), given where
    /// a hidden Kynex lives on this OS.
    pub fn hides(self, haven: Option<Haven>) -> bool {
        self == Self::KeepRunning && haven.is_some()
    }
}

/// What was picked on the tray icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
}

pub use imp::{Tray, hide_window, show_window};

#[cfg(windows)]
mod imp {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
        GWLP_USERDATA, GetCursorPos, GetSystemMetrics, GetWindowLongPtrW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR,
        LoadImageW, MF_SEPARATOR, MF_STRING, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON,
        SM_CYSMICON, SW_HIDE, SW_SHOW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
    };

    use super::TrayEvent;

    /// The icon's messages to its window.
    const CALLBACK: u32 = WM_APP + 1;
    const SHOW: usize = 1;
    const QUIT: usize = 2;

    /// What the hidden window's procedure needs.
    struct State {
        on_event: Box<dyn Fn(TrayEvent)>,
        icon: NOTIFYICONDATAW,
        /// Explorer restarted: the icon has to be added again.
        taskbar_created: u32,
    }

    /// The notification-area icon; it goes away when this drops. Lives on the
    /// UI thread, whose message loop delivers its clicks.
    pub struct Tray {
        hwnd: HWND,
        state: *mut State,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    impl Tray {
        /// Adds the icon, with `tip` on hover. `on_event` runs on the UI thread,
        /// from inside its message loop: it should only pass the event on.
        pub fn new(tip: &str, on_event: Box<dyn Fn(TrayEvent)>) -> Result<Self, String> {
            let class = wide("KynexTray");
            // SAFETY: plain Win32 calls; every pointer passed lives across its call.
            unsafe {
                let instance = GetModuleHandleW(null());
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    ..Default::default()
                };
                // Fails harmlessly if a Tray was made before.
                RegisterClassW(&wc);
                // A plain top-level window that's never shown: a message-only one
                // wouldn't hear Explorer restart.
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                );
                if hwnd.is_null() {
                    return Err(format!("couldn't add the tray icon: {}", std::io::Error::last_os_error()));
                }
                // The exe's icon, resource 1 (build.rs), at the small-icon size.
                let icon = LoadImageW(
                    instance,
                    // MAKEINTRESOURCE(1)
                    std::ptr::without_provenance(1),
                    IMAGE_ICON,
                    GetSystemMetrics(SM_CXSMICON),
                    GetSystemMetrics(SM_CYSMICON),
                    LR_DEFAULTCOLOR,
                ) as HICON;
                let mut data = NOTIFYICONDATAW {
                    cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: hwnd,
                    uID: 1,
                    uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                    uCallbackMessage: CALLBACK,
                    hIcon: icon,
                    ..Default::default()
                };
                for (d, s) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
                    *d = s;
                }
                let state = Box::into_raw(Box::new(State {
                    on_event,
                    icon: data,
                    taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                }));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                if Shell_NotifyIconW(NIM_ADD, &(*state).icon) == 0 {
                    let e = std::io::Error::last_os_error();
                    drop(Self { hwnd, state });
                    return Err(format!("couldn't add the tray icon: {e}"));
                }
                Ok(Self { hwnd, state })
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            // SAFETY: `state` came from `Box::into_raw` in `new` and is freed once,
            // after the window that points at it is gone.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &(*self.state).icon);
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd);
                let state = Box::from_raw(self.state);
                if !state.icon.hIcon.is_null() {
                    DestroyIcon(state.icon.hIcon);
                }
            }
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: the pointer is the live `State` set in `Tray::new`, or 0.
        let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() };
        let Some(state) = state else { return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) } };
        if msg == CALLBACK {
            match lparam as u32 {
                WM_LBUTTONUP => (state.on_event)(TrayEvent::Show),
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    if let Some(ev) = unsafe { menu(hwnd) } {
                        (state.on_event)(ev);
                    }
                }
                _ => {}
            }
            return 0;
        }
        if msg == state.taskbar_created {
            unsafe { Shell_NotifyIconW(NIM_ADD, &state.icon) };
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// The icon's right-click menu, at the pointer; what was picked.
    unsafe fn menu(hwnd: HWND) -> Option<TrayEvent> {
        unsafe {
            let menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, SHOW, wide("Show Kynex").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, QUIT, wide("Quit Kynex").as_ptr());
            let mut at = POINT { x: 0, y: 0 };
            GetCursorPos(&mut at);
            // Without this the menu stays up when you click elsewhere.
            SetForegroundWindow(hwnd);
            let picked =
                TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, at.x, at.y, 0, hwnd, null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            match picked as usize {
                SHOW => Some(TrayEvent::Show),
                QUIT => Some(TrayEvent::Quit),
                _ => None,
            }
        }
    }

    /// Hides the main window (its `HWND`) from the screen and the taskbar.
    pub fn hide_window(window: isize) {
        // SAFETY: a no-op on a handle that isn't a window.
        unsafe { ShowWindow(window as HWND, SW_HIDE) };
    }

    /// Shows the main window again after [`hide_window`], as it was.
    pub fn show_window(window: isize) {
        // SAFETY: a no-op on a handle that isn't a window.
        unsafe { ShowWindow(window as HWND, SW_SHOW) };
    }
}

#[cfg(not(windows))]
mod imp {
    use super::TrayEvent;

    /// No tray icon outside Windows yet (see the module docs).
    pub struct Tray;

    impl Tray {
        pub fn new(_tip: &str, _on_event: Box<dyn Fn(TrayEvent)>) -> Result<Self, String> {
            Err("no tray icon on this system".into())
        }
    }

    /// The app hides itself on macOS (the caller does that); nothing here.
    pub fn hide_window(_window: isize) {}

    pub fn show_window(_window: isize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_hides_only_where_there_is_a_way_back() {
        assert!(OnClose::KeepRunning.hides(Some(Haven::Tray)));
        assert!(OnClose::KeepRunning.hides(Some(Haven::Dock)));
        assert!(!OnClose::KeepRunning.hides(None));
        assert!(!OnClose::Quit.hides(Some(Haven::Tray)));
        assert_eq!(Haven::of_os().is_some(), cfg!(any(windows, target_os = "macos")));
    }

    #[cfg(windows)]
    #[test]
    fn the_tray_icon_comes_and_goes() {
        let tray = Tray::new("Kynex test", Box::new(|_| {})).expect("tray icon");
        drop(tray);
    }
}
