#!/usr/bin/env python3
"""Checks the frameless meter window for real on a niri session: drag by the top bar, resize from the edges, maximize
(double click and button), minimize, close, and the size remembered in window.json and restored at the next start.

A short-lived virtual absolute pointer (uinput) does the clicks, so the pointer moves on screen for a few seconds;
niri reports the window geometry. The meter runs with --fake-idle and a temporary settings folder.

    python3 rust/scripts/window-check.py rust/target/rust-ui/release/meter-app
"""
import json
import os
import subprocess
import sys
import tempfile
import time

from evdev import AbsInfo, UInput, ecodes as e

binary = os.path.abspath(sys.argv[1])
config = tempfile.mkdtemp(prefix='albion-meter-window-')
print(f'settings and Wayland log in {config}')
results = []


def niri(*args):
    return subprocess.run(['niri', 'msg', *args], capture_output=True, text=True, check=True).stdout


def window(pid):
    for item in json.loads(niri('--json', 'windows')):
        if item['pid'] == pid:
            return item
    return None


def geometry(pid):
    """(x, y, width, height) of the window in the global logical layout, once niri's move/resize animation is over."""
    previous = None
    for _ in range(40):
        current = raw_geometry(pid)
        if current == previous:
            return current
        previous = current
        time.sleep(0.1)
    return previous


def raw_geometry(pid):
    item = window(pid)
    workspaces = {space['id']: space for space in json.loads(niri('--json', 'workspaces'))}
    output = json.loads(niri('--json', 'outputs'))[workspaces[item['workspace_id']]['output']]['logical']
    layout = item['layout']
    dx, dy = layout['window_offset_in_tile']
    width, height = layout['window_size']
    # A maximized window has no position in the workspace view: it fills the output's working area, which ends at the
    # bottom of the output (a bar, if any, is at the top).
    x, y = layout['tile_pos_in_workspace_view'] or ((output['width'] - width) / 2, output['height'] - height)
    return output['x'] + x + dx, output['y'] + y + dy, width, height


outputs = [value['logical'] for value in json.loads(niri('--json', 'outputs')).values() if value.get('logical')]
left, top = min(o['x'] for o in outputs), min(o['y'] for o in outputs)
right, bottom = max(o['x'] + o['width'] for o in outputs), max(o['y'] + o['height'] for o in outputs)
pointer = UInput({e.EV_KEY: [e.BTN_LEFT], e.EV_ABS: [(e.ABS_X, AbsInfo(0, 0, 65535, 0, 0, 0)), (e.ABS_Y, AbsInfo(0, 0, 65535, 0, 0, 0))]}, name='albion-meter-window-check')
time.sleep(1.0)


def move(x, y):
    pointer.write(e.EV_ABS, e.ABS_X, round((x - left) / (right - left) * 65535))
    pointer.write(e.EV_ABS, e.ABS_Y, round((y - top) / (bottom - top) * 65535))
    pointer.syn()
    time.sleep(0.02)


def button(down):
    pointer.write(e.EV_KEY, e.BTN_LEFT, 1 if down else 0)
    pointer.syn()
    time.sleep(0.05)


def click(x, y):
    move(x, y)
    button(True)
    button(False)


def drag(x, y, dx, dy):
    move(x, y)
    button(True)
    for step in range(1, 21):
        move(x + dx * step / 20, y + dy * step / 20)
    button(False)
    time.sleep(0.4)


def check(name, passed, detail):
    results.append((name, passed, detail))
    print(f"{'PASS' if passed else 'FAIL'} {name}: {detail}")


def launch():
    log = open(os.path.join(config, 'wayland.log'), 'a')
    process = subprocess.Popen([binary, '--fake-idle'], env={**os.environ, 'ALBION_METER_CONFIG_DIR': config, 'WAYLAND_DEBUG': 'client'}, stdout=subprocess.DEVNULL, stderr=log)
    for _ in range(100):
        if window(process.pid):
            break
        time.sleep(0.1)
    time.sleep(1.5)
    item = window(process.pid)
    niri('action', 'focus-window', '--id', str(item['id']))
    if not item['is_floating']:
        niri('action', 'toggle-window-floating', '--id', str(item['id']))
    niri('action', 'move-floating-window', '--id', str(item['id']), '-x', '200', '-y', '20')
    time.sleep(0.8)
    return process


