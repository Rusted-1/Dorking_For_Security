# Shared helpers for the Dorker test lab scripts (dot-source this file).

$LabDir = $PSScriptRoot
$RepoRoot = Split-Path $LabDir -Parent
$LabOut = Join-Path $LabDir 'out'

# Find dorker.exe: an explicit path, the kit/repo root, or a cargo build.
function Find-Dorker([string]$Exe) {
    $candidates = @()
    if ($Exe) { $candidates += $Exe }
    $candidates += (Join-Path $RepoRoot 'dorker.exe')
    $candidates += (Join-Path $RepoRoot 'target\release\dorker.exe')
    foreach ($c in $candidates) {
        if (Test-Path $c -PathType Leaf) { return (Resolve-Path $c).Path }
    }
    throw "dorker.exe not found. Put it next to keywords.json (the folder above testlab), or pass -Exe <path>."
}

# Wipe testlab/out and drop a fresh copy of the lab config into it, with the
# placeholder key the fake Serper expects (the template keeps it empty so the
# repo's secret scanners don't flag it).
function Reset-LabOut {
    if (Test-Path $LabOut) { Remove-Item $LabOut -Recurse -Force }
    New-Item -ItemType Directory -Force $LabOut | Out-Null
    $cfg = Get-Content (Join-Path $LabDir 'lab-config.toml') -Raw
    $key = 'lab-fake-key'
    $cfg = $cfg -replace '(?m)^api_key = ""', "api_key = `"$key`""
    Set-Content (Join-Path $LabOut 'lab-config.toml') $cfg -Encoding ASCII -NoNewline
    return $LabOut
}

# Start the fake sites + fake Serper and wait until they answer.
function Start-LabServer([switch]$Visible) {
    $script = Join-Path $LabDir 'lab-server.ps1'
    $style = 'Hidden'
    if ($Visible) { $style = 'Normal' }
    $proc = Start-Process powershell -PassThru -WindowStyle $style -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$script`"", '-Out', "`"$LabOut`"")
    for ($i = 0; $i -lt 40; $i++) {
        try {
            $r = Invoke-WebRequest 'http://localhost:8600/__health' -UseBasicParsing -TimeoutSec 2
            if ($r.Content -eq 'ok') { return $proc }
        } catch {}
        Start-Sleep -Milliseconds 250
    }
    throw 'The lab server did not start (ports 8600-8606 may be in use).'
}

function Stop-LabServer($Proc) {
    try { Invoke-WebRequest 'http://localhost:8600/__stop' -UseBasicParsing -TimeoutSec 3 | Out-Null } catch {}
    if ($Proc -and -not $Proc.HasExited) {
        if (-not $Proc.WaitForExit(3000)) { Stop-Process -Id $Proc.Id -Force -ErrorAction SilentlyContinue }
    }
}

# Quote one argument for a Windows command line.
function Quote-Arg([string]$A) {
    if ($A -match '[\s"]') { return '"' + ($A -replace '"', '\"') + '"' }
    return $A
}

# Run dorker from the repo root; returns @{ Code; Output; Seconds }.
function Invoke-Dorker([string]$Exe, [string[]]$DorkerArgs) {
    $stdout = [System.IO.Path]::GetTempFileName()
    $stderr = [System.IO.Path]::GetTempFileName()
    $line = ($DorkerArgs | ForEach-Object { Quote-Arg $_ }) -join ' '
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $Exe -ArgumentList $line -WorkingDirectory $RepoRoot `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $sw.Stop()
    $text = (Get-Content $stdout -Raw -Encoding UTF8) + "`n" + (Get-Content $stderr -Raw -Encoding UTF8)
    Remove-Item $stdout, $stderr -ErrorAction SilentlyContinue
    return @{ Code = $p.ExitCode; Output = $text; Seconds = $sw.Elapsed.TotalSeconds }
}

# Lines of the request log (port, method, path, note).
function Get-LabRequests {
    $log = Join-Path $LabOut 'requests.log'
    if (-not (Test-Path $log)) { return @() }
    return @(Get-Content $log -Encoding UTF8)
}

# A per-site report written by a worker, or $null.
function Get-SiteReport([int]$Port, [string]$OutDir = $LabOut) {
    $f = Join-Path $OutDir "sites\localhost_$Port\report.json"
    if (-not (Test-Path $f)) { return $null }
    return (Get-Content $f -Raw -Encoding UTF8 | ConvertFrom-Json)
}

function Get-TopScore($Report) {
    if (-not $Report) { return $null }
    $scores = @($Report.results | Where-Object { -not $_.excluded } | ForEach-Object { [int]$_.score })
    if ($scores.Count -eq 0) { return 0 }
    return ($scores | Measure-Object -Maximum).Maximum
}

function Get-HitIds($Report) {
    if (-not $Report) { return @() }
    return @($Report.results | ForEach-Object { $_.hits } | ForEach-Object { $_.keyword_id } | Sort-Object -Unique)
}
