# Make Snagster's MSIX package from a built Snagster.exe (see README.md beside this file).
#
#   pwsh packaging/msix/make-msix.ps1 -Exe target/release/snagster.exe -Out dist/Snagster-windows-x64.msix `
#        [-Publisher "CN=..."] [-TestCert dist/test.cer] [-IdentityName X -PublisherDisplayName Y]
#
# The Microsoft Store package takes the Identity Name, Publisher and Publisher display name
# Partner Center shows under Product identity, and is uploaded unsigned (the Store signs it).
#
# -Publisher is the signing certificate's subject, exactly (Windows refuses a package whose
# publisher differs from its signature). Signing:
#   (default)                unsigned, for signing afterwards: CI signs it with Azure Artifact
#                            Signing (azure/artifact-signing-action).
#   -TestCert <file.cer>     no certificate yet: make a throwaway self-signed one, sign with it and
#                            write its public part to <file.cer>. Only computers that import that
#                            certificate (Trusted People, an administrator's job) can install it.
# Uses makeappx.exe and signtool.exe from the Windows SDK (on GitHub's Windows runners).
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out,
    [string] $Publisher = "CN=Snagster Test Signing",
    [string] $IdentityName = "SoloSaftis.Snagster",
    [string] $PublisherDisplayName = "SoloSaftis",
    [string] $TestCert = ""
)
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$root = Resolve-Path "$here/../.."

# The version from Cargo.toml, as Windows wants it: four numbers.
$version = (Select-String -Path "$root/Cargo.toml" -Pattern '^version = "(\d+)\.(\d+)\.(\d+)' | Select-Object -First 1).Matches[0]
$version = "$($version.Groups[1].Value).$($version.Groups[2].Value).$($version.Groups[3].Value).0"

function Find-SdkTool($name) {
    $found = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\$name" -ErrorAction SilentlyContinue |
        Sort-Object { [version]($_.Directory.Parent.Name) } | Select-Object -Last 1
    if (-not $found) { throw "$name isn't installed (it comes with the Windows SDK)" }
    $found.FullName
}

$stage = Join-Path ([IO.Path]::GetTempPath()) "snagster-msix-$PID"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage/Assets" | Out-Null
Copy-Item $Exe "$stage/Snagster.exe"
Copy-Item "$root/LICENSE-MIT", "$root/LICENSE-APACHE" $stage
$esc = { param($s) [Security.SecurityElement]::Escape($s) }
# The languages Snagster speaks, as Windows language codes, English (the default) first.
$langs = (Get-Content "$here/store-languages.json" -Raw | ConvertFrom-Json).PSObject.Properties |
    Where-Object { $_.Name -ne "_comment" } | ForEach-Object { $_.Value }
$langs = @("en-us") + @($langs | Where-Object { $_ -ne "en-us" })
$resources = ($langs | ForEach-Object { "    <Resource Language=`"$_`" />" }) -join "`r`n"
(Get-Content "$here/AppxManifest.xml" -Raw).Replace("__RESOURCES__", $resources).Replace("__VERSION__", $version).Replace("__IDENTITY_NAME__", (& $esc $IdentityName)).Replace("__PUBLISHER_DISPLAY_NAME__", (& $esc $PublisherDisplayName)).Replace("__PUBLISHER__", (& $esc $Publisher)) |
    Set-Content "$stage/AppxManifest.xml" -Encoding utf8

# The logos, drawn by Snagster itself (the same icon as the window and the portable exe).
$icons = @{ "StoreLogo.png" = 50; "Square44x44Logo.png" = 44; "Square150x150Logo.png" = 150; "Square44x44Logo.targetsize-256_altform-unplated.png" = 256 }
foreach ($name in $icons.Keys) {
    Start-Process $Exe -ArgumentList '--icon-png', "$stage/Assets/$name", $icons[$name] -Wait -NoNewWindow
    if (-not (Test-Path "$stage/Assets/$name")) { throw "Snagster didn't write $name" }
}
# The wide tile: the 150 px icon in the middle of 310 x 150.
Add-Type -AssemblyName System.Drawing
$wide = New-Object System.Drawing.Bitmap 310, 150
$g = [System.Drawing.Graphics]::FromImage($wide)
$square = [System.Drawing.Image]::FromFile((Resolve-Path "$stage/Assets/Square150x150Logo.png"))
$g.DrawImage($square, 80, 0, 150, 150)
$square.Dispose(); $g.Dispose()
$wide.Save("$stage/Assets/Wide310x150Logo.png", [System.Drawing.Imaging.ImageFormat]::Png)
$wide.Dispose()

New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
Remove-Item $Out -ErrorAction SilentlyContinue
& (Find-SdkTool makeappx.exe) pack /d $stage /p $Out /o /h SHA256
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }

if ($TestCert) {
    $signtool = Find-SdkTool signtool.exe
    $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher -KeyUsage DigitalSignature -FriendlyName "Snagster test signing" `
        -CertStoreLocation "Cert:\CurrentUser\My" -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
    try {
        & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /s My $Out
        if ($LASTEXITCODE -ne 0) { throw "signing failed" }
        New-Item -ItemType Directory -Force (Split-Path $TestCert) | Out-Null
        Export-Certificate -Cert $cert -FilePath $TestCert | Out-Null
    } finally { Remove-Item "Cert:\CurrentUser\My\$($cert.Thumbprint)" -ErrorAction SilentlyContinue }
    Write-Host "Signed with a throwaway test certificate ($TestCert): for testing only."
} else {
    Write-Host "Not signed yet: sign it before handing it out."
}
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
Get-Item $Out | Format-Table Name, Length
