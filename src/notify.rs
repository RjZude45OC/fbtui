//! Task reminders: a small background process with a tray icon that shows a Windows notification
//! 10 minutes before a task is due (configurable) and again when it is due.
//!
//! `file-browser.exe --notify` makes sure it is running (used by the Startup entry), the Tasks screen
//! does the same. It runs from a copy in %LOCALAPPDATA%\FileBrowser so file-browser.exe is never
//! locked and can be replaced by updates; a newer exe stops the old copy and starts itself.

use crate::tasks::{Store, Task};
use chrono::{Duration, Local, NaiveDateTime, NaiveTime};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One check of the task list.
pub struct State {
    pub store: Store,
    pub notified: HashMap<String, i64>,     // reminder key -> when it was shown (unix seconds)
    state_file: PathBuf,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub exe: PathBuf,                       // the real file-browser.exe (to open the Tasks screen)
    remind_before: i64,                     // minutes, 0 = only when due
    morning: NaiveTime,                     // tasks with a date but no time are announced at this time
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("FileBrowser")
}

impl State {
    pub fn new(tasks: PathBuf, exe: PathBuf) -> State {
        let state_file = data_dir().join("reminders_shown.json");
        let notified = std::fs::read_to_string(&state_file).ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, i64>>(&t).ok()).unwrap_or_default();
        let mut s = State { store: Store::open(tasks), notified, state_file, exe, remind_before: 10, morning: NaiveTime::from_hms_opt(9, 0, 0).unwrap() };
        s.read_settings();
        s
    }

    /// [tasks] remind_before = 10  /  morning = 09:00  in config.ini (next to tasks.json)
    fn read_settings(&mut self) {
        let Some(dir) = self.store.path.parent() else { return };
        let Ok(b) = std::fs::read(dir.join("config.ini")) else { return };
        let ini = crate::config::parse_ini(&crate::text::decode(&b).0);
        if let Some(v) = ini.get("tasks", "remind_before") { if let Ok(n) = v.trim().parse::<i64>() { self.remind_before = n.clamp(0, 24 * 60); } }
        if let Some(v) = ini.get("tasks", "morning") { if let Ok(crate::tasks::TimeIn::At(t)) = crate::tasks::parse_time(v) { self.morning = t; } }
    }

    /// Reminders that are due now: (title, text). Also returns the tray tooltip.
    pub fn tick(&mut self, now: NaiveDateTime) -> (Vec<(String, String)>, String) {
        if self.store.reload_if_changed() { self.read_settings(); }
        let mut out = Vec::new();
        let mut keys = Vec::new();
        let today = now.date();
        let catchup = Duration::hours(12);           // missed while the PC was off: still shown if not older than this
        for t in &self.store.tasks {
            if t.is_done(today) { continue; }
            let (when, timed) = match occurrence(t, now, self.morning) { Some(x) => x, None => continue };
            let at = when.format("%Y-%m-%d %H:%M").to_string();
            let due_key = format!("due:{}:{at}", t.id);
            let pre_key = format!("pre:{}:{at}", t.id);
            if when <= now {
                if when > now - catchup && !self.notified.contains_key(&due_key) {
                    let late = (now - when).num_minutes();
                    let text = if !timed { "Today".to_string() }
                        else if late < 2 { format!("Due now  ·  {}", when.format("%H:%M")) }
                        else { format!("Was due at {}  ·  {} ago", when.format("%H:%M"), crate::tasks::fmt_span(late)) };
                    out.push((t.title.clone(), text));
                    keys.push(due_key); keys.push(pre_key);
                }
            } else if timed && self.remind_before > 0 && when - Duration::minutes(self.remind_before) <= now && !self.notified.contains_key(&pre_key) {
                let mins = (when - now).num_minutes() + 1;
                out.push((t.title.clone(), format!("In {} min  ·  {}", mins, when.format("%H:%M"))));
                keys.push(pre_key);
            }
        }
        let late: Vec<&Task> = self.store.tasks.iter().filter(|t| t.overdue(now)).collect();
        // once a day: older late tasks that were not announced above
        let late_key = format!("late:{today}");
        if !late.is_empty() && out.is_empty() && !self.notified.contains_key(&late_key) {
            let names: Vec<&str> = late.iter().take(4).map(|t| t.title.as_str()).collect();
            out.push((format!("{} late {}", late.len(), if late.len() == 1 { "task" } else { "tasks" }), names.join("\n")));
        }
        if !late.is_empty() { keys.push(late_key); }
        if !keys.is_empty() {
            let ts = Local::now().timestamp();
            for k in keys { self.notified.entry(k).or_insert(ts); }
            let old = ts - 4 * 86_400;
            self.notified.retain(|_, v| *v > old);
            let _ = std::fs::create_dir_all(data_dir());
            let _ = std::fs::write(&self.state_file, serde_json::to_string(&self.notified).unwrap_or_default());
        }
        let todo = self.store.tasks.iter().filter(|t| !t.is_done(today) && (t.applies(today) || (!t.daily && t.date.map(|d| d <= today).unwrap_or(true)))).count();
        let mut tip = format!("Tasks  ·  {todo} to do");
        if !late.is_empty() { tip += &format!("  ·  {} late", late.len()); }
        // several at once: one notification
        if out.len() > 1 {
            let body: Vec<String> = out.iter().map(|(t, s)| format!("• {t}  ({})", s.lines().next().unwrap_or(""))).collect();
            out = vec![(format!("{} reminders", out.len()), body.join("\n"))];
        }
        (out, tip)
    }
}

