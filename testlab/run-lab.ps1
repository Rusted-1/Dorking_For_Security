# Dorker offline test: runs the real dorker.exe against the fake sites and the
# fake Serper on localhost, then checks what it did. No internet, no credits.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File testlab\run-lab.ps1 [-Exe path\to\dorker.exe]
#
# Exit code 0 = every check passed.
param([string]$Exe)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lab-common.ps1')

$Exe = Find-Dorker $Exe
$Out = Reset-LabOut
$Cfg = Join-Path $Out 'lab-config.toml'
$Coord = Join-Path $Out 'coord'
$script:Failed = 0
$script:Passed = 0

function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
    if ($Ok) { $script:Passed++; Write-Host ("  PASS  " + $Name) -ForegroundColor Green }
    else { $script:Failed++; Write-Host ("  FAIL  " + $Name + $(if ($Detail) { "  ($Detail)" })) -ForegroundColor Red }
}

function Section([string]$Title) { Write-Host ''; Write-Host "== $Title" -ForegroundColor Cyan }

Write-Host "Dorker offline test lab"
Write-Host "  dorker: $Exe"
Write-Host "  output: $Out"
$Server = Start-LabServer
try {
    # ---- Round 1: discover + scan everything --------------------------------
    Section 'Round 1: dorker auto (fake Serper -> fake sites)'
    $r1 = Invoke-Dorker $Exe @('--color', 'never', '--coordinate', $Coord, 'auto', '--config', $Cfg, '--max', '20')
    $r1.Output | Set-Content (Join-Path $Out 'round1.log') -Encoding UTF8
    Write-Host ("  finished in {0:N1}s (exit {1}); full output in testlab\out\round1.log" -f $r1.Seconds, $r1.Code)
    Check 'auto exits cleanly' ($r1.Code -eq 0) "exit $($r1.Code)"

    $req = Get-LabRequests
    $searches1 = @($req | Where-Object { $_ -like '8600 POST /search*' })
    Check 'Serper credit cap: at most 3 searches (max_queries_per_run = 3)' ($searches1.Count -ge 1 -and $searches1.Count -le 3) "$($searches1.Count) searches"
    Check 'keeps 7 new sites: drops the duplicate and the skip_domains result' ($r1.Output -match 'Kept 7 new site\(s\)') 'see round1.log'

    $disc = @(Get-Content (Join-Path $Out 'discovered.txt') -Encoding UTF8 | Where-Object { $_ -and -not $_.StartsWith('#') })
    Check 'nothing outside localhost was queued' (@($disc | Where-Object { $_ -notlike 'http://localhost:*' }).Count -eq 0) ($disc -join ', ')

    $a = Get-SiteReport 8601
    $aTop = Get-TopScore $a
    $aIds = Get-HitIds $a
    Check 'scam template site is flagged (score 50+)' ($aTop -ge 50) "score $aTop"
    Check 'phrase split across tags and lines matches (investment-platform)' ($aIds -contains 'investment-platform')
    Check "straight-quote page matches curly-quote keyword (Tesla's, ind-0024)" ($aIds -contains 'ind-0024')
    Check 'URL keyword matches /user/register' ($aIds -contains 'url-user-register')
    Check 'image link not downloaded (/logo.png)' (@($req | Where-Object { $_ -like '8601 GET /logo.png*' }).Count -eq 0)
    Check 'PDF link not downloaded (/docs/terms.pdf)' (@($req | Where-Object { $_ -like '8601 GET /docs/terms.pdf*' }).Count -eq 0)

    $b = Get-SiteReport 8602
    Check 'bank site is excluded as legitimate' ($b -and @($b.results | Where-Object { $_.excluded }).Count -ge 1 -and (Get-TopScore $b) -eq 0)

    $c = Get-SiteReport 8603
    Check 'garden blog has no hits' ($c -and (Get-HitIds $c).Count -eq 0) ("hits: " + ((Get-HitIds $c) -join ', '))

    Check 'robots.txt respected: /private/offer.html never fetched' (@($req | Where-Object { $_ -like '8604 GET /private*' }).Count -eq 0)
    $robotsHits = @($req | Where-Object { $_ -like '* GET /robots.txt*' } | ForEach-Object { ($_ -split ' ')[0] } | Group-Object | Where-Object { $_.Count -gt 1 })
    Check 'robots.txt fetched at most once per site' ($robotsHits.Count -eq 0) (($robotsHits | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', ')

    $e = Get-SiteReport 8605
    $eTop = Get-TopScore $e
    Check 'generic finance words score low (1-49)' ($eTop -ge 1 -and $eTop -le 49) "score $eTop"

    $z = Get-SiteReport 8606
    Check 'repaired Chinese keyword matches (ind-2062)' ((Get-HitIds $z) -contains 'ind-2062') ("hits: " + ((Get-HitIds $z) -join ', '))

    $dead = Get-SiteReport 8607
    Check 'dead site is reported as an error, not a crash' ($dead -and @($dead.results | Where-Object { $_.error }).Count -ge 1)

    # ---- Round 2: nothing new, fresh queries --------------------------------
    Section 'Round 2: run auto again'
    $before = (Get-LabRequests).Count
    $r2 = Invoke-Dorker $Exe @('--color', 'never', '--coordinate', $Coord, 'auto', '--config', $Cfg, '--max', '20')
    $r2.Output | Set-Content (Join-Path $Out 'round2.log') -Encoding UTF8
    $req2 = @(Get-LabRequests | Select-Object -Skip $before)
    $searches2 = @($req2 | Where-Object { $_ -like '8600 POST /search*' })
    Check 'already-scanned sites are skipped (Kept 0 new)' ($r2.Output -match 'Kept 0 new site\(s\)') 'see round2.log'
    Check 'no site pages fetched again' (@($req2 | Where-Object { $_ -notlike '8600 *' }).Count -eq 0) "$(@($req2 | Where-Object { $_ -notlike '8600 *' }).Count) requests"
    $q1 = @($searches1 | ForEach-Object { $_ -replace '^8600 POST /search q=', '' })
    $q2 = @($searches2 | ForEach-Object { $_ -replace '^8600 POST /search q=', '' })
    Check 'queries rotate: round 2 uses different searches' ($q2.Count -ge 1 -and @($q2 | Where-Object { $q1 -contains $_ }).Count -eq 0)

    # ---- Control panel (HTTP level) -----------------------------------------
    Section 'Control panel: dorker serve on port 8797'
    $panelArgs = @('--color', 'never', '--coordinate', $Coord, 'serve', '--config', $Cfg, '--port', '8797')
    $panel = Start-Process -FilePath $Exe -ArgumentList (($panelArgs | ForEach-Object { Quote-Arg $_ }) -join ' ') `
        -WorkingDirectory $RepoRoot -PassThru -WindowStyle Hidden
    $status = $null
    for ($i = 0; $i -lt 40 -and -not $status; $i++) {
        try { $status = (Invoke-WebRequest 'http://localhost:8797/api/status' -UseBasicParsing -TimeoutSec 2).Content | ConvertFrom-Json } catch { Start-Sleep -Milliseconds 250 }
    }
    Check 'panel starts and serves status' ($null -ne $status)
    if ($status) {
        Check 'panel lists the flagged site' (@($status.hits | Where-Object { $_.host -eq 'localhost:8601' }).Count -eq 1)
        Check 'panel counts 1 site awaiting review' ($status.awaiting -eq 1) "awaiting $($status.awaiting)"
        $blocked = $false
        try { Invoke-WebRequest 'http://localhost:8797/api/stop' -Method Post -UseBasicParsing -TimeoutSec 3 | Out-Null }
        catch { $blocked = ($_.Exception.Response.StatusCode.value__ -eq 403) }
        Check 'panel rejects an action without its header (other websites blocked)' $blocked
        $H = @{ 'X-Dorker-Panel' = '1' }
        $rv = (Invoke-WebRequest 'http://localhost:8797/api/review' -Method Post -Headers $H -UseBasicParsing `
            -Body '{"host":"localhost:8605","verdict":"false_positive"}').Content | ConvertFrom-Json
        Check 'panel saves a review' ($rv.ok -eq $true)
        $s2 = (Invoke-WebRequest 'http://localhost:8797/api/status' -UseBasicParsing).Content | ConvertFrom-Json
        Check 'false positive disappears from the hit list' (@($s2.hits | Where-Object { $_.host -eq 'localhost:8605' }).Count -eq 0)
        $log = (Invoke-WebRequest 'http://localhost:8797/api/log' -UseBasicParsing).StatusCode
        Check 'activity log endpoint answers' ($log -eq 200)
        try { Invoke-WebRequest 'http://localhost:8797/api/quit' -Method Post -Headers $H -UseBasicParsing -TimeoutSec 3 | Out-Null } catch {}
    }
    if (-not $panel.WaitForExit(5000)) { Stop-Process -Id $panel.Id -Force -ErrorAction SilentlyContinue }

    # ---- Review command -----------------------------------------------------
    Section 'Review: dorker review'
    $rl = Invoke-Dorker $Exe @('--color', 'never', 'review', '--config', $Cfg)
    Check 'review lists the flagged site' ($rl.Output -match 'localhost:8601')
    $rc = Invoke-Dorker $Exe @('--color', 'never', 'review', 'localhost:8601', 'confirmed', '--config', $Cfg)
    Check 'review records a verdict' ($rc.Code -eq 0 -and $rc.Output -match 'Marked localhost:8601 as confirmed')
    $rl2 = Invoke-Dorker $Exe @('--color', 'never', 'review', '--config', $Cfg)
    Check 'nothing left awaiting review' ($rl2.Output -match 'Nothing awaiting review')
} finally {
    Stop-LabServer $Server
}

Write-Host ''
$total = $script:Passed + $script:Failed
if ($script:Failed -eq 0) {
    Write-Host "All $total checks passed. Every request is in testlab\out\requests.log." -ForegroundColor Green
    exit 0
}
Write-Host "$($script:Failed) of $total checks failed. Logs are in testlab\out\." -ForegroundColor Red
exit 1
