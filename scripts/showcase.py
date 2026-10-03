#!/usr/bin/env python3
"""Records a showcase video of a derisk session.

Runs `derisk session` nested in a virtual X server (Xvfb), drives it with
real input through xdotool and the agent socket, and records the screen
with ffmpeg. Needs: Xvfb, xdotool, ffmpeg, foot, neofetch, htop, cmatrix,
and a `derisk` built with `--features host` on PATH.

    scripts/showcase.py [OUTPUT.mp4]
"""

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

W, H = 1600, 900
DISPLAY = ":77"
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "derisk-showcase.mp4")

runtime = tempfile.mkdtemp(prefix="derisk-showcase-")
os.chmod(runtime, 0o700)
env = dict(os.environ, DISPLAY=DISPLAY, XDG_RUNTIME_DIR=runtime)
agent = os.path.join(runtime, "derisk", "agent.sock")
pointer = [W // 2, H // 2]


def xdo(*args):
    subprocess.run(["xdotool", *map(str, args)], env=env, check=True)


def request(obj):
    with socket.socket(socket.AF_UNIX) as s:
        s.connect(agent)
        s.sendall((json.dumps(obj) + "\n").encode())
        while not data.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                raise ConnectionError("agent socket closed before sending a response")
            data += chunk
    return json.loads(data)


def state():
    return request({"method": "state"})["result"]


def windows():
    return state()["windows"]


def focused():
    return next(w for w in windows() if w["focused"])


def wait_for(predicate, timeout=10.0):
    end = time.time() + timeout
    while time.time() < end:
        try:
            if predicate():
                return
        except (OSError, StopIteration, KeyError, ValueError):
            pass
        time.sleep(0.1)
    raise TimeoutError("condition not met")


def glide(x, y, duration=0.7):
    """Moves the pointer smoothly (ease in-out) to x, y."""
    x0, y0 = pointer
    steps = max(1, int(duration * 60))
    for i in range(1, steps + 1):
        t = i / steps
        e = 4 * t * t * t if t < 0.5 else 1 - (-2 * t + 2) ** 3 / 2
        xdo("mousemove", int(x0 + (x - x0) * e), int(y0 + (y - y0) * e))
        time.sleep(duration / steps)
    pointer[:] = [x, y]


def click(x, y, duration=0.6):
    glide(x, y, duration)
    time.sleep(0.15)
    xdo("click", 1)


def drag(x0, y0, x1, y1, duration=1.2):
    glide(x0, y0)
    time.sleep(0.2)
    xdo("mousedown", 1)
    glide(x1, y1, duration)
    time.sleep(0.5)
    xdo("mouseup", 1)


def type_text(text, delay=55):
    xdo("type", "--delay", delay, text)


def key(*combos, pause=0.6):
    for combo in combos:
        xdo("key", combo)
        time.sleep(pause)


def title_bar(win):
    f = win["frame"]
    return f["x"] + f["w"] // 2, f["y"] + 14


def main():
    procs = []
    try:
        procs.append(subprocess.Popen(["Xvfb", DISPLAY, "-screen", "0", f"{W}x{H}x24"], env=env))
        time.sleep(1.0)
        log = open(os.path.join(runtime, "session.log"), "w")
        recorder = subprocess.Popen(
            ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
             "-framerate", "30", "-video_size", f"{W}x{H}", "-i", DISPLAY,
             "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p",
             "-movflags", "+faststart", OUT],
            env=env, stdin=subprocess.PIPE)
        procs.append(subprocess.Popen(
            ["derisk", "session", "--size", f"{W}x{H}", "--launch", "foot"],
            env=env, stdout=log, stderr=subprocess.STDOUT))
        wait_for(lambda: len(windows()) == 1)
        time.sleep(2.5)  # startup animation
        xdo("windowfocus", "--sync", subprocess.run(
            ["xdotool", "search", "--name", "^derisk$"], env=env,
            capture_output=True, text=True).stdout.split()[0])

        # 1. A real Wayland app, with derisk's title bar (buttons on the left).
        type_text("neofetch\n")
        time.sleep(2.5)

        # 2. Agent-first: ask the running session from inside the terminal.
        type_text("derisk do open foot and snap it right\n")
        wait_for(lambda: len(windows()) == 2)
        time.sleep(1.5)

        # 3. Snap Assist offers the other half: pick the first terminal.
        click(W // 4, H // 2)
        time.sleep(1.2)
        type_text("htop\n")
        time.sleep(2.5)

        # 4. Drag a title bar to the top edge to maximize, then to a corner.
        left = next(w for w in windows() if w["frame"] and w["frame"]["x"] < W // 2)
        x, y = title_bar(left)
        drag(x, y, W // 2, 2, 1.4)
        time.sleep(1.5)
        x, y = title_bar(focused())
        drag(x, y, W - 3, H - 3, 1.6)
        time.sleep(1.5)

        # 5. Windows-style keyboard snapping.
        key("super+Left", "super+Up", "super+Right", pause=1.0)
        time.sleep(0.6)

        # 6. Tiling layouts: monocle and back to tall.
        key("super+t", pause=0.8)
        key("super+m", pause=1.4)
        key("super+shift+m", pause=1.4)

        # 7. The overview: workspaces, exposé and widgets; ask the assistant.
        key("super", pause=1.5)
        type_text("open foot and move it to workspace 2", 45)
        key("Return", pause=2.5)
        key("Escape", pause=1.0)
        key("super+2", pause=1.0)
        type_text("cmatrix\n")
        time.sleep(3.0)
        key("super+1", pause=1.5)

        # 8. The global menu: the Window menu lives in the top bar.
        click(150, 14)
        time.sleep(1.5)
        key("Escape", pause=0.8)

        # 9. Overview again, pick a window from the exposé grid.
        key("super", pause=1.8)
        click(420, 420)
        time.sleep(2.0)
    finally:
        if "recorder" in locals():
            recorder.communicate(b"q", timeout=30)
        for p in reversed(procs):
            p.terminate()
            p.wait(timeout=10)
        shutil.rmtree(runtime, ignore_errors=True)
    print(OUT)


if __name__ == "__main__":
    main()
