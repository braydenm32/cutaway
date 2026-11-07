# Builds Cutaway for Windows 11 (x64) into dist\Cutaway and a zip of it.
#
#   powershell -ExecutionPolicy Bypass -File windows\build.ps1
#   powershell -ExecutionPolicy Bypass -File windows\build.ps1 -NoAsio
#
# Needs, once:
#   - Rust (https://rustup.rs), with the default MSVC toolchain.
#   - Visual Studio 2022 Build Tools (or Visual Studio) with the
#     "Desktop development with C++" workload, which includes CMake.
#   - For ASIO interfaces: LLVM (winget install LLVM.LLVM). The ASIO SDK
#     itself is downloaded from Steinberg the first time.
# Downloaded the first time (into windows\deps): ONNX Runtime with DirectML
# (auto-framing and background removal) and the ASIO SDK.

param(
    [switch]$NoAsio,
    [switch]$NoWhisper
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$deps = Join-Path $PSScriptRoot "deps"
New-Item -ItemType Directory -Force -Path $deps | Out-Null
Set-Location $root

function Say($text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Warn($text) { Write-Host "    $text" -ForegroundColor Yellow }

# ---- tools ----------------------------------------------------------------------

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust isn't installed: get it from https://rustup.rs, then open a new terminal."
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere) -or -not (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)) {
    throw "The Visual Studio C++ build tools aren't installed: install 'Desktop development with C++' from https://visualstudio.microsoft.com/visual-cpp-build-tools/"
}
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath

$features = @()
if (-not $NoWhisper) {
    # whisper.cpp (live captions) is built with CMake; Visual Studio has one.
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        $vsCmake = Join-Path $vs "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
        if (Test-Path (Join-Path $vsCmake "cmake.exe")) {
            $env:PATH = "$vsCmake;$env:PATH"
        } else {
            throw "CMake isn't installed (needed for live captions): add 'C++ CMake tools for Windows' in the Visual Studio Installer, or run with -NoWhisper."
        }
    }
}

# ---- ASIO -------------------------------------------------------------------------

if (-not $NoAsio) {
    $asio = $env:CPAL_ASIO_DIR
    if (-not $asio -or -not (Test-Path $asio)) {
        $asio = Get-ChildItem -Path $deps -Directory -Filter "asiosdk*" -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $asio) {
        Say "Downloading Steinberg's ASIO SDK (by building with it you accept Steinberg's ASIO SDK license)"
        try {
            $zip = Join-Path $deps "asiosdk.zip"
            Invoke-WebRequest -Uri "https://www.steinberg.net/asiosdk" -OutFile $zip
            # Steinberg's link always gives its newest SDK, so there's no fixed
            # fingerprint; set CUTAWAY_ASIO_SHA256 to insist on one.
            $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLower()
            Write-Host "    ASIO SDK SHA-256 $got"
            if ($env:CUTAWAY_ASIO_SHA256 -and $got -ne $env:CUTAWAY_ASIO_SHA256.ToLower()) {
                Remove-Item $zip
                throw "The ASIO SDK isn't the expected one (SHA-256 $got)."
            }
            Expand-Archive -Path $zip -DestinationPath $deps -Force
            Remove-Item $zip
            $asio = Get-ChildItem -Path $deps -Directory -Filter "asiosdk*" | Select-Object -First 1 -ExpandProperty FullName
        } catch {
            Warn "Couldn't download the ASIO SDK ($_)."
        }
    }
    $clang = $env:LIBCLANG_PATH
    if (-not $clang) {
        foreach ($p in @("$env:ProgramFiles\LLVM\bin", "$vs\VC\Tools\Llvm\x64\bin")) {
            if (Test-Path (Join-Path $p "libclang.dll")) { $clang = $p; break }
        }
    }
    if ($asio -and $clang) {
        $env:CPAL_ASIO_DIR = $asio
        $env:LIBCLANG_PATH = $clang
        $features += "asio"
        Say "ASIO: $asio"
    } else {
        if (-not $asio) { Warn "No ASIO SDK: unzip it from https://www.steinberg.net/developers/ into windows\deps (a folder named asiosdk...)." }
        if (-not $clang) { Warn "No LLVM (libclang.dll): winget install LLVM.LLVM" }
        Warn "Building without ASIO: interfaces work through Windows audio only. Run with -NoAsio to skip this check."
    }
}

