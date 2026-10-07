// Windows: put res/file-browser.ico into the exe (Explorer, taskbar, shortcuts).
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/file-browser.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") { return; }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.res");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
        if cfg!(windows) { "windres".into() } else { "x86_64-w64-mingw32-windres".into() }
    });
    let ok = std::process::Command::new(&windres)
        .args(["res/app.rc", "-O", "coff", "-o"]).arg(&out)
        .status().map(|s| s.success()).unwrap_or(false);
    if ok { println!("cargo:rustc-link-arg-bins={}", out.display()); }
    else { println!("cargo:warning=windres not found - building without the icon"); }
}
