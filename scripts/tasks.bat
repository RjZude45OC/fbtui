@echo off
rem Tasks ^& clock: to-do list, daily tasks, reminders, clock and time left today.
if exist "%~dp0migration\file-browser.new.exe" move /y "%~dp0migration\file-browser.new.exe" "%~dp0migration\file-browser.exe" >nul 2>&1
"%~dp0migration\file-browser.exe" --tasks
