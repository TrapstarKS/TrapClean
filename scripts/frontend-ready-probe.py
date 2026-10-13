"""Developer-only read-only probe of a fresh app's local readiness counters."""
import argparse
import json
import sqlite3
import time
from contextlib import closing
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--database', required=True, type=Path)
    parser.add_argument('--pid', required=True, type=int)
    parser.add_argument('--timeout', type=float, default=15)
    arguments = parser.parse_args()
    if arguments.pid <= 0 or not 0 < arguments.timeout <= 30:
        parser.error('Use a positive launched PID and a timeout greater than zero, at most 30 seconds.')
    database = arguments.database.absolute()
    deadline = time.monotonic() + arguments.timeout
    while time.monotonic() < deadline:
        try:
            if database.is_file():
                with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=0.1)) as connection:
                    row = connection.execute('SELECT value FROM runtime_metrics WHERE id=1').fetchone()
                value = json.loads(row[0]) if row else None
                milliseconds = value.get('frontendReadyMs') if isinstance(value, dict) else None
                if isinstance(value, dict) and value.get('pid') == arguments.pid and isinstance(milliseconds, int) and not isinstance(milliseconds, bool) and milliseconds > 0:
                    print(json.dumps({'status': 'ready', 'pid': arguments.pid, 'frontendReadyMs': milliseconds}))
                    return 0
        except (sqlite3.Error, ValueError, TypeError):
            pass
        time.sleep(min(0.2, max(0, deadline - time.monotonic())))
    print(json.dumps({'status': 'not-ready', 'pid': arguments.pid, 'message': 'No matching positive frontend-ready acknowledgment within the timeout.'}))
    return 1


if __name__ == '__main__':
    raise SystemExit(main())
