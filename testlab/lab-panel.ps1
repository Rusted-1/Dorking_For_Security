# Opens the Dorker control panel wired to the offline test lab, so you can
# click through it safely. Start/Discover use the fake Serper on localhost.
#
# A second window lists every request the fake sites receive. Close the
# panel with "Quit panel" (or close this window) to shut everything down.
param([string]$Exe, [int]$Port = 8787)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lab-common.ps1')

$Exe = Find-Dorker $Exe
$Out = Reset-LabOut
$Cfg = Join-Path $Out 'lab-config.toml'
$Coord = Join-Path $Out 'coord'

Write-Host 'Starting the test lab (fake sites + fake Serper on localhost)...'
$Server = Start-LabServer -Visible
try {
    Write-Host "Starting the control panel at http://localhost:$Port"
    $panelArgs = @('--coordinate', $Coord, 'serve', '--config', $Cfg, '--port', "$Port")
    $panel = Start-Process -FilePath $Exe -ArgumentList (($panelArgs | ForEach-Object { Quote-Arg $_ }) -join ' ') `
        -WorkingDirectory $RepoRoot -PassThru -NoNewWindow
    Start-Sleep -Seconds 1
    Start-Process "http://localhost:$Port"
    Write-Host ''
    Write-Host 'Test checklist: testlab\README.md (section "Click through the control panel").'
    Write-Host 'Leave this window open. Click "Quit panel" in the browser when you are done.'
    $panel.WaitForExit()
} finally {
    Stop-LabServer $Server
    Write-Host 'Lab stopped. Results and logs are in testlab\out\.'
}
