//! Embeds the app icon in kynex.exe on Windows; gpui gives every window resource 1.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/kynex.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/kynex.ico");
        res.compile().expect("failed to embed the Windows icon");
    }
}
