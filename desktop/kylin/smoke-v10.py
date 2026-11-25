"""Exercise the installed GUI and static core on the original V10 baseline."""
import runpy
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import gi

gi.require_version('Gtk', '3.0')
from gi.repository import GLib

launcher = sys.argv[1] if len(sys.argv) > 1 else "/usr/bin/guanhai-bafen"
module = runpy.run_path(launcher, run_name="guanhai_bafen_smoke")
module["configure_identity"]()
app = module["Desktop"]()
assert app.register(None)
app.activate()
window = app.window

def process_events():
    context = GLib.MainContext.default()
    while context.pending():
        context.iteration(False)

def wait_for_completion():
    end = time.monotonic() + 35
    while app.cancel_file is not None and time.monotonic() < end:
        process_events()
        time.sleep(.02)
    assert app.cancel_file is None
    buffer = app.log.get_buffer()
    assert "成功 1 项，失败 0 项" in buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), False)

process_events()
window_id = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--class", module["APP_ID"]]).split()[0]
properties = subprocess.check_output(["xprop", "-id", window_id, "WM_CLASS", "_NET_WM_ICON"])
assert b'"com.guanhai.bafen", "com.guanhai.bafen"' in properties
assert b"_NET_WM_ICON(CARDINAL)" in properties
second = subprocess.Popen([launcher])
deadline = time.monotonic() + 8
while second.poll() is None and time.monotonic() < deadline:
    process_events()
    time.sleep(.02)
if second.poll() is None:
    second.terminate()
    raise AssertionError("Second instance did not reuse the original window")
assert second.wait() == 0 and app.window is window

with tempfile.TemporaryDirectory(prefix="观海 八分 ") as scratch:
    source = Path(scratch) / "输入 文件.txt"
    archive = Path(scratch) / "封装 文件.bfq"
    restored = Path(scratch) / "恢复 文件.txt"
    payload = b"Guanhai Bafen original Kylin V10 ARM64"
    source.write_bytes(payload)
    password = "Only a disposable build test password"
    app.set_inputs([str(source)])
    app.set_output(str(archive))
    app.password.set_text(password)
    app.confirmation.set_text(password)
    app.execute.clicked()
    wait_for_completion()
    assert archive.is_file()

    app.mode.set_active_id("unpack")
    app.set_inputs([str(archive)])
    app.set_output(str(restored))
    app.password.set_text(password)
    app.execute.clicked()
    wait_for_completion()
    assert restored.read_bytes() == payload

window.close()
process_events()
assert app.window is None
print("GTK launch, icon, single instance, round trip, and clean close passed")