/// The moment a task should be announced (and whether it has a real time).
fn occurrence(t: &Task, now: NaiveDateTime, morning: NaiveTime) -> Option<(NaiveDateTime, bool)> {
    if t.daily { return if t.applies(now.date()) { t.time.map(|x| (now.date().and_time(x), true)) } else { None }; }
    match (t.date, t.time) {
        (Some(d), Some(x)) => Some((d.and_time(x), true)),
        (Some(d), None) => Some((d.and_time(morning), false)),
        _ => None,
    }
}

fn stamp(exe: &Path, tasks: &Path) -> String {
    let md = std::fs::metadata(exe).ok();
    let len = md.as_ref().map(|m| m.len()).unwrap_or(0);
    let mt = md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    format!("{len}-{mt}|{}|{}", exe.display(), tasks.display())
}

const STARTUP_NAME: &str = "File browser reminders.vbs";

fn startup_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\Startup").join(STARTUP_NAME))
}

pub fn autostart_enabled() -> bool { startup_file().map(|p| p.is_file()).unwrap_or(false) }

/// A hidden-window script in the Startup folder that runs `file-browser.exe --notify` at logon.
pub fn set_autostart(on: bool) -> Result<(), String> {
    if !cfg!(windows) { return Ok(()); }
    let file = startup_file().ok_or("APPDATA is not set")?;
    if !on { return if file.exists() { std::fs::remove_file(&file).map_err(|e| e.to_string()) } else { Ok(()) }; }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bytes = startup_script(&exe);
    if let Some(d) = file.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    std::fs::write(&file, bytes).map_err(|e| e.to_string())
}

/// The Startup script for this exe. It checks that the exe is still there first, so a moved or deleted
/// app never shows a "cannot find the file" error at logon (it just does nothing).
fn startup_script(exe: &std::path::Path) -> Vec<u8> {
    let exe = exe.to_string_lossy().replace('"', "\"\"");
    let script = format!("' Starts the task reminders of the file browser (tray icon + notifications).\r\n\
        ' Made by file-browser.exe, which updates it when the app moves: press L in Tasks & clock to remove it.\r\n\
        exe = \"{exe}\"\r\n\
        If CreateObject(\"Scripting.FileSystemObject\").FileExists(exe) Then\r\n\
        \x20   CreateObject(\"WScript.Shell\").Run \"\"\"\" & exe & \"\"\" --notify\", 0, False\r\n\
        End If\r\n");
    // UTF-16 with BOM so any folder name works
    let mut bytes = vec![0xFF, 0xFE];
    for u in script.encode_utf16() { bytes.extend_from_slice(&u.to_le_bytes()); }
    bytes
}