# ---- ONNX Runtime with DirectML ------------------------------------------------------

# A fixed version of each, checked against its SHA-256 before use, so a
# changed package never ends up in the app. To move to a newer one, change
# the version and put in the new package's SHA-256 (docs/DEVELOPING.md).
function Get-NuGet($id, $version, $sha256, $file, $inner) {
    $out = Join-Path $deps $file
    $stamp = "$out.version"
    if ((Test-Path $out) -and (Test-Path $stamp) -and ((Get-Content $stamp -Raw).Trim() -eq "$version $sha256")) { return $out }
    Say "Downloading $id $version"
    $pkg = Join-Path $deps "$id.$version.zip"
    Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/$($id.ToLower())/$version/$($id.ToLower()).$version.nupkg" -OutFile $pkg
    $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $pkg).Hash.ToLower()
    if ($got -ne $sha256) {
        Remove-Item $pkg
        throw "$id $version isn't the expected package (SHA-256 $got, expected $sha256)."
    }
    $tmp = Join-Path $deps "$id.$version"
    Expand-Archive -Path $pkg -DestinationPath $tmp -Force
    Copy-Item (Join-Path $tmp $inner) $out -Force
    Set-Content -LiteralPath $stamp -Value "$version $sha256"
    Remove-Item $pkg
    Remove-Item $tmp -Recurse -Force
    Write-Host "    $id $version"
    return $out
}

$ort = Get-NuGet "Microsoft.ML.OnnxRuntime.DirectML" "1.24.4" "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681" "onnxruntime.dll" "runtimes\win-x64\native\onnxruntime.dll"
$dml = Get-NuGet "Microsoft.AI.DirectML" "1.15.4" "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9" "DirectML.dll" "bin\x64-win\DirectML.dll"

# ---- build --------------------------------------------------------------------------------

$cargoArgs = @("build", "--locked", "--profile", "dist")
# The default features are whisper and a macOS-only one, so this drops just whisper here.
if ($NoWhisper) { $cargoArgs += @("--no-default-features") }
if ($features.Count -gt 0) { $cargoArgs += @("--features", ($features -join ",")) }
Say "cargo $($cargoArgs -join ' ')"
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "The build failed." }

Say "Building the virtual camera"
& cargo build --locked --release --manifest-path windows\vcam\Cargo.toml
if ($LASTEXITCODE -ne 0) { throw "The virtual camera's build failed." }

# ---- package ------------------------------------------------------------------------------

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$dist = Join-Path $root "dist\Cutaway"
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item target\dist\cutaway.exe (Join-Path $dist "Cutaway.exe")
Copy-Item windows\vcam\target\release\cutaway_vcam.dll $dist
Copy-Item $ort $dist
Copy-Item $dml $dist
New-Item -ItemType Directory -Force -Path (Join-Path $dist "streamdeck") | Out-Null
Copy-Item streamdeck\Cutaway.streamDeckPlugin (Join-Path $dist "streamdeck")
Copy-Item README.md, cutaway.example.toml $dist
New-Item -ItemType Directory -Force -Path (Join-Path $dist "licenses") | Out-Null
Copy-Item models\LICENSE-*.txt (Join-Path $dist "licenses")

$zip = Join-Path $root "dist\Cutaway-$version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $dist -DestinationPath $zip
Say "Built $dist"
Say "Zipped $zip"
if ($features -notcontains "asio" -and -not $NoAsio) { Warn "(without ASIO)" }
