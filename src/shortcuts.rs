//! Desktop / Start menu shortcuts that start file-browser.exe directly (so they show its icon,
//! not the Command Prompt one a .bat file gets), and the update swap the .bat files used to do.

use std::path::{Path, PathBuf};

/// A newer exe that could not replace the running one (file-browser.new.exe next to it):
/// rename the running exe out of the way (Windows allows that) and put the new one in its place.
/// The next start runs the new version.
pub fn apply_pending_update() {
    let Ok(exe) = std::env::current_exe() else { return };
    let new = exe.with_file_name("file-browser.new.exe");
    if !new.is_file() { return; }
    let old = exe.with_file_name("file-browser.old.exe");
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(&exe, &old).is_ok() && std::fs::rename(&new, &exe).is_err() {
        let _ = std::fs::rename(&old, &exe);      // put it back if the new one could not move
    }
}

/// Remove the previous version left by apply_pending_update (only possible once it is not running).
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() { let _ = std::fs::remove_file(exe.with_file_name("file-browser.old.exe")); }
}

struct Link { name: &'static str, args: &'static str, what: &'static str }

const LINKS: &[Link] = &[
    Link { name: "File Browser", args: "", what: "Browse, preview and edit files" },
    Link { name: "Script Launcher", args: "--menu", what: "Run the scripts in the app folder" },
    Link { name: "Tasks & Clock", args: "--tasks", what: "To-do list, reminders, clock and countdown" },
];

const ICON: &[u8] = include_bytes!("../res/file-browser.ico");

/// The icon as a file (Windows Terminal profiles need one): %LOCALAPPDATA%\FileBrowser\file-browser.ico
fn icon_file() -> Option<PathBuf> {
    let p = crate::notify::data_dir().join("file-browser.ico");
    if std::fs::read(&p).map(|b| b == ICON).unwrap_or(false) { return Some(p); }
    std::fs::create_dir_all(p.parent()?).ok()?;
    std::fs::write(&p, ICON).ok()?;
    Some(p)
}

/// Windows Terminal: a profile per tool with the app's icon (shown on the tab), added as a
/// "fragment" so the user's own settings.json is not touched.
fn wt_profiles(exe: &Path, start_dir: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let wt = base.join(r"Microsoft\WindowsApps\wt.exe");
    // an app execution alias (0-byte link): exists() would follow it and fail
    if std::fs::symlink_metadata(&wt).is_err() { return None; }
    let icon = icon_file()?;
    let profiles: Vec<serde_json::Value> = LINKS.iter().map(|l| serde_json::json!({
        "name": l.name,
        "commandline": format!("\"{}\" {}", exe.display(), l.args).trim().to_string(),
        "icon": icon.display().to_string(),
        "startingDirectory": start_dir.display().to_string(),
        "tabTitle": l.name,
        "suppressApplicationTitle": true,
        "hidden": false,
    })).collect();
    let dir = base.join(r"Microsoft\Windows Terminal\Fragments\FileBrowser");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join("file-browser.json"), serde_json::to_string_pretty(&serde_json::json!({ "profiles": profiles })).ok()?).ok()?;
    Some(wt)
}

fn ps_quote(s: &str) -> String { format!("'{}'", s.replace('\'', "''")) }

/// `file-browser.exe --shortcuts`: create the shortcuts on the desktop and in the Start menu.
/// Where the shortcuts open the app.
#[derive(Clone, Copy, PartialEq)]
pub enum Window {
    /// its own classic console window (conhost): the taskbar shows the app's icon
    Console,
    /// a Windows Terminal tab: sharp pictures; the taskbar shows the Windows Terminal icon
    Terminal,
}

