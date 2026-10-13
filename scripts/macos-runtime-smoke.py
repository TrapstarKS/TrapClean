"""Developer/CI-only DMG install and native frontend readiness smoke check."""
import argparse
import hashlib
import json
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from contextlib import closing
from datetime import datetime, timezone
from pathlib import Path


def command(arguments, timeout=60):
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()[:1000]
        raise RuntimeError(f'{Path(arguments[0]).name} exited {result.returncode}: {detail}')
    return result


def runtime_ready(database, process_id):
    if not database.is_file():
        return None
    try:
        with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=0.1)) as connection:
            row = connection.execute('SELECT value FROM runtime_metrics WHERE id=1').fetchone()
        value = json.loads(row[0]) if row else None
        if isinstance(value, dict) and value.get('pid') == process_id:
            milliseconds = value.get('frontendReadyMs')
            if isinstance(milliseconds, (int, float)) and not isinstance(milliseconds, bool) and milliseconds > 0:
                return milliseconds
    except (sqlite3.Error, ValueError, TypeError):
        pass
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--installer', required=True, type=Path)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--expected-arch', required=True, choices=['arm64', 'x86_64'])
    parser.add_argument('--output-dir', type=Path, default=Path('release-assets'))
    arguments = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('macOS is required; no native runtime result was produced.')
    if not re.fullmatch(r'(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})', arguments.source_sha):
        parser.error('Supply the complete source SHA associated with the installer.')

    label = 'arm64' if arguments.expected_arch == 'arm64' else 'x64'
    arguments.output_dir.mkdir(parents=True, exist_ok=True)
    report_path = arguments.output_dir / f'runtime-smoke-macos-{label}.json'
    if report_path.exists():
        parser.error('Smoke evidence already exists; use a fresh output directory.')
    profile = Path.home() / 'Library/Application Support/io.github.trapstarks.trapclean'
    database = profile / 'index.sqlite'
    report = {
        'schemaVersion': 1,
        'sourceSha': arguments.source_sha.lower(),
        'platform': f'macos-{label}',
        'startedAt': datetime.now(timezone.utc).isoformat(),
        'finishedAt': None,
        'outcome': 'failed',
        'installer': {'name': arguments.installer.name, 'sha256': None},
        'installation': {'status': 'not-run', 'applicationsShortcut': False, 'bundleArchitectures': [], 'signature': None},
        'launch': {'status': 'not-run', 'processId': None, 'frontendReadyMs': None, 'survivedFiveSeconds': False, 'exitCode': None},
        'windowInspected': False,
        'screenshotCaptured': False,
        'interactiveUiExercised': False,
        'cleanup': {'process': 'not-needed', 'mount': 'not-needed', 'temporaryInstall': 'not-created', 'appData': 'profile data is never deleted; fresh runtime data remains on the ephemeral runner'},
        'limitations': ['Checks a copied native bundle and an app-owned WebView-frame readiness acknowledgment. No scanning, provider request, cleanup, keyboard, visible-window inspection or installer drag/drop is exercised. Ad-hoc signing is not notarization or Gatekeeper approval.'],
        'error': None,
    }
    temporary = None
    mount = None
    process = None
    try:
        # Even an otherwise empty existing profile can contain preferences; do not inspect it.
        if profile.exists() or profile.is_symlink():
            raise RuntimeError('Existing TrapClean app data detected. Use a fresh CI runner; no profile or vault contents were read.')
        installer = arguments.installer.resolve(strict=True)
        if not installer.is_file() or installer.suffix.lower() != '.dmg':
            raise RuntimeError('Supply the freshly built DMG installer.')
        digest = hashlib.sha256()
        with installer.open('rb') as handle:
            for block in iter(lambda: handle.read(1024 * 1024), b''):
                digest.update(block)
        report['installer']['sha256'] = digest.hexdigest()
        temporary = Path(tempfile.mkdtemp(prefix='TrapClean-smoke-'))
        mount = temporary / 'mount'
        mount.mkdir()
        command(['/usr/bin/hdiutil', 'attach', str(installer), '-readonly', '-nobrowse', '-noautoopen', '-mountpoint', str(mount)])
        report['cleanup']['mount'] = 'attached to uniquely owned temporary mount'
        bundles = list(mount.glob('*.app'))
        if len(bundles) != 1 or bundles[0].name != 'TrapClean.app' or bundles[0].is_symlink():
            raise RuntimeError('The DMG must contain one real TrapClean.app bundle.')
        shortcut = mount / 'Applications'
        if not shortcut.is_symlink() or shortcut.resolve() != Path('/Applications'):
            raise RuntimeError('The DMG is missing its expected Applications shortcut.')
        report['installation']['applicationsShortcut'] = True
        executable = bundles[0] / 'Contents/MacOS/trapclean'
        architectures = command(['/usr/bin/lipo', '-archs', str(executable)]).stdout.split()
        report['installation']['bundleArchitectures'] = architectures
        if arguments.expected_arch not in architectures:
            raise RuntimeError('The bundle lacks the expected native architecture.')
        command(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(bundles[0])])
        signature = command(['/usr/bin/codesign', '-dv', '--verbose=4', str(bundles[0])])
        if 'Signature=adhoc' not in signature.stderr + signature.stdout:
            raise RuntimeError('The bundle did not report the expected ad-hoc signature.')
        report['installation']['signature'] = 'ad-hoc verified; not notarized'
        installed = temporary / 'install/TrapClean.app'
        installed.parent.mkdir()
        command(['/usr/bin/ditto', str(bundles[0]), str(installed)])
        command(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(installed)])
        report['installation']['status'] = 'copied-to-unique-temporary-install'
        report['cleanup']['temporaryInstall'] = 'created only by this check'
        process = subprocess.Popen([str(installed / 'Contents/MacOS/trapclean')], cwd=installed, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        report['launch']['status'] = 'launched'
        report['launch']['processId'] = process.pid
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if process.poll() is not None:
                report['launch']['exitCode'] = process.returncode
                raise RuntimeError(f'The launched app exited before frontend readiness (code {process.returncode}).')
            milliseconds = runtime_ready(database, process.pid)
            if milliseconds is not None:
                report['launch']['frontendReadyMs'] = milliseconds
                break
            time.sleep(0.2)
        else:
            raise RuntimeError('No frontend-ready acknowledgment matching the launched PID was observed within 15 seconds.')
        try:
            code = process.wait(timeout=5)
            report['launch']['exitCode'] = code
            raise RuntimeError(f'The launched app exited during the additional five-second check (code {code}).')
        except subprocess.TimeoutExpired:
            pass
        report['launch']['status'] = 'native-frontend-ready-and-alive'
        report['launch']['survivedFiveSeconds'] = True
        report['outcome'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        if process is not None:
            try:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
                    report['cleanup']['process'] = 'terminated only the launched Popen process'
                else:
                    report['cleanup']['process'] = 'launched process had already exited'
            except Exception as error:
                report['outcome'] = 'failed'
                report['limitations'].append(f'Could not close the owned process: {error}')
        detached = True
        if mount is not None and mount.is_mount():
            try:
                try:
                    command(['/usr/bin/hdiutil', 'detach', str(mount)], timeout=30)
                except Exception:
                    command(['/usr/bin/hdiutil', 'detach', '-force', str(mount)], timeout=30)
                report['cleanup']['mount'] = 'detached only the owned mount'
            except Exception as error:
                detached = False
                report['outcome'] = 'failed'
                report['limitations'].append(f'Owned mount could not be detached: {error}')
        if mount is not None and mount.is_mount():
            detached = False
            report['outcome'] = 'failed'
            report['limitations'].append('The owned mount remained active; its temporary directory was not traversed or removed.')
        if temporary is not None:
            if detached:
                try:
                    shutil.rmtree(temporary)
                    report['cleanup']['temporaryInstall'] = 'removed only the uniquely created temporary directory and copied app'
                except Exception as error:
                    report['outcome'] = 'failed'
                    report['limitations'].append(f'Owned temporary directory could not be removed: {error}')
            else:
                report['cleanup']['temporaryInstall'] = 'left untouched because the owned image is still mounted'
        report['finishedAt'] = datetime.now(timezone.utc).isoformat()
        report_path.write_text(json.dumps(report, indent=2) + '\n')
        print(f'Runtime smoke report: {report_path}')
        print(f'Observed outcome: {report["outcome"]}; interactive UI exercised: false')
        if report['error']:
            print(f'Failure: {report["error"]}', file=sys.stderr)
    return 0 if report['outcome'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
