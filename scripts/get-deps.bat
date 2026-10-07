@echo off
rem get-deps.bat - downloads the optional preview helpers into migration\deps
rem   PDFium  (PDF pages)          https://github.com/bblanchon/pdfium-binaries   chromium/7881
rem   FFmpeg  (video, audio, HEIC)  https://github.com/BtbN/FFmpeg-Builds           n8.1 LGPL shared
rem Uses curl.exe and tar.exe, both built into Windows 10 / 11.
setlocal
cd /d "%~dp0"
if not exist deps mkdir deps
set TMPD=%TEMP%\fb_deps
if not exist "%TMPD%" mkdir "%TMPD%"

if exist deps\pdfium.dll (
  echo   PDFium: already there
) else (
  echo   Downloading PDFium ...
  curl.exe -L --fail -o "%TMPD%\pdfium.tgz" "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%%2F7881/pdfium-win-x64.tgz" || goto :fail
  tar -xzf "%TMPD%\pdfium.tgz" -C "%TMPD%" || goto :fail
  copy /y "%TMPD%\bin\pdfium.dll" deps\ >nul || goto :fail
  echo   PDFium: ok
)

if exist deps\ffmpeg\ffmpeg.exe (
  echo   FFmpeg: already there
) else (
  echo   Downloading FFmpeg ^(about 80 MB^) ...
  curl.exe -L --fail -o "%TMPD%\ffmpeg.zip" "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip" || goto :fail
  tar -xf "%TMPD%\ffmpeg.zip" -C "%TMPD%" || goto :fail
  if not exist deps\ffmpeg mkdir deps\ffmpeg
  for /d %%D in ("%TMPD%\ffmpeg-n8.1-*") do (
    copy /y "%%D\bin\*.*" deps\ffmpeg\ >nul
    copy /y "%%D\LICENSE.txt" deps\ffmpeg\ >nul
  )
  if not exist deps\ffmpeg\ffmpeg.exe goto :fail
  echo   FFmpeg: ok
)
rmdir /s /q "%TMPD%" 2>nul
echo.
echo   Done. Restart browser-rs.bat to use them.
pause
exit /b 0

:fail
echo.
echo   Download failed. Check the internet connection and try again.
pause
exit /b 1
