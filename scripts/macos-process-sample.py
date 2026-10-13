"""Developer-only sampler. It does not control the app or read user files."""
import argparse
import ctypes
import json
import struct
import subprocess
import sys
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--pid', type=int, required=True)
parser.add_argument('--seconds', type=float, default=10)
parser.add_argument('--output', required=True)
arguments = parser.parse_args()
if sys.platform != 'darwin' or not 0 < arguments.seconds <= 600:
    raise SystemExit('macOS and a sampling duration of 0–600 seconds are required.')
if Path(arguments.output).exists():
    raise SystemExit('Refusing to overwrite measurement evidence.')

library = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
library.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
library.proc_pidinfo.restype = ctypes.c_int

def coalition(pid):
    # Apple's XNU proc_info_private.h: flavor20, two IDs and three reserved u64s.
    buffer = ctypes.create_string_buffer(40)
    if library.proc_pidinfo(pid, 20, 0, buffer, 40) != 40:
        return None
    return struct.unpack('5Q', buffer.raw)[:2]

identity = coalition(arguments.pid)
if not identity:
    raise SystemExit('The operating system did not provide coalition membership; no complete app measurement can be claimed.')

samples = []
started = time.monotonic()
while True:
    selected = []
    for line in subprocess.check_output(['ps', '-axo', 'pid=,rss=,pcpu=,comm='], text=True).splitlines():
        fields = line.strip().split(None, 3)
        if len(fields) != 4:
            continue
        pid, rss, cpu, command = fields
        if coalition(int(pid)) == identity:
            selected.append({'pid': int(pid), 'rssBytes': int(rss) * 1024, 'psCpuPercent': float(cpu), 'name': Path(command).name})
    if not any(row['pid'] == arguments.pid for row in selected):
        break
    samples.append({'seconds': time.monotonic() - started, 'rssBytes': sum(row['rssBytes'] for row in selected), 'psCpuPercent': sum(row['psCpuPercent'] for row in selected), 'processes': selected})
    if time.monotonic() - started >= arguments.seconds:
        break
    time.sleep(0.5)

result = {'method': 'sum ps resident memory and reported CPU for all processes with matching resource and jetsam coalition IDs; RSS includes shared mappings and excludes compressed pages; ps CPU is smoothed, not instantaneous', 'pid': arguments.pid, 'coalition': identity, 'samples': samples, 'peakRssBytes': max((sample['rssBytes'] for sample in samples), default=0), 'averagePsCpuPercent': sum(sample['psCpuPercent'] for sample in samples) / max(1, len(samples))}
Path(arguments.output).write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({key: value for key, value in result.items() if key != 'samples'}))
