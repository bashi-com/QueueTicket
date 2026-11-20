#Requires -Version 5.1
# QueueTicket Windows backup/restore - shared helpers.
# Encoding: UTF-8 with BOM / CRLF.

$script:BackupEnvPath = Join-Path $PSScriptRoot 'backup.env'

# Read KEY="value" / KEY=value pairs. Blank lines and '#'-only lines are ignored.
function Read-BackupEnvFile {
    $values = @{}
    if (-not (Test-Path -LiteralPath $script:BackupEnvPath -PathType Leaf)) {
        return $values
    }
    foreach ($line in [System.IO.File]::ReadAllLines($script:BackupEnvPath)) {
        $text = $line.Trim()
        if ([string]::IsNullOrEmpty($text) -or $text.StartsWith('#')) { continue }
        if ($text -notmatch '^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') { continue }
        $key = $Matches[1]
        $value = $Matches[2].Trim()
        if ($value.Length -ge 2) {
            $first = $value[0]
            $last = $value[$value.Length - 1]
            if (($first -eq '"' -and $last -eq '"') -or ($first -eq "'" -and $last -eq "'")) {
                $value = $value.Substring(1, $value.Length - 2)
            }
        }
        $values[$key] = $value
    }
    return $values
}

# Environment variables win over backup.env (needed for tests and overrides).
function Get-BackupSetting {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [string]$Default = '',
        [hashtable]$FileValues
    )
    $value = [Environment]::GetEnvironmentVariable($Name)
    if (-not [string]::IsNullOrWhiteSpace($value)) { return $value }
    if ($null -ne $FileValues -and $FileValues.ContainsKey($Name)) {
        $fromFile = [string]$FileValues[$Name]
        if (-not [string]::IsNullOrWhiteSpace($fromFile)) { return $fromFile }
    }
    return $Default
}

function Test-PositiveInteger {
    param([string]$Value)
    return ($Value -match '^[1-9][0-9]*$')
}

# Render requires SSL. Append sslmode=require unless it is already present.
function Add-SslMode {
    param([string]$Url)
    if ([string]::IsNullOrWhiteSpace($Url)) { return $Url }
    if ($Url -match 'sslmode=') { return $Url }
    if ($Url.Contains('?')) { return $Url + '&sslmode=require' }
    return $Url + '?sslmode=require'
}

# Save/update DATABASE_URL in backup.env without touching the other keys.
function Save-DatabaseUrl {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$Path
    )
    $lines = @()
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        $lines = [System.Collections.Generic.List[string]]::new()
        foreach ($line in [System.IO.File]::ReadAllLines($Path)) { $lines.Add($line) }
    } else {
        $lines = [System.Collections.Generic.List[string]]::new()
    }
    $replaced = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*DATABASE_URL\s*=') {
            $lines[$i] = 'DATABASE_URL="' + $Url + '"'
            $replaced = $true
        }
    }
    if (-not $replaced) {
        $lines.Add('DATABASE_URL="' + $Url + '"')
    }
    [System.IO.File]::WriteAllLines($Path, $lines, (New-Object System.Text.UTF8Encoding($false)))
}

function Get-JstNow {
    $zone = [TimeZoneInfo]::FindSystemTimeZoneById('Tokyo Standard Time')
    return [TimeZoneInfo]::ConvertTime([DateTime]::UtcNow, $zone)
}

function Read-TextFileAuto {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return '' }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -eq 0) { return '' }
    if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) {
        return [System.Text.Encoding]::Unicode.GetString($bytes, 2, $bytes.Length - 2)
    }
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        return (New-Object System.Text.UTF8Encoding($false)).GetString($bytes, 3, $bytes.Length - 3)
    }
    return [System.Text.Encoding]::UTF8.GetString($bytes)
}

# Message + console + UTF-8 log file (one line per call).
function Write-BackupLog {
    param(
        [Parameter(Mandatory = $true)][string]$BackupDir,
        [Parameter(Mandatory = $true)][string]$Message,
        [switch]$IsError
    )
    $line = '[' + (Get-JstNow).ToString('yyyy-MM-dd HH:mm:ss') + '] ' + $Message
    if ($IsError) { Write-Host $line -ForegroundColor Red } else { Write-Host $line }
    try {
        if (-not (Test-Path -LiteralPath $BackupDir -PathType Container)) {
            [System.IO.Directory]::CreateDirectory($BackupDir) | Out-Null
        }
        $logPath = Join-Path $BackupDir 'backup.log'
        [System.IO.File]::AppendAllText($logPath, $line + "`r`n", (New-Object System.Text.UTF8Encoding($false)))
    } catch {
        # Never fail a backup just because the log could not be written.
    }
}

