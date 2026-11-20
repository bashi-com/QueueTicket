#Requires -Version 5.1
<#
  Acceptance tests for the Windows backup/restore scripts (PowerShell 5.1).
  Encoding: UTF-8 with BOM / CRLF.
#>
param()

try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

# GetFullPath removes the ".." and the PowerShell provider prefix from the path,
# which -File would otherwise reject.
$Root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$CommonPs1 = Join-Path $Root 'scripts\windows\common.ps1'
$BackupPs1 = Join-Path $Root 'scripts\windows\backup.ps1'
$RestorePs1 = Join-Path $Root 'scripts\windows\restore.ps1'
. $CommonPs1
$script:TmpRoot = Join-Path $env:TEMP ('queueticket-backup-test-' + [guid]::NewGuid().ToString('N'))
[System.IO.Directory]::CreateDirectory($script:TmpRoot) | Out-Null

$script:PassCount = 0
$script:FailCount = 0
function Record-Pass {
    param([string]$Name)
    $script:PassCount++
    Write-Host ('PASS: ' + $Name) -ForegroundColor Green
}
function Record-Fail {
    param([string]$Name, [string]$Why)
    $script:FailCount++
    Write-Host ('FAIL: ' + $Name + ' - ' + $Why) -ForegroundColor Red
}
function Assert-True {
    param([bool]$Condition, [string]$Name, [string]$Why)
    if ($Condition) { Record-Pass $Name } else { Record-Fail $Name $Why }
}
function Write-BomPs1 {
    param([string]$Path, [string]$Body)
    $normalized = ($Body -replace "`r`n", "`n") -replace "`n", "`r`n"
    [System.IO.File]::WriteAllText($Path, $normalized, (New-Object System.Text.UTF8Encoding($true)))
}
function New-FakeWrapper {
    param([string]$CmdPath, [string]$Ps1Name)
    $body = '@echo off' + "`r`n" +
        'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0' + $Ps1Name + '" %*' + "`r`n" +
        'exit /b %errorlevel%' + "`r`n"
    [System.IO.File]::WriteAllText($CmdPath, $body, (New-Object System.Text.UTF8Encoding($false)))
}
function Invoke-ScriptWithEnv {
    param([string]$ScriptPath, [hashtable]$Environment, [string[]]$Arguments = @())
    $names = @('DATABASE_URL', 'BACKUP_DIR', 'KEEP', 'INTERVAL_MINUTES', 'PG_DUMP', 'PSQL', 'FAKE_PSQL_CAPTURE')
    $saved = @{}
    foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name) }
    try {
        foreach ($name in $names) {
            if ($Environment.ContainsKey($name) -and $null -ne $Environment[$name]) {
                [Environment]::SetEnvironmentVariable($name, [string]$Environment[$name])
            } else {
                [Environment]::SetEnvironmentVariable($name, $null)
            }
        }
        $log = Join-Path $script:TmpRoot ('run-' + [guid]::NewGuid().ToString('N') + '.log')
        if ($Arguments.Count -gt 0) {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $ScriptPath @Arguments > $log 2>&1
        } else {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $ScriptPath > $log 2>&1
        }
        return $LASTEXITCODE
    } finally {
        foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
    }
}

$script:ExpectedText = "-- 日本語ダンプ`r`nCREATE TABLE tickets (id int);`r`nINSERT INTO tickets (name) VALUES ('整理券');`r`n"
$script:ExpectedBytes = (New-Object System.Text.UTF8Encoding($false)).GetBytes($script:ExpectedText)
$script:TestCaseUrl = 'postgres://dummy:pass@localhost:5432/db?sslmode=require'

