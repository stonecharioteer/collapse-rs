#!/usr/bin/env python3
"""Drive a deterministic collapse demo inside a pseudo-terminal."""

from __future__ import annotations

import errno
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time

ROWS = 32
COLS = 110
DEADLINE_SECONDS = 20

# Times are relative to process start. The recording starts before collapse, so its animated logo
# is included. Escape-[Z is Shift-Tab in a terminal.
ACTIONS = [
    (1.55, b"r"),
    (1.85, b"y"),
    (10.00, b"\x1b[Z"),
    (10.35, b"/test\r"),
    (10.85, b"n"),
    (11.70, b"q"),
    (12.00, b"y"),
]

COMMAND = [
    "collapse",
    "demo-task format",
    "sleep-rs 4",
    "demo-task test",
    "demo-task bundle",
]


def set_window_size(fd: int) -> None:
    size = struct.pack("HHHH", ROWS, COLS, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, size)


def main() -> int:
    pid, master = pty.fork()
    if pid == 0:
        environment = {
            "HOME": "/tmp/collapse-demo",
            "USER": "demo",
            "LOGNAME": "demo",
            "SHELL": "/bin/sh",
            "PATH": "/tmp/collapse-demo/bin:/usr/bin:/bin",
            "TERM": "xterm-256color",
            "COLORTERM": "truecolor",
        }
        os.execvpe(COMMAND[0], COMMAND, environment)

    set_window_size(master)
    started = time.monotonic()
    action_index = 0
    child_status: int | None = None

    while time.monotonic() - started < DEADLINE_SECONDS:
        elapsed = time.monotonic() - started
        while action_index < len(ACTIONS) and elapsed >= ACTIONS[action_index][0]:
            os.write(master, ACTIONS[action_index][1])
            action_index += 1

        readable, _, _ = select.select([master], [], [], 0.03)
        if readable:
            try:
                output = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not output:
                break
            os.write(sys.stdout.fileno(), output)

        finished, status = os.waitpid(pid, os.WNOHANG)
        if finished:
            child_status = status
            break
    else:
        os.kill(pid, signal.SIGTERM)

    if child_status is None:
        _, child_status = os.waitpid(pid, 0)

    try:
        while True:
            output = os.read(master, 65536)
            if not output:
                break
            os.write(sys.stdout.fileno(), output)
    except OSError as error:
        if error.errno != errno.EIO:
            raise
    finally:
        os.close(master)

    return os.waitstatus_to_exitcode(child_status)


if __name__ == "__main__":
    raise SystemExit(main())
