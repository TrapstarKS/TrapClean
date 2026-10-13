# Developer/CI smoke check. This script never scans or cleans filesystem data.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})$')]
    [string]$SourceSha,

    [string]$OutputDirectory = 'release-assets',

    [string]$InstallDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'This smoke check must run on Windows. No Windows runtime result was produced.'
}

$temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$temporaryRoot = [IO.Path]::GetFullPath($temporaryRoot).TrimEnd('\')
if (!$InstallDirectory) {
    $InstallDirectory = Join-Path $temporaryRoot ('TrapClean-smoke-' + [Guid]::NewGuid().ToString('N'))
}
$InstallDirectory = [IO.Path]::GetFullPath($InstallDirectory).TrimEnd('\')
if (!$InstallDirectory.StartsWith($temporaryRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or
    (Split-Path $InstallDirectory -Leaf) -notmatch '^TrapClean-smoke-[a-zA-Z0-9-]+$') {
    throw 'Use a uniquely named TrapClean-smoke-* install directory inside RUNNER_TEMP or the system temporary directory.'
}
if (Test-Path -LiteralPath $InstallDirectory) {
    throw 'The smoke install directory already exists. Refusing to overwrite or uninstall an existing installation.'
}

$OutputDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$reportPath = Join-Path $OutputDirectory 'runtime-smoke-windows-x64.json'
$screenshotPath = Join-Path $OutputDirectory 'runtime-smoke-windows-x64.png'
if ((Test-Path -LiteralPath $reportPath) -or (Test-Path -LiteralPath $screenshotPath)) {
    throw 'Smoke evidence already exists in the output directory. Use a fresh output directory to avoid mixing results.'
}

$report = [ordered]@{
    schemaVersion = 1
    sourceSha = $SourceSha.ToLowerInvariant()
    platform = 'windows-x64'
    startedAt = [DateTime]::UtcNow.ToString('o')
    finishedAt = $null
    outcome = 'failed'
    installer = [ordered]@{ name = (Split-Path $Installer -Leaf); sha256 = $null; exitCode = $null }
    installation = [ordered]@{ status = 'not-run'; uniqueDirectory = (Split-Path $InstallDirectory -Leaf); executableFound = $false }
    launch = [ordered]@{ status = 'not-run'; processId = $null; readinessAcknowledged = $false; frontendReadyMs = $null; survivedFiveSeconds = $false; exitCode = $null }
    window = [ordered]@{ status = 'not-checked'; expectedTitle = 'TrapClean'; observedTitle = $null; handle = $null; visibleFlag = $false; interactiveSession = [Environment]::UserInteractive }
    screenshot = [ordered]@{ status = 'not-attempted'; file = $null; method = 'user32 PrintWindow into System.Drawing bitmap'; visuallyReviewed = $false; message = $null }
    cleanup = [ordered]@{ process = 'not-needed'; installation = 'left in uniquely owned temporary directory; ephemeral CI runner discards it'; appData = 'new runtime data remains on the ephemeral runner; profile data is never deleted' }
    interactiveUiExercised = $false
    limitations = @('This checks installation, app-owned frontend readiness, process survival and an optional native window. Scanning, providers, cleanup, keyboard and installer wizard interactions are not exercised.')
    error = $null
}
$installerProcess = $null
$appProcess = $null
$probeProcess = $null
$failed = $false

try {
    if (!$env:LOCALAPPDATA) { throw 'LOCALAPPDATA is unavailable; no fresh profile can be established.' }
    $appDataDirectory = Join-Path $env:LOCALAPPDATA 'io.github.trapstarks.trapclean'
    if ((Test-Path -LiteralPath $appDataDirectory) -or (Get-Item -LiteralPath $appDataDirectory -Force -ErrorAction SilentlyContinue)) {
        throw 'Existing TrapClean app data detected. Use a fresh CI runner; no profile or vault contents were read.'
    }
    $metricsDatabase = Join-Path $appDataDirectory 'index.sqlite'
    $python = Get-Command python -CommandType Application -ErrorAction Stop
    $probeScript = Join-Path $PSScriptRoot 'frontend-ready-probe.py'
    if (!(Test-Path -LiteralPath $probeScript -PathType Leaf)) { throw 'The developer readiness probe is missing.' }
    $installerFile = Get-Item -LiteralPath $Installer
    if ($installerFile.PSIsContainer -or $installerFile.Extension -ne '.exe') {
        throw 'Supply the freshly built NSIS .exe installer.'
    }
    $report.installer.sha256 = (Get-FileHash -LiteralPath $installerFile.FullName -Algorithm SHA256).Hash.ToLowerInvariant()

    # A fresh directory does not protect a previously registered app from an NSIS upgrade.
    $uninstallRoots = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKCU:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
    )
    foreach ($registryRoot in $uninstallRoots) {
        foreach ($registeredApp in @(Get-ChildItem -LiteralPath $registryRoot -ErrorAction SilentlyContinue)) {
            $displayName = $registeredApp.GetValue('DisplayName')
            if ($displayName -and $displayName -match '^TrapClean(?:\s|$)') {
                throw 'TrapClean is already registered on this machine. Use a fresh CI runner; the smoke check will not upgrade or uninstall it.'
            }
        }
    }

    $report.installation.status = 'installing'
    # NSIS requires /D to be last and unquoted, even if the directory contains spaces.
    $installerProcess = Start-Process -FilePath $installerFile.FullName -ArgumentList "/S /D=$InstallDirectory" -PassThru
    if (!$installerProcess.WaitForExit(120000)) {
        throw 'The silent installer did not exit within 120 seconds.'
    }
    $report.installer.exitCode = $installerProcess.ExitCode
    if ($installerProcess.ExitCode -ne 0) {
        throw "The silent installer exited with code $($installerProcess.ExitCode)."
    }
    $executable = Join-Path $InstallDirectory 'trapclean.exe'
    if (!(Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw 'NSIS exited successfully but trapclean.exe was not installed in the requested unique directory.'
    }
    $report.installation.status = 'installed'
    $report.installation.executableFound = $true

    if (!('TrapCleanSmoke.NativeWindow' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace TrapCleanSmoke {
    [StructLayout(LayoutKind.Sequential)]
    public struct WindowRect { public int Left; public int Top; public int Right; public int Bottom; }
    public static class NativeWindow {
        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool IsWindowVisible(IntPtr window);
        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool GetWindowRect(IntPtr window, out WindowRect rect);
        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool PrintWindow(IntPtr window, IntPtr context, uint flags);
    }
}
'@
    }

    $appProcess = Start-Process -FilePath $executable -WorkingDirectory $InstallDirectory -PassThru
    $report.launch.status = 'launched'
    $report.launch.processId = $appProcess.Id
    $readyTimer = [Diagnostics.Stopwatch]::StartNew()
    $probeInfo = [Diagnostics.ProcessStartInfo]::new()
    $probeInfo.FileName = $python.Source
    $probeInfo.UseShellExecute = $false
    $probeInfo.CreateNoWindow = $true
    $probeInfo.RedirectStandardOutput = $true
    $probeInfo.RedirectStandardError = $true
    foreach ($argument in @($probeScript, '--database', $metricsDatabase, '--pid', $appProcess.Id.ToString(), '--timeout', '30')) {
        $probeInfo.ArgumentList.Add($argument)
    }
    $probeProcess = [Diagnostics.Process]::new()
    $probeProcess.StartInfo = $probeInfo
    [void]$probeProcess.Start()
    $probeOutput = $probeProcess.StandardOutput.ReadToEndAsync()
    $probeError = $probeProcess.StandardError.ReadToEndAsync()
    $windowHandle = [IntPtr]::Zero
    while ($readyTimer.Elapsed.TotalSeconds -lt 30) {
        $appProcess.Refresh()
        if ($appProcess.HasExited) {
            $report.launch.exitCode = $appProcess.ExitCode
            throw "The launched app exited before readiness (code $($appProcess.ExitCode))."
        }
        $candidateHandle = $appProcess.MainWindowHandle
        if ($candidateHandle -ne [IntPtr]::Zero) {
            $report.window.observedTitle = $appProcess.MainWindowTitle
            $owner = [uint32]0
            [void][TrapCleanSmoke.NativeWindow]::GetWindowThreadProcessId($candidateHandle, [ref]$owner)
            if ($owner -eq $appProcess.Id -and $appProcess.MainWindowTitle -eq 'TrapClean' -and
                [TrapCleanSmoke.NativeWindow]::IsWindowVisible($candidateHandle)) {
                $windowHandle = $candidateHandle
                break
            }
        }
        Start-Sleep -Milliseconds 200
    }
    $probeWait = [Math]::Max(0, 32000 - [int]$readyTimer.ElapsedMilliseconds)
    if (!$probeProcess.WaitForExit($probeWait)) { throw 'The developer frontend-readiness probe exceeded its bounded deadline.' }
    if ($probeProcess.ExitCode -ne 0) { throw 'No frontend-ready acknowledgment matching the launched PID was observed within 30 seconds.' }
    $acknowledgment = $probeOutput.Result | ConvertFrom-Json
    if ($acknowledgment.status -ne 'ready' -or $acknowledgment.pid -ne $appProcess.Id -or $acknowledgment.frontendReadyMs -le 0) {
        throw 'The readiness probe returned an incompatible or mismatched acknowledgment.'
    }
    $report.launch.readinessAcknowledged = $true
    $report.launch.frontendReadyMs = $acknowledgment.frontendReadyMs
    $readyTimer.Stop()
    if ($appProcess.WaitForExit(5000)) {
        $report.launch.exitCode = $appProcess.ExitCode
        throw "The launched app exited during the five-second survival check (code $($appProcess.ExitCode))."
    }
    $report.launch.status = 'native-frontend-ready-and-alive'
    $report.launch.survivedFiveSeconds = $true

    if ($windowHandle -eq [IntPtr]::Zero) {
        $report.window.status = 'unavailable'
        $report.screenshot.status = 'unavailable'
        $report.screenshot.message = 'No visible TrapClean window belonging to the launched process was observed within 30 seconds.'
        $report.limitations += 'Window readiness was not verified. The runner may lack a usable interactive desktop, or the app may not have created its expected window; this run cannot distinguish those causes.'
        $report.outcome = 'partial'
    } else {
        $report.window.status = 'visible'
        $report.window.handle = '0x' + $windowHandle.ToInt64().ToString('X')
        $report.window.visibleFlag = $true
        $report.outcome = 'passed'
        try {
            if (![Environment]::UserInteractive) {
                throw 'The runner process reports a non-interactive session.'
            }
            Add-Type -AssemblyName System.Drawing
            $rect = New-Object TrapCleanSmoke.WindowRect
            if (![TrapCleanSmoke.NativeWindow]::GetWindowRect($windowHandle, [ref]$rect)) {
                throw 'GetWindowRect failed for the app window.'
            }
            $width = $rect.Right - $rect.Left
            $height = $rect.Bottom - $rect.Top
            if ($width -lt 1 -or $height -lt 1 -or $width -gt 8192 -or $height -gt 8192) {
                throw 'The app window has an unavailable or out-of-bounds capture size.'
            }
            $bitmap = [Drawing.Bitmap]::new($width, $height)
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            try {
                $context = $graphics.GetHdc()
                try {
                    $captured = [TrapCleanSmoke.NativeWindow]::PrintWindow($windowHandle, $context, 2)
                } finally {
                    $graphics.ReleaseHdc($context)
                }
                if (!$captured) { throw 'PrintWindow could not capture the app window in this runner session.' }
                $bitmap.Save($screenshotPath, [Drawing.Imaging.ImageFormat]::Png)
                $report.screenshot.status = 'captured-unreviewed'
                $report.screenshot.file = Split-Path $screenshotPath -Leaf
                $report.screenshot.message = 'Real launched-window capture. Inspect the image before claiming that WebView content rendered correctly; PrintWindow may capture a blank or incomplete accelerated surface.'
            } finally {
                $graphics.Dispose()
                $bitmap.Dispose()
            }
        } catch {
            $report.screenshot.status = 'unavailable'
            $report.screenshot.message = $_.Exception.Message
            $report.limitations += 'Window capture was unavailable. No screenshot or interactive visual validation is claimed.'
        }
    }
} catch {
    $failed = $true
    $report.outcome = 'failed'
    $report.error = $_.Exception.Message
} finally {
    foreach ($ownedProcess in @($appProcess, $installerProcess, $probeProcess)) {
        if ($null -eq $ownedProcess) { continue }
        try {
            if (!$ownedProcess.HasExited) {
                $closed = $ownedProcess.CloseMainWindow()
                if (!$closed -or !$ownedProcess.WaitForExit(2000)) {
                    $ownedProcess.Kill()
                    [void]$ownedProcess.WaitForExit(2000)
                    if ($ownedProcess -eq $appProcess) { $report.cleanup.process = 'stopped only the launched app process' }
                } elseif ($ownedProcess -eq $appProcess) {
                    $report.cleanup.process = 'closed only the launched app process'
                }
            } elseif ($ownedProcess -eq $appProcess) {
                $report.cleanup.process = 'launched app had already exited'
            }
        } catch {
            $report.limitations += 'An owned process could not be closed. The ephemeral runner must discard the test environment.'
        }
    }
    $report.finishedAt = [DateTime]::UtcNow.ToString('o')
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding UTF8
    Write-Output "Runtime smoke report: $reportPath"
    Write-Output "Observed outcome: $($report.outcome); interactive UI exercised: false"
}
if ($failed) { exit 1 }
exit 0
