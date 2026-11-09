# Compares two dorker builds (for example the current release and the test
# branch) on the offline lab sites: scores per site, requests made, and time.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File testlab\compare.ps1 -Old testlab\old\dorker.exe [-New dorker.exe]
#
# Both builds run `scan --url` on each fake site with the same settings, so
# the old build needs no new features. Nothing leaves localhost.
param([Parameter(Mandatory = $true)][string]$Old, [string]$New, [string]$Summary)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lab-common.ps1')

$Old = (Resolve-Path $Old).Path
$New = Find-Dorker $New
$Out = Reset-LabOut
$Ports = 8601..8606

function Run-Build([string]$Label, [string]$Exe) {
    $scores = @{}
    $before = (Get-LabRequests).Count
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    foreach ($p in $Ports) {
        $dir = "testlab/out/cmp-$Label/$p"
        $cfg = Join-Path $Out "cmp-$Label-$p.toml"
        @(
            'keywords_file = "keywords.json"'
            "output_dir = `"$dir`""
            '[fetch]'
            'delay_ms = 150'
            'timeout_secs = 10'
            'user_agent = "Dorker/0.1 (+offline test lab)"'
            'respect_robots = true'
            '[source]'
            'kind = "url_list"'
            'path = "candidates.txt"'
        ) | Set-Content $cfg -Encoding ASCII
        $r = Invoke-Dorker $Exe @('--color', 'never', '--solo', 'scan', '--url', "http://localhost:$p/", '--config', $cfg, '--max', '20')
        $rep = $null
        $f = Join-Path $RepoRoot ($dir + '/report.json')
        if (Test-Path $f) { $rep = Get-Content $f -Raw -Encoding UTF8 | ConvertFrom-Json }
        $scores[$p] = Get-TopScore $rep
    }
    $sw.Stop()
    $reqs = (Get-LabRequests).Count - $before
    return @{ Scores = $scores; Seconds = $sw.Elapsed.TotalSeconds; Requests = $reqs }
}

$Names = @{ 8601 = 'scam template'; 8602 = 'bank (excluded)'; 8603 = 'garden blog'; 8604 = 'robots-blocked'; 8605 = 'generic words'; 8606 = 'Chinese phrases' }

$Server = Start-LabServer
try {
    Write-Host "Old build: $Old"
    $o = Run-Build 'old' $Old
    Write-Host "New build: $New"
    $n = Run-Build 'new' $New
} finally {
    Stop-LabServer $Server
}

$lines = @()
$lines += '| Site | Old score | New score |'
$lines += '|---|---:|---:|'
foreach ($p in $Ports) {
    $lines += ('| {0} ({1}) | {2} | {3} |' -f $p, $Names[$p], $o.Scores[$p], $n.Scores[$p])
}
$lines += ''
$lines += ('Requests to the fake sites: old {0}, new {1}.' -f $o.Requests, $n.Requests)
$lines += ('Total time: old {0:N1}s, new {1:N1}s (6 sites, 150 ms politeness delay per page).' -f $o.Seconds, $n.Seconds)
$lines | ForEach-Object { Write-Host $_ }
if ($Summary) { $lines | Add-Content $Summary -Encoding UTF8 }
