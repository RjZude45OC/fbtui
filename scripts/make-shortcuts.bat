@echo off
rem Desktop and Start menu shortcuts with the app icon (File Browser, Script Launcher, Tasks and Clock).
echo.
echo   Where should the shortcuts open the app?
echo.
echo   1  Its own window  - the taskbar shows the app icon  (classic console)
echo   2  Windows Terminal - sharpest pictures, but the taskbar shows the Windows Terminal icon
echo.
choice /c 12 /n /m "  Choose 1 or 2: "
if errorlevel 2 (
  "%~dp0migration\file-browser.exe" --shortcuts terminal
) else (
  "%~dp0migration\file-browser.exe" --shortcuts console
)
echo.
pause
