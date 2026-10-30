<#
.SYNOPSIS
  Cria a distro WSL dedicada do Control Tower a partir do rootfs oficial do Ubuntu 24.04.
.EXAMPLE
  .\create-distro.ps1 -User usuario -Nome ctower-agents -Local E:\wsl\ctower-agents
#>
param(
    [Parameter(Mandatory = $true)][string]$User,
    [Parameter(Mandatory = $true)][string]$Nome,
    [Parameter(Mandatory = $true)][string]$Local,
    [string]$RootfsUrl = "https://cloud-images.ubuntu.com/wsl/releases/24.04/current/ubuntu-noble-wsl-amd64-wsl.rootfs.tar.gz"
)
$ErrorActionPreference = "Stop"

$existing = (wsl.exe -l -q) -replace "`0", "" | Where-Object { $_.Trim() -eq $Nome }
if ($existing) {
    Write-Host "Distro '$Nome' ja existe; nada a fazer."
    exit 0
}

New-Item -ItemType Directory -Force $Local | Out-Null
$tar = Join-Path $env:TEMP "control-tower-rootfs.tar.gz"
if (-not (Test-Path $tar)) {
    Write-Host "Baixando rootfs do Ubuntu 24.04..."
    Invoke-WebRequest -Uri $RootfsUrl -OutFile $tar
}

wsl.exe --import $Nome $Local $tar --version 2
if ($LASTEXITCODE -ne 0) { throw "wsl --import falhou" }

$setup = @"
set -e
id -u $User >/dev/null 2>&1 || useradd -m -s /bin/bash -G sudo $User
echo '$User ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/90-$User
chmod 440 /etc/sudoers.d/90-$User
printf '[boot]\nsystemd=true\n[user]\ndefault=$User\n[interop]\nappendWindowsPath=false\n' > /etc/wsl.conf
"@
wsl.exe -d $Nome -u root -- bash -c ($setup -replace "`r", "")
if ($LASTEXITCODE -ne 0) { throw "configuracao inicial falhou" }

wsl.exe --terminate $Nome
Write-Host "Distro '$Nome' criada. Proximo passo: wsl -d $Nome e rodar scripts/provision/provision.sh"
