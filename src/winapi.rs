//! Windows-only helpers (thumbnails, shortcuts, drive/version info, clipboard, opening files).
//! On other systems they return nothing so the app still builds and runs for testing.

use image::DynamicImage;
use std::path::Path;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND, SIZE};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::UI::Shell::*;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> { s.encode_wide().chain(std::iter::once(0)).collect() }

    pub fn com_init() { unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); } }

    /// Explorer's thumbnail (photos, videos, PDFs ...) or with icon_only the file's icon.
    pub fn shell_image(path: &Path, size: i32, icon_only: bool) -> Option<DynamicImage> {
        unsafe {
            let p = HSTRING::from(path.as_os_str());
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&p, None).ok()?;
            let flags = if icon_only { SIIGBF_ICONONLY } else { SIIGBF_THUMBNAILONLY };
            let hbm = factory.GetImage(SIZE { cx: size, cy: size }, flags | SIIGBF_BIGGERSIZEOK).ok()?;
            let mut bm = BITMAP::default();
            let got = GetObjectW(HGDIOBJ(hbm.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
            if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 { let _ = DeleteObject(HGDIOBJ(hbm.0)); return None; }
            let (w, h) = (bm.bmWidth, bm.bmHeight);
            // ask for a top-down 32-bit copy: no upside-down or stride surprises
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h,
                    biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let hdc = GetDC(HWND::default());
            let lines = GetDIBits(hdc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
            ReleaseDC(HWND::default(), hdc);
            let _ = DeleteObject(HGDIOBJ(hbm.0));
            if lines == 0 { return None; }
            let has_alpha = buf.chunks_exact(4).any(|p| p[3] != 0);
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2);                                   // BGRA -> RGBA
                if !has_alpha { px[3] = 255; }
                else if px[3] > 0 && px[3] < 255 {               // un-premultiply
                    let a = px[3] as u32;
                    for c in 0..3 { px[c] = ((px[c] as u32 * 255 + a / 2) / a).min(255) as u8; }
                }
            }
            image::RgbaImage::from_raw(w as u32, h as u32, buf).map(DynamicImage::ImageRgba8)
        }
    }

    pub fn shortcut_info(path: &Path) -> Vec<String> {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "url" {
            return std::fs::read_to_string(path).unwrap_or_default().lines()
                .filter_map(|l| l.strip_prefix("URL=")).map(|u| format!("{:<12}{u}", "URL:")).collect();
        }
        let mut out = Vec::new();
        unsafe {
            let link: IShellLinkW = match CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) { Ok(l) => l, Err(_) => return out };
            let pf: IPersistFile = match link.cast() { Ok(p) => p, Err(_) => return out };
            let w = wide(path.as_os_str());
            if pf.Load(PCWSTR(w.as_ptr()), STGM_READ).is_err() { return out; }
            let mut buf = [0u16; 1024];
            let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
            if link.GetPath(&mut buf, std::ptr::null_mut(), 0).is_ok() { out.push(format!("{:<12}{}", "Target:", s(&buf))); }
            buf = [0u16; 1024];
            if link.GetArguments(&mut buf).is_ok() && buf[0] != 0 { out.push(format!("{:<12}{}", "Arguments:", s(&buf))); }
            buf = [0u16; 1024];
            if link.GetWorkingDirectory(&mut buf).is_ok() && buf[0] != 0 { out.push(format!("{:<12}{}", "Start in:", s(&buf))); }
            buf = [0u16; 1024];
            if link.GetDescription(&mut buf).is_ok() && buf[0] != 0 { out.push(format!("{:<12}{}", "Comment:", s(&buf))); }
        }
        out
    }

    pub fn drive_info(path: &Path) -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let w = wide(path.as_os_str());
            let mut label = [0u16; 261];
            let mut fs = [0u16; 261];
            if GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, Some(&mut fs)).is_ok() {
                let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
                out.push(format!("Label:   {}", s(&label)));
                out.push(format!("Format:  {}", s(&fs)));
            }
            let (mut free, mut total) = (0u64, 0u64);
            if GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total), Some(&mut free)).is_ok() {
                out.push(format!("Total:   {}", crate::text::format_size(total)));
                out.push(format!("Free:    {}", crate::text::format_size(free)));
            }
        }
        out
    }

    pub fn version_info(path: &Path) -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let w = wide(path.as_os_str());
            let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
            if size == 0 { return out; }
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(PCWSTR(w.as_ptr()), 0, size, data.as_mut_ptr() as *mut c_void).is_err() { return out; }
            let query = |q: &str| -> Option<String> {
                let qw: Vec<u16> = q.encode_utf16().chain(std::iter::once(0)).collect();
                let mut ptr: *mut c_void = std::ptr::null_mut();
                let mut len = 0u32;
                if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(qw.as_ptr()), &mut ptr, &mut len).as_bool() && len > 0 {
                    let sl = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                    let s = String::from_utf16_lossy(&sl[..sl.iter().position(|&c| c == 0).unwrap_or(sl.len())]);
                    let s = s.trim().to_string();
                    if !s.is_empty() { return Some(s); }
                }
                None
            };
            // language/codepage of the string table
            let mut lang = "040904b0".to_string();
            let tq: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain(std::iter::once(0)).collect();
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(tq.as_ptr()), &mut ptr, &mut len).as_bool() && len >= 4 {
                let t = std::slice::from_raw_parts(ptr as *const u16, 2);
                lang = format!("{:04x}{:04x}", t[0], t[1]);
            }
            for (label, key) in [("Description", "FileDescription"), ("Product", "ProductName"), ("Company", "CompanyName"),
                                 ("Version", "FileVersion"), ("Copyright", "LegalCopyright"), ("Original", "OriginalFilename")] {
                if let Some(v) = query(&format!("\\StringFileInfo\\{lang}\\{key}")) { out.push(format!("{:<12}{v}", format!("{label}:"))); }
            }
        }
        out
    }

    pub fn set_clipboard(text: &str) -> bool {
        unsafe {
            if OpenClipboard(HWND::default()).is_err() { return false; }
            let _ = EmptyClipboard();
            let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = w.len() * 2;
            let ok = (|| -> Option<()> {
                let h = GlobalAlloc(GMEM_MOVEABLE, bytes).ok()?;
                let p = GlobalLock(h) as *mut u16;
                if p.is_null() { return None; }
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(h);
                SetClipboardData(13 /* CF_UNICODETEXT */, HANDLE(h.0)).ok()?;
                Some(())
            })().is_some();
            let _ = CloseClipboard();
            ok
        }
    }
    pub fn get_clipboard() -> Option<String> {
        unsafe {
            OpenClipboard(HWND::default()).ok()?;
            let res = (|| -> Option<String> {
                let h = GetClipboardData(13).ok()?;                 // CF_UNICODETEXT
                let hg = windows::Win32::Foundation::HGLOBAL(h.0);
                let p = GlobalLock(hg) as *const u16;
                if p.is_null() { return None; }
                let mut n = 0usize;
                while *p.add(n) != 0 { n += 1; }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                let _ = GlobalUnlock(hg);
                Some(s)
            })();
            let _ = CloseClipboard();
            res
        }
    }

    /// Files on the clipboard (copied in Explorer or with Ctrl+C here).
    pub fn get_clipboard_files() -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        unsafe {
            if OpenClipboard(HWND::default()).is_err() { return out; }
            if let Ok(h) = GetClipboardData(15) {                  // CF_HDROP
                let hd = HDROP(h.0);
                let n = DragQueryFileW(hd, 0xFFFF_FFFF, None);
                for i in 0..n {
                    let len = DragQueryFileW(hd, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(hd, i, Some(&mut buf));
                    out.push(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len])));
                }
            }
            let _ = CloseClipboard();
        }
        out
    }

    /// Put files on the clipboard so Explorer can paste them too.
    pub fn set_clipboard_files(paths: &[std::path::PathBuf]) -> bool {
        unsafe {
            let mut list: Vec<u16> = Vec::new();
            for p in paths { list.extend(p.as_os_str().encode_wide()); list.push(0); }
            list.push(0);
            let head = std::mem::size_of::<DROPFILES>();
            let bytes = head + list.len() * 2;
            if OpenClipboard(HWND::default()).is_err() { return false; }
            let _ = EmptyClipboard();
            let ok = (|| -> Option<()> {
                let h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes).ok()?;
                let p = GlobalLock(h) as *mut u8;
                if p.is_null() { return None; }
                let df = DROPFILES { pFiles: head as u32, fWide: true.into(), ..Default::default() };
                std::ptr::copy_nonoverlapping(&df as *const DROPFILES as *const u8, p, head);
                std::ptr::copy_nonoverlapping(list.as_ptr() as *const u8, p.add(head), list.len() * 2);
                let _ = GlobalUnlock(h);
                SetClipboardData(15, HANDLE(h.0)).ok()?;
                Some(())
            })().is_some();
            let _ = CloseClipboard();
            ok
        }
    }

    /// Send to the Recycle Bin (no dialogs).
    pub fn recycle(p: &std::path::Path) -> Result<(), String> {
        unsafe {
            let mut from: Vec<u16> = p.as_os_str().encode_wide().collect();
            from.push(0); from.push(0);
            let mut op = SHFILEOPSTRUCTW {
                wFunc: FO_DELETE,
                pFrom: PCWSTR(from.as_ptr()),
                fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
                ..Default::default()
            };
            let r = SHFileOperationW(&mut op);
            if r != 0 { return Err(format!("error 0x{r:X}")); }
            if op.fAnyOperationsAborted.as_bool() { return Err("cancelled".into()); }
            Ok(())
        }
    }
}