pub fn create(mode: Window) -> Result<Vec<String>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = strip_unc(&exe);
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let script_dir = dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dir.clone());
    // open through Windows Terminal with the app's profile (icon on the tab) when it is installed
    let wt = if mode == Window::Terminal { wt_profiles(&exe, &script_dir) } else { None };
    let conhost = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join(r"System32\conhost.exe")).filter(|p| p.exists());
    let mut ps = String::from("$ErrorActionPreference='Stop'; $sh=New-Object -ComObject WScript.Shell; \
        $desk=[Environment]::GetFolderPath('Desktop'); $start=Join-Path ([Environment]::GetFolderPath('Programs')) 'File Browser'; \
        New-Item -ItemType Directory -Force -Path $start | Out-Null; ");
    for l in LINKS {
        let (target, args) = match &wt {
            Some(wt) => (wt.display().to_string(), format!("-p \"{}\"", l.name)),
            // conhost.exe "file-browser.exe" ...: always the classic console, even when Windows Terminal is the default
            None => match (&conhost, mode) {
                (Some(c), Window::Console) => (c.display().to_string(), format!("\"{}\" {}", exe.display(), l.args).trim().to_string()),
                _ => (exe.display().to_string(), l.args.to_string()),
            },
        };
        for place in ["$desk", "$start"] {
            ps += &format!("$k=$sh.CreateShortcut((Join-Path {place} {name})); $k.TargetPath={exe}; $k.Arguments={args}; \
                $k.WorkingDirectory={wd}; $k.IconLocation={icon}; $k.Description={what}; $k.Save(); ",
                name = ps_quote(&format!("{}.lnk", l.name)), exe = ps_quote(&target), args = ps_quote(&args),
                wd = ps_quote(&script_dir.display().to_string()), icon = ps_quote(&format!("{},0", exe.display())), what = ps_quote(l.what));
        }
    }
    ps += "Write-Output $desk; Write-Output $start";
    if wt.is_some() { ps += "; Write-Output 'Opens in Windows Terminal (profiles File Browser, Script Launcher, Tasks & Clock: icon on the tab)'"; }
    else if mode == Window::Console { ps += "; Write-Output 'Opens in its own window: the taskbar shows the app icon'"; }
    let mut c = std::process::Command::new("powershell.exe");
    c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps]);
    let out = c.output().map_err(|e| format!("Cannot start PowerShell: {e}"))?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).trim().to_string()); }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

fn strip_unc(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") { Some(r) if !r.starts_with("UNC") => PathBuf::from(r), _ => p.to_path_buf() }
}

/// Classic console window: show the app's icon in its title bar and taskbar button.
/// (Windows Terminal keeps its own window icon; its tab shows the icon through the profile.)
pub fn set_console_icon() {
    #[cfg(windows)]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::System::Console::GetConsoleWindow;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let hwnd = GetConsoleWindow();
        if hwnd.0.is_null() { return; }
        let Ok(h) = GetModuleHandleW(None) else { return };
        for (which, size) in [(ICON_SMALL, GetSystemMetrics(SM_CXSMICON)), (ICON_BIG, GetSystemMetrics(SM_CXICON))] {
            if let Ok(icon) = LoadImageW(h, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
                SendMessageW(hwnd, WM_SETICON, WPARAM(which as usize), LPARAM(icon.0 as isize));
            }
        }
    }
}

/// Console entry: print what happened and wait for a key (it runs in its own window from make-shortcuts.bat).
/// `--shortcuts [console|terminal]`
pub fn run_cli(arg: Option<&str>) {
    println!();
    let mode = match arg.map(|a| a.to_lowercase()) {
        Some(a) if a.starts_with('t') => Window::Terminal,
        _ => Window::Console,
    };
    match create(mode) {
        Ok(places) => {
            println!("  Shortcuts created with the app's icon:");
            for l in LINKS { println!("    {}", l.name); }
            for p in places { println!("  in  {p}"); }
            println!("\n  Tip: right-click a shortcut > Pin to taskbar / Pin to Start.");
        }
        Err(e) => println!("  Could not create the shortcuts: {e}"),
    }
}
