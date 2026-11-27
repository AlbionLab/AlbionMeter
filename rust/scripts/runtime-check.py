#!/usr/bin/env python3
"""Checks the real meter (meter-runtime) in a real window on a niri session, with a temporary storage folder and
temporary settings:

- a capture replayed through the meter is saved in the history when the window is closed (clean shutdown);
- a second start on the same storage folder brings the first window to the front and quits;
- the process ends by itself after the window closes.

    python3 rust/scripts/runtime-check.py rust/target/rust-ui/release/meter-app <capture.pcap>
"""
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time

binary, pcap = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
root = tempfile.mkdtemp(prefix='albion-meter-runtime-')
storage, config = os.path.join(root, 'storage'), os.path.join(root, 'settings')
environment = {**os.environ, 'ALBION_METER_CONFIG_DIR': config}
results = []


def check(name, passed, detail):
    results.append(passed)
    print(f"{'PASS' if passed else 'FAIL'} {name}: {detail}")


def niri_windows():
    return json.loads(subprocess.run(['niri', 'msg', '--json', 'windows'], capture_output=True, text=True, check=True).stdout)


def window(pid):
    return next((item for item in niri_windows() if item['pid'] == pid), None)


def wait(condition, seconds):
    end = time.time() + seconds
    while time.time() < end:
        value = condition()
        if value:
            return value
        time.sleep(0.1)
    return None


def records():
    path = os.path.join(storage, 'encounters.db')
    if not os.path.exists(path):
        return 0
    with sqlite3.connect(f'file:{path}?mode=ro', uri=True) as database:
        return database.execute('select count(*) from encounters').fetchone()[0]


print(f'storage and settings in {root}')
first = subprocess.Popen([binary, f'--Storage:Directory={storage}', '--replay', pcap], env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
own = wait(lambda: window(first.pid), 20)
check('the meter opens a window', own is not None, f"window {own and own['id']}")
time.sleep(3)
before = records()

# Another window takes the focus, then a second start must hand it back to the first meter.
others = [item for item in niri_windows() if item['pid'] != first.pid and item['workspace_id'] == own['workspace_id']]
if others:
    subprocess.run(['niri', 'msg', 'action', 'focus-window', '--id', str(others[0]['id'])], check=True)
    time.sleep(0.5)
second = subprocess.run([binary, f'--Storage:Directory={storage}'], env=environment, capture_output=True, text=True, timeout=20)
check('a second start quits', second.returncode == 0 and 'already running' in second.stderr, f'exit {second.returncode}: {second.stderr.strip()}')
# Wayland gives focus only with an activation token; without one the window asks for attention (niri: urgent).
shown = wait(lambda: (lambda item: item and (item['is_focused'] or item['is_urgent']))(window(first.pid)), 3)
state = window(first.pid) or {}
check('the first meter comes to the front or asks for attention', bool(shown) or not others, f"focused {state.get('is_focused')}, urgent {state.get('is_urgent')}")

subprocess.run(['niri', 'msg', 'action', 'close-window', '--id', str(own['id'])], check=True)
try:
    code = first.wait(timeout=20)
except subprocess.TimeoutExpired:
    first.kill()
    code = None
after = records()
check('closing the window ends the meter', code == 0, f'exit code {code}')
check('the replayed record is saved', after > before or after > 0, f'{before} encounter(s) before closing, {after} after')
print(first.stdout.read().strip()[-400:])
print(f"{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