/// The app was moved (or the script is an older kind): point the Startup script at this exe again.
/// Only when it is on, and never from the reminders' own copy in %LOCALAPPDATA%.
pub fn refresh_autostart() {
    if !cfg!(windows) { return; }
    let (Some(file), Ok(exe)) = (startup_file(), std::env::current_exe()) else { return };
    if !file.is_file() || exe.starts_with(data_dir()) { return; }
    if exe.file_name().map(|n| n.to_string_lossy().to_lowercase() != "file-browser.exe").unwrap_or(true) { return; }
    let want = startup_script(&exe);
    if std::fs::read(&file).map(|b| b != want).unwrap_or(true) { let _ = std::fs::write(&file, want); }
}

/// Start the reminders if they are not running (or are an older version).
pub fn ensure_running() -> Result<(), String> {
    refresh_autostart();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tasks = crate::tasks::default_path();
    let dir = data_dir();
    let copy = dir.join(if cfg!(windows) { "file-browser-reminders.exe" } else { "file-browser-reminders" });
    let stamp_file = dir.join("reminders.stamp");
    let want = stamp(&exe, &tasks);
    let up_to_date = std::fs::read_to_string(&stamp_file).map(|s| s == want).unwrap_or(false) && copy.is_file();
    let running = imp::is_running();
    if running && up_to_date { return Ok(()); }
    if !cfg!(windows) { return Ok(()); }
    if running { imp::stop(); }
    if !up_to_date {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::copy(&exe, &copy).map_err(|e| format!("cannot copy the exe to {}: {e}", dir.display()))?;
        std::fs::write(&stamp_file, &want).map_err(|e| e.to_string())?;
    }
    imp::spawn(&copy, &tasks, &exe)
}

pub fn is_running() -> bool { imp::is_running() }
pub fn test_notification() -> Result<(), String> { imp::test() }

/// `--notify-daemon <tasks.json> <file-browser.exe>`
pub fn daemon(tasks: PathBuf, exe: PathBuf) { imp::daemon(State::new(tasks, exe)); }

#[cfg(windows)]
mod imp {
    use super::State;
    use std::cell::RefCell;
    use std::path::Path;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const MUTEX: PCWSTR = w!("Local\\FileBrowserTaskReminders");
    const QUIT: PCWSTR = w!("Local\\FileBrowserTaskRemindersQuit");
    const TEST: PCWSTR = w!("Local\\FileBrowserTaskRemindersTest");
    const WM_TRAY: u32 = WM_APP + 1;
    const TICK_MS: u32 = 15_000;

    struct Daemon { state: State, hwnd: HWND, icon: HICON, big: HICON, quit: HANDLE, test: HANDLE, taskbar_created: u32, tip: String }
    thread_local! { static D: RefCell<Option<Daemon>> = const { RefCell::new(None) }; }

