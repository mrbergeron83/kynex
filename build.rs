//! Embeds the app icon in kynex.exe on Windows (gpui gives every window resource 1)
//! and records the commit the build came from, for the About modal.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/kynex.ico");
    // Release builds run in GitHub Actions, which names the commit; locally ask git.
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let commit = std::env::var("GITHUB_SHA")
        .ok()
        .or_else(|| {
            let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .map_or_else(|| "dev".to_string(), |c| c.chars().take(7).collect());
    println!("cargo:rustc-env=KYNEX_COMMIT={commit}");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/kynex.ico");
        res.compile().expect("failed to embed the Windows icon");
    }
}