/// How long ago this process was started (for the speed test).
pub fn process_age() -> Option<std::time::Duration> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).ok()?;
        let now = GetSystemTimeAsFileTime();
        let f = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        return Some(std::time::Duration::from_nanos(f(now).saturating_sub(f(c)) * 100));
    }
    #[cfg(not(windows))]
    {
        // Linux: /proc/self/stat field 22 (start time in clock ticks since boot)
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let after = stat.rsplit_once(')')?.1;
        let start_ticks: f64 = after.split_whitespace().nth(19)?.parse().ok()?;
        let up: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
        Some(std::time::Duration::from_secs_f64((up - start_ticks / 100.0).max(0.0)))
    }
}

#[cfg(windows)]
pub use imp::{com_init, drive_info, get_clipboard, get_clipboard_files, recycle, set_clipboard, set_clipboard_files, shell_image, shortcut_info, version_info};

#[cfg(not(windows))]
pub fn com_init() {}
#[cfg(not(windows))]
pub fn shell_image(_p: &Path, _s: i32, _i: bool) -> Option<DynamicImage> { None }
#[cfg(not(windows))]
pub fn shortcut_info(_p: &Path) -> Vec<String> { Vec::new() }
#[cfg(not(windows))]
pub fn drive_info(_p: &Path) -> Vec<String> { Vec::new() }
#[cfg(not(windows))]
pub fn version_info(_p: &Path) -> Vec<String> { Vec::new() }
#[cfg(not(windows))]
pub fn set_clipboard(t: &str) -> bool { *TEST_CLIP.lock().unwrap() = Some(t.to_string()); true }
#[cfg(not(windows))]
static TEST_CLIP: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
#[cfg(not(windows))]
pub fn get_clipboard() -> Option<String> { TEST_CLIP.lock().unwrap().clone() }
#[cfg(not(windows))]
static TEST_FILES: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());
#[cfg(not(windows))]
pub fn get_clipboard_files() -> Vec<std::path::PathBuf> { TEST_FILES.lock().unwrap().clone() }
#[cfg(not(windows))]
pub fn set_clipboard_files(p: &[std::path::PathBuf]) -> bool { *TEST_FILES.lock().unwrap() = p.to_vec(); true }
#[cfg(not(windows))]
pub fn recycle(p: &std::path::Path) -> Result<(), String> {
    // test builds: move into ~/.local/share/Trash-like folder next to the file
    let bin = std::env::temp_dir().join("fb_recycle");
    let _ = std::fs::create_dir_all(&bin);
    let dest = bin.join(format!("{}_{}", std::process::id(), p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    std::fs::rename(p, dest).map_err(|e| e.to_string())
}

/// Open with the default app.
pub fn open_default(p: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd").raw_arg(format!("/C start \"\" \"{}\"", p.display())).creation_flags(0x08000000).spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    { std::process::Command::new("xdg-open").arg(p).spawn().map(|_| ()) }
}

/// Explorer with the item selected (or the folder / This PC).
pub fn show_in_explorer(p: Option<&Path>, folder: Option<&Path>) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let arg = match (p, folder) {
            (Some(p), _) => format!("/select,\"{}\"", p.display()),
            (None, Some(f)) => format!("\"{}\"", f.display()),
            _ => "shell:MyComputerFolder".to_string(),
        };
        let _ = std::process::Command::new("explorer").raw_arg(arg).spawn();
    }
    #[cfg(not(windows))]
    { let _ = (p, folder); }
}
