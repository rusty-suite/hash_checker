# create-test-cert.ps1
# Crée un certificat auto-signé pour tester la signature du binaire.
# ATTENTION : un certificat auto-signé déclenche quand même SmartScreen.
# Pour une vraie distribution, utilise un certificat d'une autorité reconnue.
#
# Utilisation :
#   .\scripts\create-test-cert.ps1

$ErrorActionPreference = "Stop"

$CertDir  = "$env:USERPROFILE\.certificates"
$CertPath = "$CertDir\hash-checker-sign.pfx"

# Créer le dossier hors du repo git
if (-not (Test-Path $CertDir)) {
    New-Item -ItemType Directory -Path $CertDir | Out-Null
    Write-Host "Dossier cree : $CertDir" -ForegroundColor Cyan
}

if (Test-Path $CertPath) {
    Write-Host "Certificat deja existant : $CertPath" -ForegroundColor Yellow
    exit 0
}

# Créer le certificat auto-signé
$cert = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject "CN=Hash Checker (test), O=Rusty-Suite, C=CH" `
    -KeyUsage DigitalSignature `
    -FriendlyName "Hash Checker Code Signing (test)" `
    -CertStoreLocation Cert:\CurrentUser\My `
    -HashAlgorithm SHA256 `
    -NotAfter (Get-Date).AddYears(3)

# Exporter en .pfx avec mot de passe
$password = Read-Host "Choisis un mot de passe pour le .pfx" -AsSecureString
Export-PfxCertificate -Cert $cert -FilePath $CertPath -Password $password | Out-Null

Write-Host ""
Write-Host "Certificat cree : $CertPath" -ForegroundColor Green
Write-Host "Lance maintenant : .\scripts\sign-release.ps1" -ForegroundColor Cyan
