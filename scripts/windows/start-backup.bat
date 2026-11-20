@echo off
rem QueueTicket backup - double click to start the continuous backup window.
rem Keep this PowerShell window open; closing it stops the backup.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0backup.ps1" -Loop
pause
