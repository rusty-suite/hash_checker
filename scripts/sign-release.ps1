# sign-release.ps1
# Compile en release et signe les binaires Windows et Linux.
#
# Utilisation :
#   .\scripts\sign-release.ps1
#
# Certificat Windows (.pfx) stocké HORS du dépôt git :
#   C:\Users\alecc\.certificates\hash-checker-sign.pfx
#
# Clé GPG pour Linux : stockée dans le trousseau GPG système (gpg --list-keys)

$ErrorActionPreference = "Stop"

# ── Configuration ────────────────────────────────────────────────────────────
$root         = Split-Path $PSScriptRoot -Parent
$back         = "$root\scripts"
$CertPath     = "$env:USERPROFILE\.certificates\hash-checker-sign.pfx"
$AppName      = "Hash Checker"
$TimestampUrl = "http://timestamp.digicert.com"

# Chemins possibles du binaire Linux (WSL, cross-compile GNU, cross-compile MUSL)
$LinuxPaths = @(
    "$root\target\release\hash_checker",
    "$root\target\x86_64-unknown-linux-gnu\release\hash_checker",
    "$root\target\x86_64-unknown-linux-musl\release\hash_checker"
)

# ── Compilation release Windows ──────────────────────────────────────────────
Write-Host ""
Write-Host "Compilation release Windows..." -ForegroundColor Cyan
Set-Location $root
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$WinExe = "$root\target\release\hash_checker.exe"

# ── Signature Windows (signtool + .pfx) ──────────────────────────────────────
Write-Host ""
Write-Host "=== Signature Windows ===" -ForegroundColor White

if (-not (Test-Path $CertPath)) {
    Write-Warning "Certificat introuvable : $CertPath"
    Write-Warning "Lance .\scripts\create-test-cert.ps1 pour en créer un."
} else {
    $SignTool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" `
        -Recurse -Filter "signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName

    if (-not $SignTool) {
        Write-Warning "signtool.exe introuvable. Installe le Windows SDK."
    } else {
        $password = Read-Host "Mot de passe du certificat .pfx" -AsSecureString
        $bstr     = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($password)
        $plain    = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)

        & $SignTool sign `
            /fd SHA256 `
            /f $CertPath `
            /p $plain `
            /d $AppName `
            /tr $TimestampUrl `
            /td SHA256 `
            $WinExe

        [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)

        if ($LASTEXITCODE -eq 0) {
            Write-Host "Windows signe : $WinExe" -ForegroundColor Green
        } else {
            Write-Warning "Echec de la signature Windows."
        }
    }
}

# ── Signature Linux (GPG) ─────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== Signature Linux ===" -ForegroundColor White

$LinuxBin = $LinuxPaths | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $LinuxBin) {
    Write-Host "Binaire Linux introuvable — signature ignoree." -ForegroundColor Yellow
    Write-Host "Emplacements cherches :"
    $LinuxPaths | ForEach-Object { Write-Host "  $_" }
} else {
    Write-Host "Binaire Linux trouve : $LinuxBin" -ForegroundColor Cyan

    $gpg = Get-Command gpg -ErrorAction SilentlyContinue
    if (-not $gpg) {
        Write-Warning "gpg introuvable. Installe Gpg4win ou Git for Windows."
    } else {
        $SigPath = "$LinuxBin.sig"
        & gpg --detach-sign --armor --output $SigPath $LinuxBin

        if ($LASTEXITCODE -eq 0) {
            Write-Host "Linux signe  : $LinuxBin" -ForegroundColor Green
            Write-Host "Signature    : $SigPath" -ForegroundColor Green
        } else {
            Write-Warning "Echec de la signature GPG."
        }
    }
}

Write-Host ""
Write-Host "Termine." -ForegroundColor White
Set-Location "$back"