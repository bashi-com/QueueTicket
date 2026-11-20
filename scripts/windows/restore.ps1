#Requires -Version 5.1
<#
  QueueTicket PostgreSQL restore for Windows 11 / PowerShell 5.1.
  Encoding: UTF-8 with BOM / CRLF.
#>
param(
    [string]$File = '',
    [switch]$Force
)

. (Join-Path $PSScriptRoot 'common.ps1')

try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$config = Read-BackupEnvFile
$databaseUrl = Get-BackupSetting 'DATABASE_URL' '' $config
$backupDir = Get-BackupSetting 'BACKUP_DIR' (Join-Path $env:USERPROFILE 'queueticket-backups') $config
$psqlConfig = Get-BackupSetting 'PSQL' '' $config

if ([string]::IsNullOrWhiteSpace($databaseUrl)) {
    Write-Host 'エラー: DATABASE_URL が設定されていません。backup.env を確認してください。' -ForegroundColor Red
    exit 2
}
$databaseUrl = Add-SslMode $databaseUrl

if ([string]::IsNullOrWhiteSpace($File)) {
    if (-not (Test-Path -LiteralPath $backupDir -PathType Container)) {
        Write-Host ('エラー: バックアップフォルダが見つかりません: ' + $backupDir) -ForegroundColor Red
        exit 1
    }
    $candidates = @(Get-ChildItem -LiteralPath $backupDir -File -Filter 'queueticket-*.sql.gz' |
        Sort-Object Name -Descending | Select-Object -First 10)
    if ($candidates.Count -eq 0) {
        Write-Host 'エラー: 復元できる queueticket-*.sql.gz がありません。' -ForegroundColor Red
        exit 1
    }
    Write-Host '復元するバックアップを選択してください（新しい順）:'
    for ($i = 0; $i -lt $candidates.Count; $i++) {
        Write-Host ('  [{0}] {1}' -f ($i + 1), $candidates[$i].Name)
    }
    $answer = Read-Host '番号を入力'
    $index = 0
    if (-not [int]::TryParse($answer, [ref]$index) -or $index -lt 1 -or $index -gt $candidates.Count) {
        Write-Host 'エラー: 正しい番号を入力してください。' -ForegroundColor Red
        exit 1
    }
    $File = $candidates[$index - 1].FullName
}

if (-not (Test-Path -LiteralPath $File -PathType Leaf)) {
    Write-Host ('エラー: バックアップファイルが見つかりません: ' + $File) -ForegroundColor Red
    exit 1
}

Write-Host 'Render の Web Service を停止（Suspend）しましたか？ 現在の DB 内容は選んだ時点に置き換わります。'
if (-not $Force) {
    $answer = Read-Host '続行する場合は yes と入力'
    if ($answer.Trim().ToLowerInvariant() -ne 'yes') {
        Write-Host '復元を中止しました。'
        exit 0
    }
}

$psql = Get-ToolExecutable -Configured $psqlConfig -ToolName 'psql'
if (-not $psql) {
    Write-Host ('エラー: ' + (Get-ToolNotFoundMessage 'psql')) -ForegroundColor Red
    exit 1
}
Write-Host ('psql: ' + $psql)

$id = [guid]::NewGuid().ToString('N')
$tmpSql = Join-Path $env:TEMP ('queueticket-restore-' + $id + '.sql')
$stderrPath = Join-Path $env:TEMP ('queueticket-restore-' + $id + '.err')
try {
    Expand-GzipToFile -GzipPath $File -DestinationPath $tmpSql

    $savedEap = $ErrorActionPreference
    $exitCode = 1
    $detail = ''
    try {
        $ErrorActionPreference = 'Continue'
        $psqlArgs = @('-v', 'ON_ERROR_STOP=1', '-f', ('"' + $tmpSql + '"'), ('"' + $databaseUrl + '"'))
        & $psql @psqlArgs 2> $stderrPath
        $exitCode = $LASTEXITCODE
    } catch {
        $exitCode = 1
        $detail = $_.Exception.Message
    } finally {
        $ErrorActionPreference = $savedEap
    }
    $stderrText = Read-TextFileAuto -Path $stderrPath
    if (-not [string]::IsNullOrWhiteSpace($stderrText)) {
        $detail = ($detail + "`r`n" + $stderrText).Trim()
    }

    if ($exitCode -eq 0) {
        Write-Host '復元が完了しました。'
        if ($detail) { Write-Host $detail }
        exit 0
    }
    Write-Host ('復元に失敗しました（psql の終了コード: ' + $exitCode + '）。') -ForegroundColor Red
    if ($detail) { Write-Host $detail -ForegroundColor Red }
    exit 1
} catch {
    Write-Host ('復元に失敗しました: ' + $_.Exception.Message) -ForegroundColor Red
    exit 1
} finally {
    foreach ($path in @($tmpSql, $stderrPath)) {
        if ($path -and (Test-Path -LiteralPath $path)) {
            Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
        }
    }
}
