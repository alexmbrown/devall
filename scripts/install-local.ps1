<#
Builds Dev All from this checkout and installs it for the current user (Windows):
  %LOCALAPPDATA%\Programs\Dev All\Dev All.exe  plus a "Dev All" Start Menu shortcut.

Run it again after any change to rebuild and replace the installed copy:
  cd apps/desktop; npm run app:install

The UI is embedded in the exe (no dev server needed). If the installed copy is open it is asked to
close (like clicking X, so unsaved-changes prompts still apply) and reopened afterwards. The build
uses the `local` cargo profile (Cargo.toml): optimised, with fast incremental rebuilds. App data
(projects, history, to-dos) is shared with dev builds — same app identifier.
#>
param([switch]$NoLaunch)
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$desktop = Join-Path $root 'apps\desktop'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Dev All'
$exe = Join-Path $installDir 'Dev All.exe'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Dev All.lnk'

Write-Host '1/4  Building the UI...'
Push-Location $desktop
try {
  npm run build
  if ($LASTEXITCODE) { throw 'UI build failed' }
} finally { Pop-Location }

Write-Host '2/4  Building the app (cargo profile "local")...'
Push-Location $root
try {
  # tauri/custom-protocol embeds the built UI (what `tauri build` turns on).
  cargo build -p devall-desktop --profile local --features tauri/custom-protocol
  if ($LASTEXITCODE) { throw 'cargo build failed' }
} finally { Pop-Location }
$built = Join-Path $root 'target\local\devall-desktop.exe'

Write-Host '3/4  Installing...'
$running = @(Get-Process -Name 'Dev All' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe })
if ($running.Count) {
  Write-Host '     Closing the running copy...'
  foreach ($p in $running) { [void]$p.CloseMainWindow() }
  $running | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue
  if (@($running | Where-Object { -not $_.HasExited }).Count) {
    throw 'Dev All is still open (unsaved changes?). Close it and run this again.'
  }
}
New-Item -ItemType Directory -Force $installDir | Out-Null
Copy-Item $built $exe -Force

Write-Host '4/4  Start Menu shortcut...'
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.WorkingDirectory = $installDir
$link.IconLocation = "$exe,0"
$link.Description = 'Dev All - local-first developer toolkit'
$link.Save()

Write-Host "Installed $exe (built $((Get-Item $built).LastWriteTime))"
Write-Host "Start Menu: Dev All"
if ($running.Count -and -not $NoLaunch) { Start-Process $exe }
