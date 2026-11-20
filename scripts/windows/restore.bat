@echo off
rem QueueTicket restore - double click to restore a backup.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0restore.ps1"
pause