# Detection order: explicit path -> bundled pgsql\bin -> installed versions -> PATH.
function Get-ToolExecutable {
    param(
        [string]$Configured = '',
        [Parameter(Mandatory = $true)][ValidateSet('pg_dump', 'psql')][string]$ToolName
    )
    $fileName = $ToolName + '.exe'
    if (-not [string]::IsNullOrWhiteSpace($Configured)) {
        if (Test-Path -LiteralPath $Configured -PathType Leaf) {
            return (Resolve-Path -LiteralPath $Configured).Path
        }
        return $null
    }
    $bundled = Join-Path $PSScriptRoot ('pgsql\bin\' + $fileName)
    if (Test-Path -LiteralPath $bundled -PathType Leaf) { return $bundled }

    $installRoot = 'C:\Program Files\PostgreSQL'
    if (Test-Path -LiteralPath $installRoot -PathType Container) {
        $best = $null
        $bestMajor = -1
        foreach ($dir in (Get-ChildItem -LiteralPath $installRoot -Directory -ErrorAction SilentlyContinue)) {
            if ($dir.Name -match '^(\d+)') {
                $major = [int]$Matches[1]
                $candidate = Join-Path $dir.FullName ('bin\' + $fileName)
                if ($major -gt $bestMajor -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                    $bestMajor = $major
                    $best = $candidate
                }
            }
        }
        if ($best) { return $best }
    }

    $found = Get-Command $ToolName -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($found) { return $found.Source }
    return $null
}

function Get-ToolNotFoundMessage {
    param([Parameter(Mandatory = $true)][ValidateSet('pg_dump', 'psql')][string]$ToolName)
    $folder = Join-Path $PSScriptRoot 'pgsql\bin'
    if ($ToolName -eq 'psql') {
        return "psql.exe が見つかりません。PostgreSQL 公式 zip を展開して $folder に置くか、C:\Program Files\PostgreSQL\<バージョン>\bin にインストールするか、PSQL にフルパスを設定してください。"
    }
    return "pg_dump.exe が見つかりません。PostgreSQL 公式 zip を展開して $folder に置くか（DB 以上のバージョンが必要です）、C:\Program Files\PostgreSQL\<バージョン>\bin にインストールするか、PG_DUMP にフルパスを設定してください。"
}

function Format-BackupSize {
    param([long]$Bytes)
    if ($Bytes -lt 1024) { return $Bytes.ToString() + ' B' }
    if ($Bytes -lt 1048576) {
        return [string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0:N1} KB', ($Bytes / 1024.0))
    }
    return [string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0:N1} MB', ($Bytes / 1048576.0))
}

# GZipStream based compression/decompression (no external gzip needed).
function New-GzipFile {
    param(
        [Parameter(Mandatory = $true)][string]$SourcePath,
        [Parameter(Mandatory = $true)][string]$DestinationPath
    )
    $input = [System.IO.File]::OpenRead($SourcePath)
    try {
        $output = [System.IO.File]::Create($DestinationPath)
        try {
            $gzip = New-Object System.IO.Compression.GZipStream($output, [System.IO.Compression.CompressionMode]::Compress)
            try { $input.CopyTo($gzip) } finally { $gzip.Dispose() }
        } finally { $output.Dispose() }
    } finally { $input.Dispose() }
}

function Read-GzipBytes {
    param([Parameter(Mandatory = $true)][string]$Path)
    $memory = New-Object System.IO.MemoryStream
    try {
        $input = [System.IO.File]::OpenRead($Path)
        try {
            $gzip = New-Object System.IO.Compression.GZipStream($input, [System.IO.Compression.CompressionMode]::Decompress)
            try { $gzip.CopyTo($memory) } finally { $gzip.Dispose() }
        } finally { $input.Dispose() }
        return $memory.ToArray()
    } finally { $memory.Dispose() }
}

function Expand-GzipToFile {
    param(
        [Parameter(Mandatory = $true)][string]$GzipPath,
        [Parameter(Mandatory = $true)][string]$DestinationPath
    )
    [System.IO.File]::WriteAllBytes($DestinationPath, (Read-GzipBytes -Path $GzipPath))
}

# Fully reads (and therefore CRC-checks) the gzip stream and compares the bytes.
function Test-GzipMatchesSource {
    param(
        [Parameter(Mandatory = $true)][string]$GzipPath,
        [Parameter(Mandatory = $true)][string]$SourcePath
    )
    try {
        $expected = [System.IO.File]::ReadAllBytes($SourcePath)
        $actual = Read-GzipBytes -Path $GzipPath
        return ([Convert]::ToBase64String($expected) -ceq [Convert]::ToBase64String($actual))
    } catch {
        return $false
    }
}

# One backup attempt. Never exits; returns a result object so tests can call it.
function Invoke-BackupOnce {
    param(
        [Parameter(Mandatory = $true)][string]$DatabaseUrl,
        [Parameter(Mandatory = $true)][string]$BackupDir,
        [Parameter(Mandatory = $true)][int]$Keep,
        [Parameter(Mandatory = $true)][string]$PgDumpPath
    )
    $stamp = (Get-JstNow).ToString('yyyyMMdd-HHmmss')
    $fileName = 'queueticket-' + $stamp + '.sql.gz'
    $target = Join-Path $BackupDir $fileName
    $tmpGz = $target + '.tmp'
    $id = [guid]::NewGuid().ToString('N')
    $tmpSql = Join-Path $BackupDir ('.queueticket-' + $stamp + '-' + $id + '.sql')
    $stderrPath = Join-Path $BackupDir ('.pgdump-' + $stamp + '-' + $id + '.err')

    try {
        if (-not (Test-Path -LiteralPath $BackupDir -PathType Container)) {
            [System.IO.Directory]::CreateDirectory($BackupDir) | Out-Null
        }

        $savedEap = $ErrorActionPreference
        $exitCode = 1
        $detail = ''
        try {
            $ErrorActionPreference = 'Continue'
            $pgArgs = @('--no-owner', '--no-privileges', '--clean', '--if-exists',
                        '--file', ('"' + $tmpSql + '"'), ('"' + $DatabaseUrl + '"'))
            # stdout is intentionally NOT piped (PS 5.1 pipes corrupt encodings).
            & $PgDumpPath @pgArgs 2> $stderrPath
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

        if ($exitCode -ne 0) {
            if ($detail -match '(?i)version mismatch') {
                $detail = $detail + "`r`n" + 'DB より新しいバージョンの PostgreSQL を入れてください。'
            }
            return [pscustomobject]@{
                Success = $false
                Message = 'ERROR: pg_dump failed (exit ' + $exitCode + ').'
                Detail = $detail
                FileName = $fileName
                SizeText = ''
            }
        }
        if (-not (Test-Path -LiteralPath $tmpSql -PathType Leaf)) {
            return [pscustomobject]@{
                Success = $false
                Message = 'ERROR: pg_dump did not create the SQL file.'
                Detail = $detail
                FileName = $fileName
                SizeText = ''
            }
        }

        New-GzipFile -SourcePath $tmpSql -DestinationPath $tmpGz
        if (-not (Test-GzipMatchesSource -GzipPath $tmpGz -SourcePath $tmpSql)) {
            return [pscustomobject]@{
                Success = $false
                Message = 'ERROR: gzip integrity check failed.'
                Detail = $detail
                FileName = $fileName
                SizeText = ''
            }
        }
        Move-Item -LiteralPath $tmpGz -Destination $target -Force

        # Rotate only queueticket-*.sql.gz, newest name first.
        $existing = @(Get-ChildItem -LiteralPath $BackupDir -File -Filter 'queueticket-*.sql.gz' |
            Sort-Object Name -Descending)
        if ($existing.Count -gt $Keep) {
            $existing | Select-Object -Skip $Keep | Remove-Item -Force
        }

        $sizeText = Format-BackupSize -Bytes (Get-Item -LiteralPath $target).Length
        return [pscustomobject]@{
            Success = $true
            Message = 'OK: ' + $fileName + ' (' + $sizeText + ')'
            Detail = $detail
            FileName = $fileName
            SizeText = $sizeText
        }
    } catch {
        return [pscustomobject]@{
            Success = $false
            Message = 'ERROR: ' + $_.Exception.Message
            Detail = ''
            FileName = $fileName
            SizeText = ''
        }
    } finally {
        foreach ($path in @($tmpSql, $tmpGz, $stderrPath)) {
            if ($path -and (Test-Path -LiteralPath $path)) {
                Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
            }
        }
    }
}
