"""Starts `ghusage tui` in a pseudo terminal, moves through every tab and
quits, then checks that it exited cleanly and restored the terminal. A second
run is ended with SIGTERM and must restore the terminal too.

Usage: python3 scripts/tui-pty-smoke.py <ghusage binary> <data dir>
Unix only (uses the pty module).
"""

import os
import pty
import select
import struct
import sys
import time

import fcntl
import termios


def read_for(fd, seconds):
    out = b""
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            out += data
    return out


def start(binary, data_dir):
    pid, fd = pty.fork()
    if pid == 0:
        os.execv(
            binary,
            [binary, "tui", "--offline", "--user", "octocat", "--data-dir", data_dir],
        )
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    return pid, fd


def check(name, pid, out):
    _, status = os.waitpid(pid, 0)
    code = os.waitstatus_to_exitcode(status)
    text = out.decode("utf-8", "replace")
    problems = []
    if code != 0:
        problems.append(f"exit code {code}")
    if "\x1b[?1049h" not in text:
        problems.append("never entered the alternate screen")
    if "\x1b[?1049l" not in text:
        problems.append("did not leave the alternate screen")
    if "panicked" in text:
        problems.append("panic message in output")
    if problems:
        print(f"TUI smoke ({name}) failed: " + "; ".join(problems))
        print(text[-2000:])
        sys.exit(1)
    print(f"TUI smoke ({name}) passed ({len(out)} bytes of output)")


def main():
    binary, data_dir = sys.argv[1], sys.argv[2]
    pid, fd = start(binary, data_dir)
    out = read_for(fd, 2.0)
    for key in [b"2", b"3", b"4", b"5", b"6", b"1", b"?", b"?"]:
        os.write(fd, key)
        out += read_for(fd, 0.3)
    # A resize is handled without crashing.
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 60, 0, 0))
    os.kill(pid, 28)  # SIGWINCH
    out += read_for(fd, 0.5)
    os.write(fd, b"q")
    out += read_for(fd, 2.0)
    check("quit with q", pid, out)

    pid, fd = start(binary, data_dir)
    out = read_for(fd, 2.0)
    os.kill(pid, 15)  # SIGTERM
    out += read_for(fd, 2.0)
    check("SIGTERM", pid, out)


if __name__ == "__main__":
    main()
