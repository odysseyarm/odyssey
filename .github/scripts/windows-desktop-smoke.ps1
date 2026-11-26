[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PackagePath,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [switch]$RequireHardwareGpu
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$gpuPreferenceKey = 'HKCU:\Software\Microsoft\DirectX\UserGpuPreferences'
$gpuPreferenceChanged = $false
$gpuPreferenceBefore = $null
$environmentBefore = @{}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$artifacts = Join-Path $OutputDirectory ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $artifacts | Out-Null
Start-Transcript -Path (Join-Path $artifacts 'smoke.log') | Out-Null

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class DesktopSmokeNative {
  [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool GetUserObjectInformation(IntPtr h, int index, StringBuilder info, uint length, out uint needed);
  [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr h);
  public static string InputDesktopName() {
    IntPtr d=OpenInputDesktop(0,false,0x0100); if(d==IntPtr.Zero) return "";
    try { uint n; var b=new StringBuilder(256); return GetUserObjectInformation(d,2,b,512,out n) ? b.ToString() : ""; }
    finally { CloseDesktop(d); }
  }
}
'@

try {
    if (-not [Environment]::UserInteractive -or $env:SESSIONNAME -eq 'Services' -or
        [DesktopSmokeNative]::InputDesktopName() -ne 'Default') {
        throw 'Unlocked interactive Windows desktop required.'
    }
    $PackagePath = (Resolve-Path -LiteralPath $PackagePath).Path
    $manifest = Get-Content -LiteralPath (Join-Path $PackagePath 'manifest.json') -Raw | ConvertFrom-Json
    foreach ($relative in @('app/odyssey-desktop.exe', 'tests/gui_smoke.exe')) {
        $expected = $manifest.sha256.$relative
        $path = Join-Path $PackagePath $relative
        if ($expected -notmatch '^[0-9a-fA-F]{64}$' -or
            (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $expected) {
            throw "Invalid or mismatched GUI package hash: $relative"
        }
    }
    $exe = Join-Path $PackagePath 'app/odyssey-desktop.exe'
    if ($RequireHardwareGpu) {
        $gpu = Get-CimInstance Win32_VideoController | Where-Object {
            $_.Name -match 'AMD|Radeon' -and $_.ConfigManagerErrorCode -eq 0
        }
        if (-not $gpu) { throw 'Healthy AMD Radeon GPU required.' }
        if (-not (Test-Path -LiteralPath $gpuPreferenceKey)) { New-Item -Path $gpuPreferenceKey | Out-Null }
        $gpuPreferenceBefore = (Get-Item -LiteralPath $gpuPreferenceKey).GetValue($exe, $null)
        New-ItemProperty -LiteralPath $gpuPreferenceKey -Name $exe -Value 'GpuPreference=2;' -PropertyType String -Force | Out-Null
        $gpuPreferenceChanged = $true
    }
    foreach ($name in @('ODYSSEY_GUI_EXECUTABLE', 'ODYSSEY_GUI_ARTIFACTS')) {
        $environmentBefore[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    $env:ODYSSEY_GUI_EXECUTABLE = $exe
    $env:ODYSSEY_GUI_ARTIFACTS = $artifacts
    & (Join-Path $PackagePath 'tests/gui_smoke.exe') --ignored --test-threads=1 --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Native GUI tests failed: exit $LASTEXITCODE" }
    if ($RequireHardwareGpu) {
        $logs = @(Get-ChildItem -LiteralPath $artifacts -Recurse -Filter latest.log)
        if ($logs.Count -ne 1) { throw 'Expected one current-launch application log.' }
        $selected = [regex]::Matches((Get-Content -LiteralPath $logs[0].FullName -Raw), 'Using GPU:\s*([^\r\n]+)') | Select-Object -Last 1
        if (-not $selected -or $selected.Groups[1].Value -notmatch 'AMD|Radeon' -or
            $selected.Groups[1].Value -match 'software|WARP') {
            throw 'Current-launch log does not confirm AMD Radeon rendering.'
        }
    }
    Write-Output "Native GUI smoke passed. Screenshots, widget snapshots and logs: $artifacts"
} catch {
    $_ | Out-String | Set-Content -LiteralPath (Join-Path $artifacts 'failure.txt') -Encoding UTF8
    throw
} finally {
    foreach ($name in $environmentBefore.Keys) {
        [Environment]::SetEnvironmentVariable($name, $environmentBefore[$name], 'Process')
    }
    if ($gpuPreferenceChanged) {
        if ($null -eq $gpuPreferenceBefore) {
            Remove-ItemProperty -LiteralPath $gpuPreferenceKey -Name $exe
        } else {
            New-ItemProperty -LiteralPath $gpuPreferenceKey -Name $exe -Value $gpuPreferenceBefore -PropertyType String -Force | Out-Null
        }
    }
    Stop-Transcript | Out-Null
}
