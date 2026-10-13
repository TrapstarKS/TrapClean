# Installer smoke methodology

This document describes developer and CI checks. It does **not** claim that a script has run successfully on any platform. Look at the JSON report associated with the exact source SHA for observed results. Compiling an installer is separate from installing and running it.

## Windows x64 NSIS check

Run on a fresh Windows runner after building the release installer:

```powershell
pwsh -File scripts/windows-runtime-smoke.ps1 `
  -Installer release-assets/TrapClean_0.1.0_windows-x64.exe `
  -SourceSha $env:GITHUB_SHA `
  -OutputDirectory release-assets
```

The developer check uses PowerShell/.NET, Win32 APIs and a Python standard-library readiness probe supplied by CI. Node, Python, Rust and development tools are not required by the installed application. Neither `pwsh` nor the Python probe is shipped with TrapClean.

The script:

1. Requires Windows, a complete source SHA and a freshly built `.exe`. It records the installer's SHA-256.
2. Refuses an existing registered TrapClean installation, existing smoke evidence, or an existing `%LOCALAPPDATA%\io.github.trapstarks.trapclean` profile before installation. The earlier profile is never opened; this prevents an installer upgrade or readiness query from touching user data.
3. Creates a unique `TrapClean-smoke-<GUID>` destination under `RUNNER_TEMP`, falling back to the system temporary directory. A supplied `-InstallDirectory` must also be a new `TrapClean-smoke-*` directory under that temporary root. An existing destination is refused.
4. Starts the current-user NSIS installer with `/S /D=<unique destination>`. `/D` is the final argument and remains unquoted, as required by NSIS. The script does not request elevation or use `RunAs`. It waits at most 120 seconds for installer exit and requires exit code zero and the expected installed `trapclean.exe`.
5. Launches that installed executable and retains the returned process object. It immediately starts `frontend-ready-probe.py` using a structured `ProcessStartInfo.ArgumentList` with shell execution disabled. The probe opens only the newly created index in SQLite read-only mode, executes `SELECT value FROM runtime_metrics WHERE id=1`, and requires a positive `frontendReadyMs` matching the exact launched PID within 30 seconds. A two-second helper-exit grace bounds the readiness phase to 32 seconds. In parallel, bounded 200 ms checks look for a nonzero main-window handle, exact title `TrapClean`, matching owner process ID and the Win32 visible flag. It does not read keys, scan rows or preferences, and never creates a missing database.
6. Requires the launched process to remain alive for another five seconds after the acknowledgment. Missing or mismatched frontend readiness, early exit, missing installed executable, installer failure or timeout makes the check fail with exit code 1. A visible window or surviving executable alone cannot pass.
7. If a suitable interactive session and window are available, captures **only that window** using `GetWindowRect`, `PrintWindow` and `System.Drawing`. It does not screenshot the entire desktop or manufacture UI data.
8. Closes only the installer, app and Python probe process objects it launched, requesting a normal window close first and stopping those exact processes if needed. It never kills by application name or terminates all WebView/Python processes. The unique install and newly created profile remain for the ephemeral runner to discard; no profile is deleted, no existing installation is uninstalled, and no real user files are scanned or cleaned.

Outputs alongside the installer:

- `runtime-smoke-windows-x64.json`: source SHA, installer hash, installation result, matching-PID frontend readiness, process survival, window observations, screenshot status, cleanup scope and limitations.
- `runtime-smoke-windows-x64.png`: only when the native capture succeeds. This is an actual launched-window capture and still needs visual review.

Each invocation requires a fresh evidence output directory and a new install destination. The default generated destination is unique. Refusing reused directories keeps older artifacts and installations untouched.

## Interpreting the evidence

The JSON separates three observations:

| Observation                                                                      | What it establishes                                                                                 | What it does not establish                                                                                                                |
| -------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `installation.status: installed`                                                 | Silent NSIS install exited zero and the requested executable exists.                                | Installer wizard usability, signing trust, behavior on another machine, or an installation without an already available WebView2 runtime. |
| `launch.status: native-frontend-ready-and-alive` and `survivedFiveSeconds: true` | The exact installed app acknowledged a native WebView frame and stayed alive for five more seconds. | A human-visible healthy rendering, successful scanning, or working interaction.                                                           |
| `window.status: visible`                                                         | A nonzero, correctly titled window owned by the launched process had the Win32 visible flag.        | That a human could see it on the runner's desktop or that its content was rendered correctly.                                             |

`outcome: passed` requires successful installation, a positive matching-PID frontend acknowledgment, five-second survival and the expected visible-window observation. `outcome: partial` means installation, frontend acknowledgment and survival succeeded but no expected visible window was observed. A missing acknowledgment is always a failure, never a partial success. The script returns zero for this documented partial level so a runner's absent desktop does not masquerade as a build failure; it does **not** establish visible-window or interactive GUI validation. A missing window can also reflect an app problem: the report does not assume a runner limitation.

The frontend acknowledgment is sent after Svelte has rendered and a real WebView animation frame has run; it does not inspect pixels or exercise features. A successful capture has status `captured-unreviewed`. Accelerated WebView content can be blank or incomplete in `PrintWindow` output even when the API returns success. Review the image before claiming visual health. Capture failure is reported as `unavailable` with its reason; no fabricated replacement is created. `interactiveUiExercised` remains `false` in every script report.

Interactive smoke testing remains separate: open the installed app, scan a generated temporary fixture, inspect its table and treemap, configure each provider, review cleanup, exercise keyboard focus and scale, and verify per-item results. Destructive checks use only generated temporary fixtures. macOS installation and interactive checks are recorded separately; this Windows script cannot validate them.

## macOS arm64 and Intel DMG check

Run only on a fresh native macOS CI runner after the DMG has been built and its source SHA recorded. For an Intel artifact, change the filename to `macos-x64` and use `--expected-arch x86_64`.

```sh
python3 scripts/macos-runtime-smoke.py \
  --installer release-assets/TrapClean_0.1.0_macos-arm64.dmg \
  --source-sha "$GITHUB_SHA" \
  --expected-arch arm64 \
  --output-dir release-assets
