# create-gpg-key.ps1
# Crée une clé GPG pour signer les binaires Linux.
# La clé est stockée dans le trousseau GPG système (hors du dépôt git).
#
# Utilisation :
#   .\scripts\create-gpg-key.ps1

$ErrorActionPreference = "Stop"

$gpg = Get-Command gpg -ErrorAction SilentlyContinue
if (-not $gpg) {
    Write-Error "gpg introuvable. Installe Gpg4win depuis https://www.gpg4win.org/"
    exit 1
}

Write-Host ""
Write-Host "=== Création de la clé GPG ===" -ForegroundColor Cyan
Write-Host ""

$name  = Read-Host "Nom complet (ex: Rusty-Suite)"
$email = Read-Host "Email"

# Générer la clé GPG en mode batch (sans interaction)
$batch = @"
%no-protection
Key-Type: RSA
Key-Length: 4096
Subkey-Type: RSA
Subkey-Length: 4096
Name-Real: $name
Name-Email: $email
Expire-Date: 0
%commit
"@

$batchFile = "$env:TEMP\gpg-batch.txt"
$batch | Set-Content $batchFile -Encoding UTF8

Write-Host ""
Write-Host "Génération de la clé (peut prendre quelques secondes)..." -ForegroundColor Cyan
& gpg --batch --gen-key $batchFile
Remove-Item $batchFile

if ($LASTEXITCODE -ne 0) {
    Write-Error "Echec de la génération de la clé GPG."
    exit 1
}

# Afficher la clé créée
Write-Host ""
Write-Host "Clé GPG créée avec succès :" -ForegroundColor Green
& gpg --list-keys $email

# Exporter la clé publique hors du repo git
$certDir    = "$env:USERPROFILE\.certificates"
$pubKeyPath = "$certDir\hash-checker-public.asc"

if (-not (Test-Path $certDir)) {
    New-Item -ItemType Directory -Path $certDir | Out-Null
}

& gpg --armor --export $email | Set-Content $pubKeyPath -Encoding UTF8
Write-Host ""
Write-Host "Clé publique exportée : $pubKeyPath" -ForegroundColor Green
Write-Host "Publie cette clé sur GitHub ou un serveur de clés pour que les utilisateurs puissent vérifier la signature."
Write-Host ""
Write-Host "Pour publier sur un serveur de clés :" -ForegroundColor Cyan
Write-Host "  gpg --keyserver keyserver.ubuntu.com --send-keys <ID_DE_TA_CLE>"
Write-Host ""
Write-Host "Lance maintenant : .\scripts\sign-release.ps1" -ForegroundColor Cyan
