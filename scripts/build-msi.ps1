# Builds the Halo MSI with WiX Toolset 3.14.
# Usage:  .\scripts\build-msi.ps1 [-Version 0.1.0] [-Wix "C:\path\to\wix"] [-SkipBuild]
# Output: dist\halo-<version>-x64.msi
param(
  [string]$Version = "",
  [string]$Wix = "",
  [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $Version) {
  $Version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
}
if (-not $Wix) {
  $candidates = @($env:WIX, "$env:LOCALAPPDATA\Programs\halo-toolchain\wix", "${env:ProgramFiles(x86)}\WiX Toolset v3.14\bin")
  $Wix = $candidates | Where-Object { $_ -and (Test-Path (Join-Path $_ 'candle.exe')) -or ($_ -and (Test-Path (Join-Path $_ 'bin\candle.exe'))) } | Select-Object -First 1
  if ($Wix -and (Test-Path (Join-Path $Wix 'bin\candle.exe'))) { $Wix = Join-Path $Wix 'bin' }
}
if (-not $Wix) { throw "WiX 3.14 not found. Install it or pass -Wix <folder containing candle.exe>." }

if (-not $SkipBuild) {
  cargo build --release -p halo-app
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$obj = Join-Path $root "target\wix"
$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $obj, $dist | Out-Null

# WinDivert (LGPL-3.0, https://github.com/basil00/WinDivert) powers the optional
# per-app network limiter. Fetched from the official release, pinned by SHA-256.
$wdVersion = "2.2.2"
$wdSha256 = "63CB41763BB4B20F600B6DE04E991A9C2BE73279E317D4D82F237B150C5F3F15"
$wdDir = Join-Path $root "target\windivert"
if (-not (Test-Path "$wdDir\WinDivert64.sys")) {
  New-Item -ItemType Directory -Force $wdDir | Out-Null
  $zip = Join-Path $wdDir "windivert.zip"
  $ProgressPreference = 'SilentlyContinue'
  Invoke-WebRequest "https://github.com/basil00/WinDivert/releases/download/v$wdVersion/WinDivert-$wdVersion-A.zip" -OutFile $zip -UseBasicParsing
  if ((Get-FileHash $zip -Algorithm SHA256).Hash -ne $wdSha256) { Remove-Item $zip; throw "WinDivert archive checksum mismatch" }
  Expand-Archive $zip "$wdDir\x" -Force
  $src = "$wdDir\x\WinDivert-$wdVersion-A"
  Copy-Item "$src\x64\WinDivert.dll", "$src\x64\WinDivert64.sys" $wdDir
  Copy-Item "$src\LICENSE" "$wdDir\LICENSE-WinDivert.txt"
  Remove-Item -Recurse -Force "$wdDir\x", $zip
}
# Also next to the release binary, so `cargo run --release` finds it.
Copy-Item "$wdDir\WinDivert.dll", "$wdDir\WinDivert64.sys" (Join-Path $root "target\release") -Force

& (Join-Path $Wix 'candle.exe') -nologo -arch x64 "-dVersion=$Version" "-dBinDir=$root\target\release" "-dRoot=$root" "-dWinDivertDir=$wdDir" `
  -ext WixUIExtension -ext WixUtilExtension -out "$obj\halo.wixobj" "$root\wix\halo.wxs"
if ($LASTEXITCODE -ne 0) { throw "candle failed" }

$msi = Join-Path $dist "halo-$Version-x64.msi"
& (Join-Path $Wix 'light.exe') -nologo -ext WixUIExtension -ext WixUtilExtension -cultures:en-us `
  -out $msi "$obj\halo.wixobj"
if ($LASTEXITCODE -ne 0) { throw "light failed" }

$hash = (Get-FileHash $msi -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $msi -Leaf)" | Out-File -Encoding ascii "$msi.sha256"
"Built $msi ($([math]::Round((Get-Item $msi).Length / 1MB, 1)) MB)"
"SHA256 $hash"
