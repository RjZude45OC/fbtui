# fbtui

**fbtui** (file browser TUI) is a fast, keyboard-driven console app for Windows: file browser and editor, script launcher, tasks & clock, and a set of small tools. It is written in Rust with [ratatui](https://ratatui.rs) and runs in Windows Terminal (pictures are drawn with Sixel) or the classic console.

## Features

- **File browser:**
  - Shows file previews with syntax highlighting, pictures, PDF pages, video frames, Office and zip contents.
  - Supports copy, cut and paste (including files copied in Explorer), rename, and delete to the Recycle Bin.
  - Searches names and text with Ctrl+F.
  - Keeps bookmarks, with Spotlight search on Tab.
  - Has a command prompt (F4 / Ctrl+T).
- **Editor:** undo and redo, find, go to line, word wrap and mouse selection.
- **Script launcher:** runs `.bat`, `.ps1`, `.py` and other scripts from a folder, with type badges and themes.
- **Tasks & clock:**
  - Tasks can be one-off or repeating (every day, every Monday, Mon–Fri, the 15th of each month…).
  - A task can be linked to a folder, file or web address: Alt+L creates one, Ctrl+O picks the link, G opens it.
  - Desktop reminders appear from the tray.
  - A work log (`worklog.md`) and a "done today" list.
  - The clock is an analog watch, with a countdown of your work hours that you can set per day.
- **Tools** (each can be turned off in the Esc list):
  - Calendar with holidays and vacations (Spain's national holidays built in) and an activity heatmap.
  - Monkeytype-style typing test.
  - Image gallery.
  - Git panel.
  - Folder compare.
  - Theme picker: 14 built-in themes plus all of monkeytype's themes, with a live preview.
- **Local AI (optional):**
  - Chat with an [Ollama](https://ollama.com) model (or any OpenAI-compatible server) that knows your tasks, calendar and work log.
  - The model can add tasks, tick them off, log work and add days off.
  - It can fill in the task form from a sentence (Ctrl+Space).
  - It is greyed out when Ollama is not installed.

## Keys (short list)

| Key | What |
|---|---|
| Tab | Spotlight: tools and bookmarks (type `exit` to quit) |
| Esc | tools list with on/off check boxes |
| → / ← | open / back |
| F1 | all shortcuts |
| Alt+T | Tasks & clock |
| Alt+C | calendar |
| Alt+P | themes |
| Alt+A | local AI |
| Alt+L | new task linked to the selected file or folder |
| Ctrl+Tab / F9 | next theme |

## Install

1. Download `file-browser.exe` from the Releases page.
2. Put it in a folder such as `file browser rust\migration\`. Settings (`config.ini`) and data (`tasks.json`, `worklog.md`, `calendar.json`) are kept next to `config.ini`; the exe looks for it next to itself and up to three folders above.
3. Optional:
   - `scripts\make-shortcuts.bat`: shortcuts with the app icon.
   - `scripts\tasks.bat`: opens Tasks & clock.
   - `scripts\get-deps.bat`: downloads PDFium and FFmpeg for PDF and video previews.

Command line:

- `file-browser.exe [folder]`: the file browser.
- `--menu [folder]`: the script launcher.
- `--tasks`: Tasks & clock.
- `--typing`: the typing test.
- `--shortcuts`: creates the shortcuts.

## Build

You need Rust 1.91 or newer.

- **Native build** (Windows, or Linux for development):

  ```
  cargo build --release
  ```

- **Windows exe from Linux** (needs mingw-w64; the linker is set in `.cargo/config.toml`):

  ```
  rustup target add x86_64-pc-windows-gnu
  cargo build --release --target x86_64-pc-windows-gnu
  ```

- **Tests:** `cargo test`.

## Credits

- Theme colours from [monkeytype](https://github.com/monkeytypegame/monkeytype): `src/mt_themes.rs` is generated from its theme list.
- Built with ratatui, ratatui-image, syntect, resvg, pdfium-render and image.

## License

GPL-3.0: see [LICENSE](LICENSE). The theme colours in `src/mt_themes.rs` come from monkeytype, which is also GPL-3.0.