def requests(name):
    with open(os.path.join(config, 'wayland.log')) as log:
        return sum(1 for line in log if f'xdg_toplevel#' in line and f'.{name}(' in line and ' -> ' in line)


process = launch()
x, y, width, height = geometry(process.pid)
print(f'window at {x},{y} {width}x{height}')

# Drag by the top bar (the brand is part of the draggable bar, as in the Electron window).
drag(x + 100, y + 26, 160, 90)
x2, y2, _, _ = geometry(process.pid)
check('drag by the top bar', abs(x2 - x - 160) <= 6 and abs(y2 - y - 90) <= 6, f'moved by {x2 - x:.0f},{y2 - y:.0f} (asked 160,90)')
x, y = x2, y2

# Resize from the right edge and from the bottom-right corner.
drag(x + width - 2, y + height / 2, 120, 0)
_, _, width2, height2 = geometry(process.pid)
check('resize from the right edge', abs(width2 - width - 120) <= 6 and height2 == height, f'{width}x{height} -> {width2}x{height2}')
x, y, width, height = geometry(process.pid)
drag(x + 2, y + 2, 100, 60)
x3, y3, width3, height3 = geometry(process.pid)
check('resize from the top-left corner', abs(width - width3 - 100) <= 6 and abs(height - height3 - 60) <= 6 and abs(x3 - x - 100) <= 6, f'{width}x{height} at {x:.0f},{y:.0f} -> {width3}x{height3} at {x3:.0f},{y3:.0f}')
# The minimum size holds (820x560).
x, y, width, height = geometry(process.pid)
drag(x + 2, y + 2, 600, 600)
_, _, width4, height4 = geometry(process.pid)
check('minimum size 820x560', width4 >= 820 and height4 >= 560, f'{width}x{height} -> {width4}x{height4}')

# Double click on the bar maximizes (xdg_toplevel.set_maximized); the maximize button restores (unset_maximized).
x, y, width, height = geometry(process.pid)
click(x + 100, y + 26)
time.sleep(0.15)
click(x + 100, y + 26)
time.sleep(0.8)
check('double click on the bar asks to maximize', requests('set_maximized') >= 1, f'{requests("set_maximized")} set_maximized request(s)')
x, y, width, height = geometry(process.pid)
click(x + width - 46 - 23, y + 26)
time.sleep(0.8)
check('maximize button toggles back', requests('unset_maximized') >= 1, f'{requests("unset_maximized")} unset_maximized request(s)')

# Minimize (niri has no minimized state, so only the request is checked).
x, y, width, height = geometry(process.pid)
click(x + width - 46 * 2 - 23, y + 26)
time.sleep(0.5)
check('minimize button asks to minimize', requests('set_minimized') >= 1, f'{requests("set_minimized")} set_minimized request(s)')

# Close: the process ends and the size is saved.
item = window(process.pid)
niri('action', 'focus-window', '--id', str(item['id']))
time.sleep(0.3)
x, y, width, height = geometry(process.pid)
click(x + width - 23, y + 26)
try:
    code = process.wait(timeout=5)
except subprocess.TimeoutExpired:
    process.kill()
    code = None
saved = json.load(open(os.path.join(config, 'window.json'))) if os.path.exists(os.path.join(config, 'window.json')) else {}
check('close button ends the meter', code == 0, f'exit code {code}')
check('size saved in window.json', saved.get('width') == width and saved.get('height') == height, f'saved {saved}, window was {width}x{height}')

# The next start asks for the saved size. A tiling compositor (niri) still sizes the new window itself, as it does the
# Electron one, so the check is the size the meter asks for.
process = launch()
with open(os.path.join(config, 'wayland.log')) as log:
    restored = [line.strip() for line in log if line.startswith('window: restoring')]
_, _, width5, height5 = geometry(process.pid)
check('saved size asked for at the next start', restored == [f'window: restoring {width}x{height}'], f'{restored}; niri gave {width5}x{height5}')
process.terminate()
process.wait(timeout=5)
pointer.close()
print(f"{sum(passed for _, passed, _ in results)}/{len(results)} passed")
sys.exit(0 if all(passed for _, passed, _ in results) else 1)