```

This is a developer-only Python standard-library check. Python is supplied by the CI runner and is not required by the installed app. The script does not read a vault, send provider requests, scan user files, or claim interactive UI testing.

Before mounting or launching anything, it refuses an existing TrapClean app-data directory at `~/Library/Application Support/io.github.trapstarks.trapclean`. This includes an existing `index.sqlite`; the script does not open or inspect an earlier profile. It also refuses an existing report. Use a fresh runner rather than deleting a user's app data to make the check pass.

The check:

1. Records the DMG's SHA-256 and supplied complete source SHA. Mounts the image read-only with `hdiutil`, `-nobrowse` and `-noautoopen` at its own unique temporary mountpoint.
2. Requires one real `TrapClean.app` and an `Applications` symlink pointing to `/Applications`. It inspects the executable with `lipo` for the expected native architecture and verifies the bundle's ad-hoc signature using `codesign`. Ad-hoc verification does not establish notarization or Gatekeeper approval.
3. Copies only that app using `ditto` into a newly created temporary installation directory and verifies the copied signature. It does not install into or modify the user's `/Applications`.
4. Launches the copied `Contents/MacOS/trapclean` directly with `Popen`, retaining its exact PID. For at most 15 seconds, it checks process survival and reads only the newly created local `runtime_metrics` row every 200 ms. The row must match that PID and report `frontendReadyMs > 0`; an old record or process survival alone cannot pass.
5. Requires five additional seconds of process survival. The acknowledgment is sent after the real native WebView frontend has completed initialization, a Svelte render tick and an animation frame. It is stronger than checking that an executable stayed alive, but does not prove that a human saw a window or exercised any feature.
6. Terminates only the `Popen` process it launched, detaches only its own image mount, and removes only its uniquely generated temporary directory/app copy. It never removes profile data. Newly created runtime data remains for the ephemeral runner to discard. If its image cannot be detached, it leaves that temporary directory untouched and reports failure.

The result is `runtime-smoke-macos-arm64.json` or `runtime-smoke-macos-x64.json` alongside the installer. It records installer/source hashes, architecture, signature verification, app PID, backend-recorded frontend readiness, survival, cleanup scope and limitations. A missing readiness acknowledgment, early app exit or cleanup failure exits with code 1; there is no successful fallback to a process-only level. No window screenshot is fabricated: `windowInspected`, `screenshotCaptured` and `interactiveUiExercised` remain `false`.

These are methodology statements, not successful run claims. Report the JSON from each actual native runner and source SHA separately. Interactive macOS fixture testing, OS trust prompts and full app/WebView performance measurements remain separate checks.

## References

- [NSIS installer command-line usage](https://nsis.sourceforge.io/Docs/Chapter3.html#installerusage)
- [PowerShell Start-Process](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/start-process)
- [Process main-window handle and refresh](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.mainwindowhandle)
- [Win32 PrintWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-printwindow)
