#Requires -Version 5.1
<#
  QueueTicket PostgreSQL backup for Windows 11 / PowerShell 5.1.
  Double-click start-backup.bat for the continuous (-Loop) mode.
  Encoding: UTF-8 with BOM / CRLF.
#>
param(
    [switch]$Loop,
    [switch]$Once
)

. (Join-Path $PSScriptRoot 'common.ps1')

try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$config = Read-BackupEnvFile
$databaseUrl = Get-BackupSetting 'DATABASE_URL' '' $config
$backupDir = Get-BackupSetting 'BACKUP_DIR' (Join-Path $env:USERPROFILE 'queueticket-backups') $config
$keepRaw = Get-BackupSetting 'KEEP' '1000' $config
$intervalRaw = Get-BackupSetting 'INTERVAL_MINUTES' '5' $config
$pgDumpConfig = Get-BackupSetting 'PG_DUMP' '' $config

if (-not (Test-PositiveInteger $keepRaw)) {
    Write-BackupLog -BackupDir $backupDir -Message 'ERROR: KEEP must be a positive integer.' -IsError
    exit 1
}
if (-not (Test-PositiveInteger $intervalRaw)) {
    Write-BackupLog -BackupDir $backupDir -Message 'ERROR: INTERVAL_MINUTES must be a positive integer.' -IsError
    exit 1
}
$keep = [int]$keepRaw
$intervalMinutes = [int]$intervalRaw

if ([string]::IsNullOrWhiteSpace($databaseUrl)) {
    if (-not $Loop) {
        # Same as scripts/backup.sh: one-shot mode never prompts.
        Write-BackupLog -BackupDir $backupDir -Message 'ERROR: DATABASE_URL is not set.' -IsError
        exit 2
    }
    # -AsSecureString keeps the URL from being echoed on the projection screen.
    $secureUrl = Read-Host 'Render の External Database URL を貼り付けてください' -AsSecureString
    $urlPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureUrl)
    try {
        $databaseUrl = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($urlPointer)
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($urlPointer)
    }
    if ([string]::IsNullOrWhiteSpace($databaseUrl)) {
        Write-BackupLog -BackupDir $backupDir -Message 'ERROR: DATABASE_URL is not set.' -IsError
        exit 2
    }
    $databaseUrl = Add-SslMode $databaseUrl
    try {
        Save-DatabaseUrl -Url $databaseUrl -Path $script:BackupEnvPath
        Write-Host 'DATABASE_URL を backup.env に保存しました。'
    } catch {
        Write-BackupLog -BackupDir $backupDir -Message ('ERROR: backup.env に保存できませんでした: ' + $_.Exception.Message) -IsError
        exit 1
    }
} else {
    $databaseUrl = Add-SslMode $databaseUrl
}

# Give up a stalled connection after 20 s so the loop keeps running.
if (-not $env:PGCONNECT_TIMEOUT) { $env:PGCONNECT_TIMEOUT = "20" }
$pgDump = Get-ToolExecutable -Configured $pgDumpConfig -ToolName 'pg_dump'
if (-not $pgDump) {
    Write-BackupLog -BackupDir $backupDir -Message ('ERROR: ' + (Get-ToolNotFoundMessage 'pg_dump')) -IsError
    exit 1
}
Write-Host ('pg_dump: ' + $pgDump)

function Invoke-OneAndLog {
    $result = Invoke-BackupOnce -DatabaseUrl $databaseUrl -BackupDir $backupDir -Keep $keep -PgDumpPath $pgDump
    $indent = ($result.Detail -split "`r?`n") | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        ForEach-Object { '  ' + $_ }
    $message = $result.Message
    if ($indent) { $message = $message + "`r`n" + ($indent -join "`r`n") }
    Write-BackupLog -BackupDir $backupDir -Message $message -IsError:(-not $result.Success)
    return $result
}

if (-not $Loop -or $Once) {
    $result = Invoke-OneAndLog
    if ($result.Success) { exit 0 }
    exit 1
}

# ------------------------------------------------------------------ Loop mode
$createdNew = $false
$mutex = $null
try {
    $mutex = New-Object System.Threading.Mutex($true, 'Global\QueueTicketBackup', [ref]$createdNew)
} catch {
    try {
        $mutex = New-Object System.Threading.Mutex($true, 'Local\QueueTicketBackup', [ref]$createdNew)
    } catch { }
}
if (-not $createdNew) {
    Write-Host 'バックアップは既に別のウィンドウで実行中です。このウィンドウを閉じます。'
    exit 0
}
# Keep the mutex alive for the whole process.
$script:BackupMutex = $mutex

if (-not ('QueueTicketPower' -as [type])) {
    # A stray LIB environment variable can make csc fail; ignore it temporarily.
    $savedLib = [Environment]::GetEnvironmentVariable('LIB')
    try {
        [Environment]::SetEnvironmentVariable('LIB', $null)
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class QueueTicketPower {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern uint SetThreadExecutionState(uint esFlags);
}
'@ -ErrorAction SilentlyContinue
    } catch {
        # Sleep prevention is best-effort; backups must still run.
    } finally {
        [Environment]::SetEnvironmentVariable('LIB', $savedLib)
    }
}
# ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
if ('QueueTicketPower' -as [type]) {
    try {
        [void][QueueTicketPower]::SetThreadExecutionState([uint32]2147483651)
    } catch { }
}

Write-Host 'このウィンドウを閉じるとバックアップが止まります。'
Write-Host ('バックアップ間隔: {0} 分' -f $intervalMinutes)

$lastSuccess = $null
$consecutiveFailures = 0
while ($true) {
    $result = Invoke-OneAndLog
    if ($result.Success) {
        $consecutiveFailures = 0
        $lastSuccess = Get-JstNow
        $title = 'QueueTicket バックアップ 最終成功 ' + $lastSuccess.ToString('HH:mm:ss')
    } else {
        $consecutiveFailures++
        $title = 'QueueTicket バックアップ 失敗あり'
        if ($consecutiveFailures -gt 1) {
            $title = $title + (' 連続 ' + $consecutiveFailures + ' 回失敗中')
        }
        if ($lastSuccess) {
            $title = $title + (' 最終成功 ' + $lastSuccess.ToString('HH:mm:ss'))
        }
        Write-Host ('連続 {0} 回失敗中' -f $consecutiveFailures) -ForegroundColor Red
    }
    try { $Host.UI.RawUI.WindowTitle = $title } catch { }
    Start-Sleep -Seconds ($intervalMinutes * 60)
}
