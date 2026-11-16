param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath
)

$ErrorActionPreference = 'Stop'
$installer = (Resolve-Path -LiteralPath $InstallerPath -ErrorAction Stop).Path
$evidence = Join-Path $PWD 'windows-installer-smoke'
New-Item -ItemType Directory -Force -Path $evidence | Out-Null
$name = [IO.Path]::GetFileNameWithoutExtension($installer)
$installLog = Join-Path $evidence "$name-install.log"
$removeLog = Join-Path $evidence "$name-uninstall.log"
$offlineKey = 'HKLM:\SOFTWARE\OdysseyArm\Odyssey'
$installDir = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Odyssey'
$exe = Join-Path $installDir 'odyssey-desktop.exe'
$installerApi = New-Object -ComObject WindowsInstaller.Installer
$database = $installerApi.OpenDatabase($installer, 0)
$view = $database.OpenView("SELECT `Value` FROM `Property` WHERE `Property`='ProductCode'")
$view.Execute()
$productCode = $view.Fetch().StringData(1)
$view.Close()
$view = $database.OpenView("SELECT Value FROM Property WHERE Property='UpgradeCode'")
$view.Execute()
$upgradeCode = $view.Fetch().StringData(1)
$view.Close()
if (-not $upgradeCode) { throw 'MSI UpgradeCode missing' }
if (-not $productCode) { throw 'MSI ProductCode missing' }
$installedProducts = @($installerApi.RelatedProducts($upgradeCode))
if ($installedProducts.Count) { throw "Related MSI product already installed: $($installedProducts -join ', ')" }
$installed = $false

try {
    $installArgs = "/i `"$installer`" /qn /norestart OFFLINE_MODE=1 /L*v `"$installLog`""
    $install = Start-Process msiexec.exe -ArgumentList $installArgs -Wait -PassThru
    if ($install.ExitCode -notin @(0, 3010)) { throw "MSI install failed: $($install.ExitCode)" }
    $installed = $true
    if (-not (Test-Path -LiteralPath $exe)) { throw "Installed executable missing: $exe" }
    $policy = Get-ItemProperty -LiteralPath $offlineKey
    if ($policy.OfflineMode -ne 1) { throw 'OFFLINE_MODE registry value missing or incorrect' }
}
finally {
    if ($installed) {
        $removeArgs = "/x $productCode /qn /norestart /L*v `"$removeLog`""
        $remove = Start-Process msiexec.exe -ArgumentList $removeArgs -Wait -PassThru
        if ($remove.ExitCode -notin @(0, 3010)) { throw "MSI uninstall failed: $($remove.ExitCode)" }
        if ($installerApi.Products | Where-Object { $_ -eq $productCode }) { throw 'MSI product remained after uninstall' }
        if (Test-Path -LiteralPath $exe) { throw "Executable remained after uninstall: $exe" }
        if (Test-Path -LiteralPath $offlineKey) {
            $policy = Get-ItemProperty -LiteralPath $offlineKey -ErrorAction SilentlyContinue
            if ($policy -and $null -ne $policy.OfflineMode) { throw 'OfflineMode registry value remained after uninstall' }
        }
    }
}
