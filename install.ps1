$ErrorActionPreference = "Stop"

$repo = "NeelM0906/zipcode"
$version = if ($env:ZIPCODE_VERSION) { $env:ZIPCODE_VERSION } else { "latest" }
$installDir = if ($env:ZIPCODE_INSTALL_DIR) { $env:ZIPCODE_INSTALL_DIR } else { Join-Path $HOME ".local\bin" }
$target = "x86_64-pc-windows-msvc"
$asset = "zipcode-$target.zip"
$releaseUrl = if ($version -eq "latest") {
    "https://github.com/$repo/releases/latest/download"
} else {
    "https://github.com/$repo/releases/download/$version"
}
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("zipcode-" + [guid]::NewGuid())

try {
    New-Item -ItemType Directory -Path $temporary | Out-Null
    Write-Host "Downloading ZIPCODE for $target..."
    Invoke-WebRequest "$releaseUrl/$asset" -OutFile (Join-Path $temporary $asset)
    Invoke-WebRequest "$releaseUrl/SHA256SUMS" -OutFile (Join-Path $temporary "SHA256SUMS")
    $checksumLine = Get-Content (Join-Path $temporary "SHA256SUMS") |
        Where-Object { $_ -match "\s$([regex]::Escape($asset))$" } |
        Select-Object -First 1
    if (-not $checksumLine) { throw "No checksum was published for $asset." }
    $expected = ($checksumLine -split "\s+")[0].ToLowerInvariant()
    $actual = (Get-FileHash (Join-Path $temporary $asset) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "ZIPCODE download checksum mismatch." }

    Expand-Archive (Join-Path $temporary $asset) -DestinationPath $temporary
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item (Join-Path $temporary "zipcode-$target\*.exe") $installDir -Force

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($userPath -split ";") -notcontains $installDir) {
        [Environment]::SetEnvironmentVariable("Path", "$installDir;$userPath", "User")
        Write-Host "Added $installDir to your user PATH. Open a new terminal."
    }
    Write-Host "ZIPCODE installed in $installDir."
    Write-Host "Run: zip-code login"
} finally {
    if (Test-Path $temporary) { Remove-Item -Recurse -Force $temporary }
}
