[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ExecutablePath,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [switch]$RequireHardwareGpu
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$deadlineSeconds = 90
$process = $null
$gpuPreferenceBefore = $null
$gpuPreferenceChanged = $false
$gpuPreferenceKey = 'HKCU:\Software\Microsoft\DirectX\UserGpuPreferences'
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$profileDirectory = Join-Path $OutputDirectory 'profile'
$logPath = Join-Path $profileDirectory 'Odyssey.Desktop\logs\latest.log'
$transcriptPath = Join-Path $OutputDirectory 'smoke.log'
Start-Transcript -Path $transcriptPath -Force | Out-Null

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class DesktopSmokeNative {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int command);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
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

function Assert-DesktopReady {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Windows required.' }
    if (-not [Environment]::UserInteractive -or $env:SESSIONNAME -eq 'Services') { throw 'Interactive user session required.' }
    if ([DesktopSmokeNative]::InputDesktopName() -ne 'Default') { throw 'Unlocked interactive desktop required.' }
    $foreground = [DesktopSmokeNative]::GetForegroundWindow()
    if ($foreground -eq [IntPtr]::Zero) { throw 'Desktop cannot receive input.' }
}

Add-Type -AssemblyName System.Runtime.WindowsRuntime

function Wait-WinRt {
    param($Operation, [Type]$ResultType)
    $method = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
        $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetParameters().Count -eq 1 -and
        $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
    } | Select-Object -First 1
    if (-not $method) { throw 'Windows Runtime async bridge unavailable.' }
    $task = $method.MakeGenericMethod($ResultType).Invoke($null,@($Operation))
    if (-not $task.Wait(15000)) { throw 'Windows Runtime operation timed out.' }
    return $task.GetAwaiter().GetResult()
}

function Get-OcrText {
    param([string]$Path)
    $file = Wait-WinRt ([Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]::GetFileFromPathAsync($Path)) ([Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime])
    $stream = Wait-WinRt ($file.OpenAsync([Windows.Storage.FileAccessMode, Windows.Storage, ContentType=WindowsRuntime]::Read)) ([Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime])
    try {
        $decoder = Wait-WinRt ([Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime])
        $bitmap = Wait-WinRt ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime])
        $engine = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]::TryCreateFromUserProfileLanguages()
        if ($null -eq $engine) { throw 'Windows OCR has no installed recognition language.' }
        $result = Wait-WinRt ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult, Windows.Media.Ocr, ContentType=WindowsRuntime])
        return [string]::Join("`n", @($result.Lines | ForEach-Object { $_.Text }))
    } finally { $stream.Dispose() }
}

function Capture-Ocr {
    param([string]$Name)
    Assert-DesktopReady
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try { $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size); $path = Join-Path $OutputDirectory $Name; $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png) }
    finally { $graphics.Dispose(); $bitmap.Dispose() }
    return [pscustomobject]@{ Path=$path; Text=(Get-OcrText $path); Bounds=$bounds }
}

function Wait-Ocr {
    param([string]$Name, [string[]]$Required, [int]$Timeout=$deadlineSeconds)
    $until = [DateTime]::UtcNow.AddSeconds($Timeout)
    do {
        if ($process.HasExited) { throw "Application exited with code $($process.ExitCode)." }
        $shot = Capture-Ocr $Name
        $text = $shot.Text
        if (@($Required | Where-Object { $text -notmatch [regex]::Escape($_) }).Count -eq 0) { return $shot }
        Start-Sleep -Milliseconds 900
    } while ([DateTime]::UtcNow -lt $until)
    throw "Timed out waiting for UI text: $($Required -join ', '). Last OCR: $text"
}

