# Validation and measured limits

The frozen production source passed **51 native Rust tests** and **13 Chromium UI tests** on macOS Apple Silicon and Windows, and native installer/startup smoke on all three architectures before the delivery commit. [Pre-release CI records](measurements/pre-release-ci.json) identify snapshot `3088e174f7fd840bb093e6990e3c010ec0fae37d` and the tested app-source digest. Exact delivery-commit checksums, source-SHA files and runtime receipts are attached to the [v0.1.0 release](https://github.com/TrapstarKS/TrapClean/releases/tag/v0.1.0). A successful source check or installer build is not an interactive runtime test. Historical performance measurements remain separate below.

## Latest local installed-app QA

The rebuilt local release DMG was installed in Applications and retested on macOS 26.3 arm64. [Latest local QA record](measurements/latest-local-qa.json) identifies source digest `40bdaf96ef64f14021e1c35e9ad6dffbd9787b3ceb40bbfce6d80218a9f823b2` and its hashing method. This is current local evidence, separate from the earlier CI snapshot and large-scale measurements. The declared minimum is macOS 12.3; this run does not test 12.3 itself.

| Observation                    | Latest local generated fixture                                              |
| ------------------------------ | --------------------------------------------------------------------------- |
| Frontend readiness             | 3.597 s; one fresh app process with warm OS caches                          |
| Scan                           | 35 indexed entries; partial coverage from one intentionally skipped symlink |
| Reviewed keyboard confirmation | Exercised                                                                   |
| System Trash result            | 900,000,002 logical bytes moved; 0 permanently deleted                      |
| Measured free-space change     | +4,222,976 B; separate from logical bytes moved to Trash                    |
| Post-cleanup rescan            | 30 entries; source and manifest preserved                                   |
| Verified duplicates            | One group with two independent copies; no automatic selection               |
| Session counters               | 4 progress events/1,031 JSON B; 13 page replies/26,787 JSON B               |

These small-fixture counters do not replace the historical 100k/1m results. Trash movement normally retains allocation; the measured free-space change is an independent observation and does not establish recovery of the moved logical bytes. Current local startup also exceeds the 2 s investigation target.

## Platform and feature coverage

The following results describe the frozen pre-release production source. Native receipts, installer hashes and workflow links are in [pre-release CI records](measurements/pre-release-ci.json). The final release pipeline repeats native tests and installer/startup smoke against its delivery SHA.

| Check                                                                             | macOS Apple Silicon                                                                                          | macOS Intel                                                         | Windows x64                                                                            |
| --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Native Rust tests                                                                 | 51 passed locally and on macOS 15 CI                                                                         | 51 passed on macOS 15 Intel CI                                      | 51 passed on Windows Server 2022 CI                                                    |
| Frontend checks, format, lint, build and Chromium UI tests                        | Passed; 13 UI tests                                                                                          | Frontend build/typecheck passed in packaging                        | Passed; 13 UI tests                                                                    |
| Native credential vault                                                           | Synthetic Keychain write/read/delete passed locally and CI                                                   | Not separately exercised                                            | Synthetic Credential Manager write/read/delete passed in CI                            |
| Installer package and architecture                                                | DMG/app arm64, ad-hoc signature verified                                                                     | DMG/app x86_64, ad-hoc signature verified                           | NSIS current-user installation passed, target x64                                      |
| Native app/frontend startup                                                       | Installed in Applications and tested interactively; CI observed matching frontend-ready PID and 5 s survival | Copied DMG app, matching frontend-ready PID and 5 s survival passed | Installed EXE, matching frontend-ready PID, visible own window and 5 s survival passed |
| Interactive scan, treemap/table, providers, duplicates, preview and Trash results | Passed with generated nonsensitive fixtures                                                                  | Not exercised interactively                                         | First-launch capture visually inspected; no interactive scan/cleanup on this runner    |
| Signing/trust                                                                     | Ad-hoc; not notarized                                                                                        | Ad-hoc; not notarized                                               | Unsigned                                                                               |

The installed-app screenshots show real generated fixtures: [Storage](images/storage.png), [provider settings](images/settings-openai.png), [cleanup preview](images/cleanup-preview.png) and [cleanup result](images/cleanup-result.png). The [Windows first-launch image](images/windows-first-launch.png) is the actual light-appearance runner capture for snapshot `3088e174f7fd840bb093e6990e3c010ec0fae37d`; its rendered content was visually reviewed. Runtime smoke does not establish interactive correctness on every platform.

Pre-release installers measured 4,651,906 B (arm64 DMG), 5,031,687 B (Intel DMG) and 3,867,786 B (Windows EXE). Native frontend acknowledgments were 4.404 s, 3.961 s and 11.088 s respectively. These single CI observations exceed the 2 s investigation target and are not consumer-hardware benchmarks. Final installer sizes and startup observations are recorded with their exact hashes in the release receipts.

The latest local macOS Rust suite also passed **51 tests**. Provider contract tests use real loopback HTTP mock servers and cover proxy bypass, redaction, refusals, malformed responses, authentication/rate limits, cancellation and local fallback. Paid OpenAI, TypeSafe and Custom-provider integration requests were **not run**. Mock contracts do not establish that a paid account or currently available model works.

The final local frontend run passed Svelte/TypeScript checks with zero errors/warnings, formatting, a production Vite build and all **13 Playwright Chromium tests**. Browser tests inject the official Tauri mock API only in the test page; production has no fake filesystem data. They exercise paging/search/sort, immutable cleanup tokens and typed confirmations, duplicate selection, provider configuration and explicit metadata-send confirmation, errors, history, scan completion races, keyboard focus, effective 200% widths and compact desktop layouts. Browser mocks do not replace native WebView installation/runtime testing.

## Local machine and benchmark conditions

- Apple M4 Pro, 12 CPU cores, 24 GiB unified memory.
- macOS 26.3, local APFS storage.
- Rust 1.93.1 and Node.js 24.13.1; Rust release profile.
- One measured run for each scale. Fixture creation immediately preceded scanning, so metadata was warm. No filesystem caches were flushed.
- Generated files contain 32 bytes each, arranged in directories of up to 1,000 files. These measurements emphasize traversal/indexing, not large-file hashing or a real user's workload.

The authoritative local logs are `.local/storage-final-100k.log` and `.local/storage-final-1m.log`. The results below are transcribed from their final JSON records. The [versioned harness](../src-tauri/examples/storage_bench.rs) can reproduce the method without reading or removing user data.

## Rust scanner/index benchmark

These are historical pre-commit measurements from the earlier `6f74f1d3e63f725a6d49aaaf6b9272e627a6e883` build phase, identified by the preserved source digest in [macOS measurements](measurements/macos-arm64.json). They were not rerun for the latest small-fixture QA; their original numbers and digest are unchanged.

| Measurement                                 |  100,000 generated files |  1,000,000 generated files |
| ------------------------------------------- | -----------------------: | -------------------------: |
| Indexed entries, including directories/root |                  100,101 |                  1,001,001 |
| Completion status                           |               `complete` |                 `complete` |
| Scan time, excluding fixture generation     |              11.958669 s |               192.561778 s |
| Indexed entries per second                  |                 8,370.58 |                   5,198.34 |
| Fixture generation, outside scan timer      |                  8.644 s |                  115.340 s |
| Peak benchmark-process resident memory      | 22,102,016 B (21.08 MiB) |   30,883,840 B (29.45 MiB) |
| Primary SQLite file size at measurement     | 67,993,600 B (64.84 MiB) | 683,130,880 B (651.48 MiB) |
| Root query, 50 items                        |              0.186542 ms |                1.000708 ms |
| Aggregated progress snapshots               |                       49 |                        711 |
| Serialized progress-summary JSON            |                 14,660 B |                  215,039 B |
| Cooperative cancellation status             |              `cancelled` |                `cancelled` |
| Cancellation flag → scanner return          |              2.546958 ms |                1.161083 ms |

**Scope matters:** peak RSS is for the Rust benchmark process across its lifetime, including fixture generation and cancellation. It excludes the installed app, WebView and auxiliary processes. The root-page timing covers the Rust index query, not command transport, JavaScript parsing or UI rendering. Progress bytes count serialized summary JSON only; transport framing and table replies are excluded. SQLite sizes cover the main database file containing the completed scan and a small cancellation scan; WAL/SHM companion files and the installed app are excluded.

Cancellation is measured when the harness sets the cooperative flag from a progress callback, then waits for the scanner to return. It is not a measurement from a user's click to a visible UI change. Local OS I/O calls that block on a disconnected drive can delay cancellation. The bounded Rust memory measurements support this fixture's scaling behavior; they are not an app memory guarantee or a cold-cache throughput claim.

## Installed-app performance evidence

This table preserves the earlier `6f74f1d3e63f725a6d49aaaf6b9272e627a6e883` build phase and source digest recorded in [macOS measurements](measurements/macos-arm64.json). The latest local tiny-fixture launch is separately recorded above at 3.597 s; the older 2.615 s and 100k observations are not final-delivery-source measurements.

The initial investigation targets are startup within 2 seconds, total idle app/WebView footprint within 150 MiB and a visible response to cancellation within 2 seconds on normal local I/O. They are targets to investigate, not guaranteed product specifications.

| Installed-app measurement                                 | Evidence recorded in this report                                                                                                                                                                          |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Process-start-to-usable-frontend time                     | 2.615 s for the measured pre-commit local build; fresh process, warm OS caches                                                                                                                            |
| All app/WebView processes: idle resident memory and CPU   | After scan: median 72.97 MiB, peak 73.50 MiB, reported CPU 0%;19 samples over 10.54 s                                                                                                                     |
| All app/WebView processes: scan resident memory and CPU   | Peak 202.09 MiB; average reported CPU 27.06%, peak 70.7%;51 samples over 31.77 s including idle intervals                                                                                                 |
| 100k-entry interactive scan and user-visible cancellation | 100,101 entries complete in approximately 18 s; Settings opened during scan in 1.871 s; stopped/incomplete visible within 1.234 s of Cancel                                                               |
| 1m-entry native UI responsiveness                         | Not run; the 1m Rust scanner benchmark above verifies traversal/index memory, separately from the UI                                                                                                      |
| Full app progress/table JSON volume                       | Completed 100k run: 76 progress payloads/20,410 B; 20 page replies/552,368 B. Excludes other commands, completion event and transport framing                                                             |
| Pre-commit CI installer sizes                             | arm64 DMG 4,650,723 B; Intel DMG 5,029,925 B; Windows EXE 3,871,073 B. Historical source `6f74f1d3e63f725a6d49aaaf6b9272e627a6e883`; final artifact hashes and sizes require delivery-source verification |

These rows use all matching resource/jetsam-coalition processes, including WebContent/GPU/network/audio/theme helpers. RSS includes shared mappings and excludes compressed pages. CPU is the smoothed `ps` value. The 31.77 s scan sample includes time before/after the 18 s scan. The native fixture generator uses 22-byte files, whereas the Rust benchmark uses 32-byte files. Scan timestamps have one-second resolution. UI observation times include automation/capture overhead and are upper bounds, rather than frame-level latency traces.

[Recorded measurements and process samples](measurements/macos-arm64.json) contain the tested source digest and methodology, with process identifiers removed. The initial cold-process launch exceeded the 2 s target. Initial idle sampling of the preceding build had median 110.56 MiB and a 151.72 MiB peak from transient helpers; the measured postscan sample was below 150 MiB. These are observations on one machine, not guaranteed performance limits.

Native checks also exercised keyboard confirmation/focus, minimum 650×492 window scrolling, real duplicate groups with no automatic selection, and the missing-key error without sending a paid request. Native fixture screenshots use macOS dark appearance; the Windows first-launch capture shows native light appearance. Light/dark flows and effective 200% are additionally checked in Chromium; no physical Windows 200% OS-scale run is claimed.

## Reproduce the checks

From a clean checkout with the documented platform development prerequisites:

```sh
npm ci
npm run check
npm run format:check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
npx playwright install chromium
npm run test:ui
```

Run the release scanner benchmark. Each invocation creates and removes its own temporary fixture and local index; fixture generation is reported separately.

```sh
cargo run --release --locked --manifest-path src-tauri/Cargo.toml --example storage_bench -- 100000
cargo run --release --locked --manifest-path src-tauri/Cargo.toml --example storage_bench -- 1000000
```

For native UI checks, create a new directory that does not already exist and choose it in the installed app. The generator refuses to replace an existing directory. Its demonstration files are partly sparse, so logical and allocated sizes intentionally differ.

```sh
node scripts/make-fixture.mjs /tmp/TrapClean-validation-demo
node scripts/make-scale-fixture.mjs /tmp/TrapClean-validation-100k 100000
node scripts/make-scale-fixture.mjs /tmp/TrapClean-validation-1m 1000000
```

On Windows, pass a new directory under the runner's temporary directory instead of `/tmp`. Cleanup tests must select only these generated fixtures. Never use personal files to demonstrate deletion or empty the entire system Trash.

The native vault check uses a unique validation-only service entry and a synthetic value, verifies readback, then deletes that exact entry:

```sh
cargo run --release --locked --manifest-path src-tauri/Cargo.toml --example vault_smoke
```

For a whole-app macOS sample, supply the PID of the installed TrapClean process and a fresh evidence file. The developer-only sampler sums RSS and reported CPU across matching resource/jetsam coalition IDs, including related WebView processes. It checks that the original app remains in the sampled coalition; it does not assume a parent/child process tree contains every WebView service.

```sh
python3 scripts/macos-process-sample.py --pid <APP_PID> --seconds 10 --output <NEW_EVIDENCE_FILE.json>
```

RSS includes shared mappings and excludes compressed pages, so it is not private physical memory. `ps` CPU is smoothed rather than an instantaneous core-usage trace. Record idle and active scan samples separately, preserve the sample data and include the hardware, app source SHA, fixture scale and duration with the result. Python is a developer measurement tool, not an installed-app dependency.

The backend records a single startup-ready acknowledgment after the frontend has rendered a usable frame, plus counts/serialized JSON lengths for progress and paginated replies. Those local SQLite counters do not contain file payloads or credentials, use the backend clock and send no telemetry. Startup readiness is not an OS boot-cold launch measurement; distinguish an app-process cold launch from a cold filesystem cache.

The [installer smoke methodology](installation-smoke-method.md) documents the bounded Windows NSIS install/launch check, real window capture when available, source/hash association and its explicit partial levels. A process surviving five seconds or a visible-window flag does not establish interactive correctness. Both native CI startup methods passed on historical pre-commit source `6f74f1d3e63f725a6d49aaaf6b9272e627a6e883`. These are not new validation results for the latest source. The first fresh frontend acknowledgments were 2.463 s on the macOS arm64 runner, 6.346 s on macOS Intel, and 13.010 s on Windows Server 2022. These single CI observations exceed the 2 s target and are not consumer-hardware benchmarks; further cold-start optimization remains a performance limitation. The Windows probe prevents an early visible-but-empty host window from counting as loaded frontend. Initial PrintWindow capture was blank before this stronger readiness check; the subsequent ready-and-alive capture rendered the actual first screen.

The macOS minimum is 12.3 because the native WebView must support the HTML confirmation dialog and focus styles shipped with [WebKit in Safari 15.4](https://webkit.org/blog/12445/new-webkit-features-in-safari-15-4/). macOS 12.3 itself has not been exercised; observed native runs used macOS 15 CI and macOS 26.3 locally.

## Known limits under test

- Scans are snapshots. Denied access, skipped links/mounts, interrupted work and changed items produce incomplete or stale coverage and restrict cleanup.
- Hashing equal primary contents is insufficient for a file with extra data. Nonempty macOS resource forks, Windows named data streams and uninspectable stream metadata are excluded from duplicate confirmation. Extended metadata and location purpose can still differ between accepted files.
- Hardlinks are not independent duplicate copies. Group/member output is bounded, nothing is selected automatically, and cleanup requires an unchanged independent retained copy.
- Allocation, Trash movement and measured free-space change are different observations. APFS clones, sparse allocation, compression, snapshots, open files and concurrent activity can prevent byte-for-byte recovery.
- macOS ad-hoc signing is not notarization; signing and OS trust warnings remain part of final artifact validation. No paid-provider or unperformed platform runtime result is claimed.