    pub fn is_running() -> bool {
        unsafe { match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, MUTEX) { Ok(h) => { let _ = CloseHandle(h); true } Err(_) => false } }
    }

    /// Ask the running copy to quit and wait for it (up to 4 s).
    pub fn stop() {
        unsafe {
            if let Ok(e) = OpenEventW(EVENT_MODIFY_STATE, false, QUIT) { let _ = SetEvent(e); let _ = CloseHandle(e); }
        }
        for _ in 0..40 { if !is_running() { break; } std::thread::sleep(std::time::Duration::from_millis(100)); }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    pub fn test() -> Result<(), String> {
        unsafe {
            match OpenEventW(EVENT_MODIFY_STATE, false, TEST) {
                Ok(e) => { let _ = SetEvent(e); let _ = CloseHandle(e); Ok(()) }
                Err(_) => Err("Reminders are not running  ·  press R to start them".into()),
            }
        }
    }

    pub fn spawn(copy: &Path, tasks: &Path, exe: &Path) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        const DETACHED: u32 = 0x8;
        const NEW_GROUP: u32 = 0x200;
        const BREAKAWAY: u32 = 0x0100_0000;
        let mk = || { let mut c = std::process::Command::new(copy); c.arg("--notify-daemon").arg(tasks).arg(exe)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()); c };
        // leave the terminal's job so closing the window does not end the reminders
        mk().creation_flags(DETACHED | NEW_GROUP | BREAKAWAY).spawn()
            .or_else(|_| mk().creation_flags(DETACHED | NEW_GROUP).spawn())
            .map(|_| ()).map_err(|e| e.to_string())
    }

    fn fill<const N: usize>(dst: &mut [u16; N], s: &str) {
        let v: Vec<u16> = s.encode_utf16().take(N - 1).collect();
        dst[..v.len()].copy_from_slice(&v);
        dst[v.len()] = 0;
    }

    fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
    }

    fn add_icon(d: &Daemon) {
        unsafe {
            let mut n = nid(d.hwnd);
            n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
            n.uCallbackMessage = WM_TRAY;
            n.hIcon = d.icon;
            fill(&mut n.szTip, &d.tip);
            let _ = Shell_NotifyIconW(NIM_ADD, &n);
            n.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &n);
        }
    }

    fn balloon(d: &Daemon, title: &str, text: &str) {
        unsafe {
            let mut n = nid(d.hwnd);
            n.uFlags = NIF_INFO;
            n.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
            n.hBalloonIcon = d.big;
            fill(&mut n.szInfoTitle, title);
            fill(&mut n.szInfo, text);
            let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
        }
    }

    fn set_tip(d: &mut Daemon, tip: &str) {
        if d.tip == tip { return; }
        d.tip = tip.to_string();
        unsafe {
            let mut n = nid(d.hwnd);
            n.uFlags = NIF_TIP | NIF_SHOWTIP;
            fill(&mut n.szTip, tip);
            let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
        }
    }

    fn open_tasks(d: &Daemon) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new(&d.state.exe).arg("--tasks").creation_flags(0x10 /* CREATE_NEW_CONSOLE */).spawn();
    }

    fn check(d: &mut Daemon) {
        let now = chrono::Local::now().naive_local();
        let (list, tip) = d.state.tick(now);
        set_tip(d, &tip);
        for (t, s) in list { balloon(d, &t, &s); }
    }

    fn menu(hwnd: HWND) -> u32 {
        unsafe {
            let Ok(m) = CreatePopupMenu() else { return 0 };
            let _ = AppendMenuW(m, MF_STRING, 1, w!("Open tasks && clock"));
            let _ = AppendMenuW(m, MF_STRING, 3, w!("Send a test notification"));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(m, MF_STRING, 2, w!("Stop reminders"));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, None);
            let _ = DestroyMenu(m);
            let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
            cmd.0 as u32
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let tb = D.with(|d| d.borrow().as_ref().map(|d| d.taskbar_created).unwrap_or(0));
        match msg {
            WM_TIMER => {
                if wp.0 == 2 { let _ = KillTimer(hwnd, 2); }
                if wp.0 == 3 {
                    // quit / test requests from the Tasks screen
                    let (quit, test) = D.with(|d| d.borrow().as_ref().map(|d| (d.quit, d.test)).unwrap_or_default());
                    if WaitForSingleObject(quit, 0) == WAIT_OBJECT_0 { let _ = DestroyWindow(hwnd); return LRESULT(0); }
                    if WaitForSingleObject(test, 0) == WAIT_OBJECT_0 {
                        D.with(|d| if let Some(d) = d.borrow().as_ref() { balloon(d, "Reminders are working", "This is how task reminders will look.") });
                    }
                    return LRESULT(0);
                }
                D.with(|d| if let Some(d) = d.borrow_mut().as_mut() { check(d) });
                LRESULT(0)
            }
            WM_TRAY => {
                let ev = (lp.0 as u32) & 0xFFFF;
                const NIN_SELECT_: u32 = WM_USER;
                const NIN_KEYSELECT_: u32 = WM_USER + 1;
                const NIN_BALLOONUSERCLICK_: u32 = WM_USER + 5;
                match ev {
                    NIN_SELECT_ | NIN_KEYSELECT_ | NIN_BALLOONUSERCLICK_ | WM_LBUTTONDBLCLK => D.with(|d| if let Some(d) = d.borrow().as_ref() { open_tasks(d) }),
                    WM_CONTEXTMENU | WM_RBUTTONUP => match menu(hwnd) {
                        1 => D.with(|d| if let Some(d) = d.borrow().as_ref() { open_tasks(d) }),
                        2 => { let _ = DestroyWindow(hwnd); }
                        3 => D.with(|d| if let Some(d) = d.borrow().as_ref() { balloon(d, "Reminders are working", "This is how task reminders will look.") }),
                        _ => {}
                    },
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let n = nid(hwnd);
                let _ = Shell_NotifyIconW(NIM_DELETE, &n);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ if tb != 0 && msg == tb => {
                // Explorer restarted: put the icon back
                D.with(|d| if let Some(d) = d.borrow().as_ref() { add_icon(d) });
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub fn daemon(state: State) {
        unsafe {
            let Ok(_mutex) = CreateMutexW(None, true, MUTEX) else { return };
            if GetLastError() == ERROR_ALREADY_EXISTS { return; }
            let quit = CreateEventW(None, true, false, QUIT).unwrap_or_default();
            let test = CreateEventW(None, false, false, TEST).unwrap_or_default();
            let Ok(hinst) = GetModuleHandleW(None) else { return };
            let class = w!("FileBrowserTaskReminders");
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("File browser reminders"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, hinst, None) else { return };
            let load = |size: i32| -> HICON {
                LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR).map(|h| HICON(h.0)).unwrap_or_else(|_| LoadIconW(None, IDI_INFORMATION).unwrap_or_default())
            };
            let icon = load(GetSystemMetrics(SM_CXSMICON));
            let big = load(GetSystemMetrics(SM_CXICON));
            let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
            let d = Daemon { state, hwnd, icon, big, quit, test, taskbar_created, tip: "Tasks".into() };
            add_icon(&d);
            D.with(|x| *x.borrow_mut() = Some(d));
            SetTimer(hwnd, 1, TICK_MS, None);
            SetTimer(hwnd, 2, 1500, None);        // first check soon after start
            SetTimer(hwnd, 3, 500, None);         // quit / test requests
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::State;
    use std::path::Path;
    pub fn is_running() -> bool { false }
    pub fn stop() {}
    pub fn test() -> Result<(), String> { Err("Notifications only work on Windows".into()) }
    pub fn spawn(_c: &Path, _t: &Path, _e: &Path) -> Result<(), String> { Ok(()) }
    /// Test build: print the reminders instead of showing them.
    pub fn daemon(mut state: State) {
        let secs: u64 = std::env::var("FB_NOTIFY_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
        loop {
            let (list, tip) = state.tick(chrono::Local::now().naive_local());
            for (t, s) in list { println!("NOTIFY: {t} | {}", s.replace('\n', " / ")); }
            println!("TIP: {tip}");
            if std::env::var("FB_NOTIFY_ONCE").is_ok() { return; }
            std::thread::sleep(std::time::Duration::from_secs(secs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reminders() {
        let dir = std::env::temp_dir().join(format!("fb_notify_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tasks.json");
        let now = Local::now().naive_local().date().and_hms_opt(12, 0, 0).unwrap();
        let mut s = Store::open(p.clone());
        s.add(crate::tasks::new_task("soon", false, Some(now.date()), NaiveTime::from_hms_opt(12, 5, 0)));
        s.add(crate::tasks::new_task("now", false, Some(now.date()), NaiveTime::from_hms_opt(12, 0, 0)));
        s.add(crate::tasks::new_task("later", false, Some(now.date()), NaiveTime::from_hms_opt(18, 0, 0)));
        s.add(crate::tasks::new_task("old", false, Some(now.date() - Duration::days(3)), None));
        s.add(crate::tasks::new_task("gym", true, None, NaiveTime::from_hms_opt(11, 59, 0)));
        s.save().unwrap();
        let mut st = State::new(p, PathBuf::new());
        st.notified.clear();
        let (list, tip) = st.tick(now);
        assert_eq!(list.len(), 1);
        assert!(list[0].0.contains("3 reminders"), "{list:?}");
        assert!(tip.contains("late"), "{tip}");
        // nothing new a minute later
        let (list2, _) = st.tick(now + Duration::minutes(1));
        assert!(list2.is_empty(), "{list2:?}");
        // "soon" is due at 12:05
        let (list3, _) = st.tick(now + Duration::minutes(5));
        assert_eq!(list3.len(), 1);
        assert_eq!(list3[0].0, "soon");
        assert!(list3[0].1.starts_with("Due now"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