function Click-OcrText {
    param($Shot, [string]$Text)
    $file = Wait-WinRt ([Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]::GetFileFromPathAsync($Shot.Path)) ([Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime])
    $stream = Wait-WinRt ($file.OpenAsync([Windows.Storage.FileAccessMode, Windows.Storage, ContentType=WindowsRuntime]::Read)) ([Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime])
    try {
        $decoder = Wait-WinRt ([Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime])
        $bitmap = Wait-WinRt ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime])
        $engine = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]::TryCreateFromUserProfileLanguages()
        $result = Wait-WinRt ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult, Windows.Media.Ocr, ContentType=WindowsRuntime])
        $line = @($result.Lines | Where-Object { $_.Text -match [regex]::Escape($Text) } | Select-Object -First 1)[0]
        if ($null -eq $line) { throw "Cannot click absent OCR text '$Text'." }
        $x = [int]($line.Words[0].BoundingRect.X + ($line.Words[-1].BoundingRect.X + $line.Words[-1].BoundingRect.Width - $line.Words[0].BoundingRect.X)/2)
        $y = [int]($line.Words[0].BoundingRect.Y + $line.Words[0].BoundingRect.Height/2)
        if ($bitmap.PixelWidth -ne $Shot.Bounds.Width -or $bitmap.PixelHeight -ne $Shot.Bounds.Height) { throw 'Screenshot dimensions changed during OCR.' }
        if (-not [DesktopSmokeNative]::SetCursorPos($Shot.Bounds.Left + $x, $Shot.Bounds.Top + $y)) { throw 'Could not position mouse.' }
        Start-Sleep -Milliseconds 100
        [DesktopSmokeNative]::mouse_event(0x0002,0,0,0,[UIntPtr]::Zero)
        Start-Sleep -Milliseconds 100
        [DesktopSmokeNative]::mouse_event(0x0004,0,0,0,[UIntPtr]::Zero)
    } finally { $stream.Dispose() }
}

