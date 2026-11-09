# Dorker offline test lab: serves the fake sites in testlab/sites and a fake
# Serper search API, on localhost only. Nothing here touches the internet.
#
#   port 8600       fake Serper (POST /search), plus /__health and /__stop
#   ports 8601-8606 one fake website each (see $Sites)
#
# Every request is printed and appended to <Out>/requests.log, so you can see
# exactly what Dorker fetched.
param([Parameter(Mandatory = $true)][string]$Out)

$ErrorActionPreference = 'Stop'
$Lab = $PSScriptRoot

$Sites = @{
    8601 = 'scam-a'        # investment-scam template (should be flagged)
    8602 = 'legit-bank'    # mentions an exclusion term (should be excluded)
    8603 = 'garden-blog'   # nothing suspicious (should score 0)
    8604 = 'robots-site'   # robots.txt blocks /private (must never be fetched)
    8605 = 'weak-signals'  # a few generic finance words (low score)
    8606 = 'scam-zh'       # Chinese phrases repaired in keywords.json
}

# What the fake Serper returns for every query. 8607 has no server (a dead
# site); skipme.invalid is filtered by skip_domains and .invalid can never
# resolve, so even a filtering bug could not reach the internet.
$Results = @(
    'http://localhost:8601/',
    'http://localhost:8601/about.html',
    'http://localhost:8602/',
    'http://localhost:8603/',
    'http://localhost:8604/',
    'http://localhost:8605/',
    'http://localhost:8606/',
    'http://localhost:8607/',
    'http://news.skipme.invalid/scam-warning'
)

$Types = @{
    '.html' = 'text/html; charset=utf-8'
    '.txt'  = 'text/plain; charset=utf-8'
    '.png'  = 'image/png'
    '.pdf'  = 'application/pdf'
}

New-Item -ItemType Directory -Force $Out | Out-Null
$Log = Join-Path $Out 'requests.log'
$Utf8 = New-Object System.Text.UTF8Encoding($false)

function Send-Reply($Res, [int]$Code, [string]$Type, [byte[]]$Bytes) {
    $Res.StatusCode = $Code
    $Res.ContentType = $Type
    $Res.ContentLength64 = $Bytes.Length
    $Res.OutputStream.Write($Bytes, 0, $Bytes.Length)
    $Res.Close()
}

function Send-Text($Res, [int]$Code, [string]$Type, [string]$Text) {
    Send-Reply $Res $Code $Type $Utf8.GetBytes($Text)
}

$Listener = New-Object System.Net.HttpListener
$Listener.Prefixes.Add('http://localhost:8600/')
foreach ($p in $Sites.Keys) { $Listener.Prefixes.Add("http://localhost:$p/") }
$Listener.Start()
Write-Host 'Dorker test lab is running on localhost (ports 8600-8606).'
Write-Host 'Fake Serper: http://localhost:8600/search   Requests are listed below.'
Write-Host ''

while ($Listener.IsListening) {
    try { $Ctx = $Listener.GetContext() } catch { break }
    $Req = $Ctx.Request
    $Res = $Ctx.Response
    $Port = $Req.Url.Port
    $Method = $Req.HttpMethod
    $Path = [Uri]::UnescapeDataString($Req.Url.AbsolutePath)
    $Note = ''
    try {
        if ($Port -eq 8600) {
            switch ($Path) {
                '/__health' { Send-Text $Res 200 'text/plain' 'ok' }
                '/__stop' { Send-Text $Res 200 'text/plain' 'stopping'; $Listener.Stop() }
                '/search' {
                    $Body = (New-Object System.IO.StreamReader($Req.InputStream, $Utf8)).ReadToEnd()
                    $Q = ''
                    try { $Q = ($Body | ConvertFrom-Json).q } catch {}
                    $Note = "q=$Q"
                    if (-not $Req.Headers['X-API-KEY']) {
                        Send-Text $Res 401 'application/json' '{"message":"Unauthorized."}'
                    } else {
                        $Organic = @()
                        $i = 1
                        foreach ($u in $Results) {
                            $Organic += [ordered]@{ title = "Lab result $i"; link = $u; position = $i }
                            $i++
                        }
                        $Json = [ordered]@{ searchParameters = @{ q = $Q }; organic = $Organic } | ConvertTo-Json -Depth 5
                        Send-Text $Res 200 'application/json' $Json
                    }
                }
                default { Send-Text $Res 404 'text/plain' 'not found' }
            }
        } else {
            $Root = Join-Path (Join-Path $Lab 'sites') $Sites[$Port]
            $Rel = $Path.TrimStart('/')
            if ($Rel -match '\.\.') {
                Send-Text $Res 400 'text/plain' 'bad path'
            } else {
                $File = Join-Path $Root $Rel
                if ($Path.EndsWith('/')) {
                    $File = Join-Path $File 'index.html'
                } elseif (-not (Test-Path $File -PathType Leaf) -and (Test-Path "$File.html" -PathType Leaf)) {
                    $File = "$File.html"
                }
                if (Test-Path $File -PathType Leaf) {
                    $Ext = [System.IO.Path]::GetExtension($File).ToLower()
                    $Type = $Types[$Ext]
                    if (-not $Type) { $Type = 'application/octet-stream' }
                    Send-Reply $Res 200 $Type ([System.IO.File]::ReadAllBytes($File))
                } else {
                    Send-Text $Res 404 'text/plain' 'not found'
                }
            }
        }
    } catch {
        try { $Res.StatusCode = 500; $Res.Close() } catch {}
    }
    $Line = ("{0} {1} {2} {3}" -f $Port, $Method, $Path, $Note).TrimEnd()
    [System.IO.File]::AppendAllText($Log, $Line + "`r`n", $Utf8)
    Write-Host $Line
}
