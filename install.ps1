$ErrorActionPreference = "Stop"

$Repo = "princetonacelopez/ace-term"
$BinName = "ace-term.exe"
$InstallDir = if ($env:ACE_TERM_INSTALL_DIR) { $env:ACE_TERM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "ace-term\bin" }

$Arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    "X64"   { "x86_64-pc-windows-msvc" }
    "Arm64" { "aarch64-pc-windows-msvc" }
    default { throw "Unsupported Windows architecture: $_" }
}

$Tag = $env:ACE_TERM_VERSION
if (-not $Tag) {
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $Tag = $Release.tag_name
}

if (-not $Tag) {
    throw "Could not determine latest release tag for $Repo."
}

$Archive = "ace-term-$Tag-$Arch.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Tag"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("ace-term-install-" + [System.Guid]::NewGuid().ToString("N"))

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
    $ArchivePath = Join-Path $TempDir $Archive
    $ChecksumsPath = Join-Path $TempDir "checksums.txt"

    Write-Host "Downloading $Archive ($Tag)..."
    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ArchivePath
    Invoke-WebRequest -Uri "$BaseUrl/checksums.txt" -OutFile $ChecksumsPath

    $ExpectedLine = Get-Content $ChecksumsPath | Where-Object { $_ -match "\s$([regex]::Escape($Archive))$" } | Select-Object -First 1
    if (-not $ExpectedLine) {
        throw "Checksum entry not found for $Archive"
    }
    $ExpectedHash = ($ExpectedLine -split "\s+")[0].ToUpperInvariant()
    $ActualHash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($ExpectedHash -ne $ActualHash) {
        throw "Checksum mismatch for $Archive"
    }

    Expand-Archive -Path $ArchivePath -DestinationPath $TempDir -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path (Join-Path $TempDir $BinName) -Destination (Join-Path $InstallDir $BinName) -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "Added $InstallDir to user PATH. Restart your terminal to use 'ace-term'."
    }

    Write-Host "Installed $BinName to $(Join-Path $InstallDir $BinName)"
}
finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