try {
    # ---- Fake tools -----------------------------------------------------------
    $fakeDir = Join-Path $script:TmpRoot 'fakes'
    [System.IO.Directory]::CreateDirectory($fakeDir) | Out-Null

    $okBody = @'
$found = $null
for ($i = 0; $i -lt $args.Count - 1; $i++) {
    if ($args[$i] -eq '--file') { $found = $args[$i + 1] }
}
if (-not $found) { Write-Error '--file not found'; exit 1 }
$text = "-- 日本語ダンプ`r`nCREATE TABLE tickets (id int);`r`nINSERT INTO tickets (name) VALUES ('整理券');`r`n"
[System.IO.File]::WriteAllText($found, $text, (New-Object System.Text.UTF8Encoding($false)))
exit 0
'@
    Write-BomPs1 -Path (Join-Path $fakeDir 'fake_pg_dump_ok.ps1') -Body $okBody
    $fakeDumpOk = Join-Path $fakeDir 'fake_pg_dump_ok.cmd'
    New-FakeWrapper -CmdPath $fakeDumpOk -Ps1Name 'fake_pg_dump_ok.ps1'

    $failBody = @'
[Console]::Error.WriteLine('pg_dump simulated error: server version mismatch')
exit 1
'@
    Write-BomPs1 -Path (Join-Path $fakeDir 'fake_pg_dump_fail.ps1') -Body $failBody
    $fakeDumpFail = Join-Path $fakeDir 'fake_pg_dump_fail.cmd'
    New-FakeWrapper -CmdPath $fakeDumpFail -Ps1Name 'fake_pg_dump_fail.ps1'

    $psqlBody = @'
$found = $null
for ($i = 0; $i -lt $args.Count - 1; $i++) {
    if ($args[$i] -eq '-f') { $found = $args[$i + 1] }
}
if (-not $found) { Write-Error '-f not found'; exit 1 }
$capture = $env:FAKE_PSQL_CAPTURE
if (-not $capture) { exit 1 }
[System.IO.File]::WriteAllBytes($capture, [System.IO.File]::ReadAllBytes($found))
exit 0
'@
    Write-BomPs1 -Path (Join-Path $fakeDir 'fake_psql.ps1') -Body $psqlBody
    $fakePsql = Join-Path $fakeDir 'fake_psql.cmd'
    New-FakeWrapper -CmdPath $fakePsql -Ps1Name 'fake_psql.ps1'

    # ---- Case 1: successful backup, byte-identical gzip content ---------------
    $dir1 = Join-Path $script:TmpRoot 'case1'
    $rc1 = Invoke-ScriptWithEnv -ScriptPath $BackupPs1 -Environment @{
        DATABASE_URL = $script:TestCaseUrl; BACKUP_DIR = $dir1; PG_DUMP = $fakeDumpOk
    }
    $files1 = @(Get-ChildItem -LiteralPath $dir1 -File -Filter 'queueticket-*.sql.gz' -ErrorAction SilentlyContinue)
    $contentOk = $false
    if ($files1.Count -eq 1) {
        $bytes1 = Read-GzipBytes -Path $files1[0].FullName
        $contentOk = ([Convert]::ToBase64String($bytes1) -ceq [Convert]::ToBase64String($script:ExpectedBytes))
    }
    Assert-True ($rc1 -eq 0 -and $files1.Count -eq 1 -and $contentOk) 'case1: gzip created with exact UTF-8 content' ("rc=$rc1 files=$($files1.Count) match=$contentOk")

    # ---- Case 2: KEEP=3 rotation, other files untouched -----------------------
    $dir2 = Join-Path $script:TmpRoot 'case2'
    [System.IO.Directory]::CreateDirectory($dir2) | Out-Null
    foreach ($ts in @('20200101-000001', '20200101-000002', '20200101-000003', '20200101-000004')) {
        [System.IO.File]::WriteAllText((Join-Path $dir2 ("queueticket-$ts.sql.gz")), "dummy $ts")
    }
    [System.IO.File]::WriteAllText((Join-Path $dir2 'keep-other-file.txt'), 'keep me')
    $rc2 = Invoke-ScriptWithEnv -ScriptPath $BackupPs1 -Environment @{
        DATABASE_URL = $script:TestCaseUrl; BACKUP_DIR = $dir2; KEEP = '3'; PG_DUMP = $fakeDumpOk
    }
    $files2 = @(Get-ChildItem -LiteralPath $dir2 -File -Filter 'queueticket-*.sql.gz')
    $names2 = @($files2 | ForEach-Object { $_.Name })
    $rotationOk = ($rc2 -eq 0 -and $files2.Count -eq 3 -and
        $names2 -notcontains 'queueticket-20200101-000001.sql.gz' -and
        $names2 -notcontains 'queueticket-20200101-000002.sql.gz' -and
        $names2 -contains 'queueticket-20200101-000003.sql.gz' -and
        $names2 -contains 'queueticket-20200101-000004.sql.gz' -and
        (Test-Path -LiteralPath (Join-Path $dir2 'keep-other-file.txt')))
    Assert-True $rotationOk 'case2: KEEP=3 keeps newest 3 and other files' ("rc=$rc2 files=$($files2.Count)")

    # ---- Case 3: pg_dump failure leaves no temporary files --------------------
    $dir3 = Join-Path $script:TmpRoot 'case3'
    $rc3 = Invoke-ScriptWithEnv -ScriptPath $BackupPs1 -Environment @{
        DATABASE_URL = $script:TestCaseUrl; BACKUP_DIR = $dir3; PG_DUMP = $fakeDumpFail
    }
    $leftovers3 = @(Get-ChildItem -LiteralPath $dir3 -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like '*.sql' -or $_.Name -like '*.tmp' -or $_.Name -like '*.err' })
    Assert-True ($rc3 -ne 0 -and $leftovers3.Count -eq 0) 'case3: failure cleans up temp files' ("rc=$rc3 leftovers=$($leftovers3.Count)")

    # ---- Case 4: missing DATABASE_URL in one-shot mode -> exit 2 --------------
    $dir4 = Join-Path $script:TmpRoot 'case4'
    $rc4 = Invoke-ScriptWithEnv -ScriptPath $BackupPs1 -Environment @{
        DATABASE_URL = $null; BACKUP_DIR = $dir4; PG_DUMP = $fakeDumpOk
    }
    Assert-True ($rc4 -eq 2) 'case4: missing DATABASE_URL exits 2' ("rc=$rc4")

    # ---- Case 5: invalid KEEP -> non-zero exit --------------------------------
    $dir5 = Join-Path $script:TmpRoot 'case5'
    $rc5 = Invoke-ScriptWithEnv -ScriptPath $BackupPs1 -Environment @{
        DATABASE_URL = $script:TestCaseUrl; BACKUP_DIR = $dir5; KEEP = 'abc'; PG_DUMP = $fakeDumpOk
    }
    Assert-True ($rc5 -ne 0) 'case5: KEEP=abc exits non-zero' ("rc=$rc5")

    # ---- Case 6: sslmode helper ------------------------------------------------
    Assert-True ((Add-SslMode 'postgres://a/b') -eq 'postgres://a/b?sslmode=require') 'case6a: append sslmode with ?' (Add-SslMode 'postgres://a/b')
    Assert-True ((Add-SslMode 'postgres://a/b?x=1') -eq 'postgres://a/b?x=1&sslmode=require') 'case6b: append sslmode with &' (Add-SslMode 'postgres://a/b?x=1')
    Assert-True ((Add-SslMode 'postgres://a/b?sslmode=require') -eq 'postgres://a/b?sslmode=require') 'case6c: keep existing sslmode' (Add-SslMode 'postgres://a/b?sslmode=require')

    # ---- Case 7: restore with fake psql ---------------------------------------
    $dir7 = Join-Path $script:TmpRoot 'restore'
    [System.IO.Directory]::CreateDirectory($dir7) | Out-Null
    $sourceSql = Join-Path $script:TmpRoot 'original.sql'
    [System.IO.File]::WriteAllBytes($sourceSql, $script:ExpectedBytes)
    $backupGz = Join-Path $dir7 'queueticket-20260101-010101.sql.gz'
    New-GzipFile -SourcePath $sourceSql -DestinationPath $backupGz
    $capture = Join-Path $script:TmpRoot 'captured.sql'
    $rc7 = Invoke-ScriptWithEnv -ScriptPath $RestorePs1 -Environment @{
        DATABASE_URL = $script:TestCaseUrl; BACKUP_DIR = $dir7; PSQL = $fakePsql
        FAKE_PSQL_CAPTURE = $capture
    } -Arguments @('-File', $backupGz, '-Force')
    $capturedOk = $false
    if (Test-Path -LiteralPath $capture -PathType Leaf) {
        $capturedOk = ([Convert]::ToBase64String([System.IO.File]::ReadAllBytes($capture)) -ceq
            [Convert]::ToBase64String($script:ExpectedBytes))
    }
    $tempLeftovers = @(Get-ChildItem -LiteralPath $env:TEMP -File -Filter 'queueticket-restore-*.sql' -ErrorAction SilentlyContinue)
    $tempErrLeftovers = @(Get-ChildItem -LiteralPath $env:TEMP -File -Filter 'queueticket-restore-*.err' -ErrorAction SilentlyContinue)
    Assert-True ($rc7 -eq 0 -and $capturedOk -and $tempLeftovers.Count -eq 0 -and $tempErrLeftovers.Count -eq 0) 'case7: restore passes exact SQL to psql and cleans up' ("rc=$rc7 captured=$capturedOk temp=$($tempLeftovers.Count)/$($tempErrLeftovers.Count)")
} catch {
    Record-Fail 'unexpected exception' $_.Exception.Message
} finally {
    if (Test-Path -LiteralPath $script:TmpRoot -PathType Container) {
        Remove-Item -LiteralPath $script:TmpRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ("backup windows tests: {0} passed, {1} failed" -f $script:PassCount, $script:FailCount)
if ($script:FailCount -gt 0) { exit 1 }
exit 0