try {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Assert-DesktopReady
    $exe = (Resolve-Path -LiteralPath $ExecutablePath).Path
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Executable not found: $ExecutablePath" }
    if ($RequireHardwareGpu) {
        $gpu = Get-CimInstance Win32_VideoController | Where-Object { $_.Name -match 'AMD|Radeon' }
        if (-not $gpu) { throw 'Required AMD Radeon GPU not present.' }
        if (@($gpu | Where-Object { $_.ConfigManagerErrorCode -eq 0 }).Count -eq 0) { throw 'AMD Radeon GPU is unhealthy.' }
        if (-not (Test-Path -LiteralPath $gpuPreferenceKey)) { New-Item -Path $gpuPreferenceKey | Out-Null }
        $gpuPreferenceBefore = (Get-Item -LiteralPath $gpuPreferenceKey).GetValue($exe, $null)
        New-ItemProperty -LiteralPath $gpuPreferenceKey -Name $exe -Value 'GpuPreference=2;' -PropertyType String -Force | Out-Null
        $gpuPreferenceChanged = $true
    }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exe
    $start.WorkingDirectory = Split-Path -Parent $exe
    $start.UseShellExecute = $false
    $start.EnvironmentVariables['ODYSSEY_OFFLINE'] = '1'
    $start.EnvironmentVariables['LOCALAPPDATA'] = $profileDirectory
    $start.EnvironmentVariables['ODYSSEY_FIRMWARE_PACK_DIR'] = Join-Path $OutputDirectory 'empty-firmware-pack'
    New-Item -ItemType Directory -Force -Path $start.EnvironmentVariables['ODYSSEY_FIRMWARE_PACK_DIR'] | Out-Null
    $process = [Diagnostics.Process]::Start($start)
    if (-not $process) { throw 'Could not start app.' }
    $window = [IntPtr]::Zero
    $until = [DateTime]::UtcNow.AddSeconds($deadlineSeconds)
    while ([DateTime]::UtcNow -lt $until) {
        if ($process.HasExited) { throw "Application exited with code $($process.ExitCode)." }
        $window = [DesktopSmokeNative]::GetForegroundWindow()
        [uint32]$ownerPid = 0
        if ($window -ne [IntPtr]::Zero -and [DesktopSmokeNative]::GetWindowThreadProcessId($window,[ref]$ownerPid) -and $ownerPid -eq $process.Id -and [DesktopSmokeNative]::IsWindowVisible($window)) { break }
        Start-Sleep -Milliseconds 300
    }
    if ($window -eq [IntPtr]::Zero -or $ownerPid -ne $process.Id) { throw 'Timed out waiting for app foreground window.' }
    [DesktopSmokeNative]::ShowWindow($window,9) | Out-Null
    [DesktopSmokeNative]::SetForegroundWindow($window) | Out-Null
    Start-Sleep -Seconds 3
    $setup = Wait-Ocr 'setup.png' @('Setup','Choose a display and tracker')
    if ($setup.Text -match 'Could not set up temporary settings|configuration is corrupted') { throw 'App displayed unexpected configuration error.' }
    if ($RequireHardwareGpu) {
        $until = [DateTime]::UtcNow.AddSeconds(15)
        do { $gpuLog = if (Test-Path -LiteralPath $logPath) { Get-Content -Raw -LiteralPath $logPath } else { '' }; Start-Sleep -Milliseconds 500 } while ([DateTime]::UtcNow -lt $until -and $gpuLog -notmatch 'Using GPU:')
        $selectedGpu = [regex]::Matches($gpuLog,'Using GPU:\s*(.+)') | Select-Object -Last 1
        if (-not $selectedGpu -or $selectedGpu.Groups[1].Value -notmatch 'AMD|Radeon|hardware' -or $selectedGpu.Groups[1].Value -match 'software|WARP') { throw "Hardware GPU selection not confirmed by latest.log: $gpuLog" }
    }
    Click-OcrText $setup 'Settings'
    $settings = Wait-Ocr 'settings.png' @('Settings','Firmware updates','About')
    Click-OcrText $settings 'Setup'
    $setup = Wait-Ocr 'setup-return.png' @('Setup','Calibrate screen') 20
    Click-OcrText $setup 'Calibrate screen'
    $wizard = Wait-Ocr 'calibration-setup.png' @("Let's calibrate a screen",'Which screen?') 20
    Click-OcrText $wizard 'Back to setup'
    $closed = Wait-Ocr 'wizard-closed.png' @('Setup','Calibrate screen') 20
    if ($closed.Text -match "Let's calibrate a screen") { throw 'Calibration setup did not close.' }
    if (Test-Path -LiteralPath $logPath) { Copy-Item -LiteralPath $logPath -Destination (Join-Path $OutputDirectory 'latest.log') -Force }
    if (-not (Test-Path -LiteralPath (Join-Path $OutputDirectory 'setup.png')) -or -not (Test-Path -LiteralPath (Join-Path $OutputDirectory 'settings.png')) -or -not (Test-Path -LiteralPath (Join-Path $OutputDirectory 'calibration-setup.png'))) { throw 'Required screenshots missing.' }
    Write-Output "Windows desktop smoke passed. Screenshots and logs: $OutputDirectory"
} catch {
    try {
        $failure = Join-Path $OutputDirectory 'failure.txt'
        $_ | Out-String | Set-Content -LiteralPath $failure -Encoding UTF8
        if ([DesktopSmokeNative]::InputDesktopName() -eq 'Default') { $null = Capture-Ocr 'failure.png' }
        if (Test-Path -LiteralPath $logPath) { Copy-Item -LiteralPath $logPath -Destination (Join-Path $OutputDirectory 'latest.log') -Force }
    } catch { $_ | Out-String | Add-Content -LiteralPath (Join-Path $OutputDirectory 'failure.txt') -Encoding UTF8 }
    throw
} finally {
    if ($null -ne $process) { try { if (-not $process.HasExited) { $process.CloseMainWindow() | Out-Null; if (-not $process.WaitForExit(4000)) { $process.Kill() } } } catch {} }
    if ($gpuPreferenceChanged) {
        if ($null -eq $gpuPreferenceBefore) { Remove-ItemProperty -LiteralPath $gpuPreferenceKey -Name $exe }
        else { Set-ItemProperty -LiteralPath $gpuPreferenceKey -Name $exe -Value $gpuPreferenceBefore }
    }
    Stop-Transcript | Out-Null
}

