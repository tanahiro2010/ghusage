"""Starts `ghusage tui` in a pseudo terminal and checks that it exits cleanly
and restores the terminal, for each way it can end:

- quit with `q` after moving through every tab and a resize,
- SIGTERM,
- SIGHUP.

After each run the terminal must have left the alternate screen, shown the
cursor again after hiding it, and left raw mode (the terminal settings,
including ICANON and ECHO, are back to what they were before the start).

Usage: python3 scripts/tui-pty-smoke.py <ghusage binary> <data dir>
Unix only (uses pseudo terminals and termios).
"""

import os
import select
import signal
import struct
import sys
import time

import fcntl
import termios

# Indexes into the list returned by termios.tcgetattr.
IFLAG, OFLAG, CFLAG, LFLAG = 0, 1, 2, 3

# How long the TUI may take to exit after being asked to.
EXIT_TIMEOUT = 15.0


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


def send(fd, data):
    # Fails once the TUI has gone; the exit check reports that.
    try:
        os.write(fd, data)
    except OSError:
        pass


def signal_child(pid, signum):
    try:
        os.kill(pid, signum)
    except ProcessLookupError:
        pass


def start(binary, data_dir):
    """Runs the TUI on a new pseudo terminal. The settings of the terminal
    side are read through the controlling side, which stays usable after the
    TUI has exited (macOS revokes the terminal side when its session ends).
    The test also keeps the terminal side open, so the pseudo terminal and
    its settings are not torn down when the TUI exits."""
    master, slave = os.openpty()
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    before = termios.tcgetattr(master)
    pid = os.fork()
    if pid == 0:
        try:
            os.close(master)
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            for fd in (0, 1, 2):
                os.dup2(slave, fd)
            if slave > 2:
                os.close(slave)
            os.execv(
                binary,
                [binary, "tui", "--offline", "--user", "octocat", "--data-dir", data_dir],
            )
        except BaseException as e:
            os.write(2, f"cannot start {binary}: {e!r}\n".encode())
        finally:
            os._exit(127)
    return pid, master, slave, before


def wait_for_exit(pid, master):
    """Keeps reading the output while waiting, so the TUI never blocks on a
    full terminal buffer. Kills it if it does not exit in time. Returns the
    output, the exit code and whether it exited by itself."""
    out = b""
    end = time.time() + EXIT_TIMEOUT
    while True:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            return out + read_for(master, 0.3), os.waitstatus_to_exitcode(status), True
        if time.time() >= end:
            os.kill(pid, signal.SIGKILL)
            _, status = os.waitpid(pid, 0)
            return out + read_for(master, 0.3), os.waitstatus_to_exitcode(status), False
        out += read_for(master, 0.1)


def describe_exit(code):
    if code < 0:
        try:
            return f"killed by {signal.Signals(-code).name}"
        except ValueError:
            return f"killed by signal {-code}"
    return f"exit code {code}"


def check(name, pid, master, slave, before, out):
    rest, code, exited = wait_for_exit(pid, master)
    out += rest
    after = termios.tcgetattr(master)
    os.close(master)
    os.close(slave)
    text = out.decode("utf-8", "replace")
    problems = []
    if not exited:
        problems.append(f"did not exit within {EXIT_TIMEOUT:.0f} s (killed)")
    elif code != 0:
        problems.append(describe_exit(code))
    if "\x1b[?1049h" not in text:
        problems.append("never entered the alternate screen")
    if text.rfind("\x1b[?1049l") < text.rfind("\x1b[?1049h"):
        problems.append("did not leave the alternate screen")
    hidden = text.rfind("\x1b[?25l")
    if hidden < 0:
        problems.append("never hid the cursor")
    elif text.rfind("\x1b[?25h") < hidden:
        problems.append("did not show the cursor again")
    for flag in ("ICANON", "ECHO"):
        if not after[LFLAG] & getattr(termios, flag):
            problems.append(f"{flag} is still off (terminal left in raw mode)")
    groups = ((IFLAG, "input"), (OFLAG, "output"), (CFLAG, "control"), (LFLAG, "local"))
    for index, label in groups:
        if after[index] != before[index]:
            problems.append(
                f"{label} flags differ from before the start "
                f"({before[index]:#x} -> {after[index]:#x})"
            )
    if "panicked" in text:
        problems.append("panic message in output")
    if problems:
        print(f"TUI smoke ({name}) failed: " + "; ".join(problems), flush=True)
        print(repr(text[-2000:]), flush=True)
        return False
    print(f"TUI smoke ({name}) passed ({len(out)} bytes of output)", flush=True)
    return True


def run_and_signal(binary, data_dir, signum, name):
    pid, master, slave, before = start(binary, data_dir)
    out = read_for(master, 2.0)
    signal_child(pid, signum)
    return check(name, pid, master, slave, before, out)


def quit_with_q(binary, data_dir):
    pid, master, slave, before = start(binary, data_dir)
    out = read_for(master, 2.0)
    for key in [b"2", b"3", b"4", b"5", b"6", b"1", b"?", b"?"]:
        send(master, key)
        out += read_for(master, 0.3)
    # A resize is handled without crashing.
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 60, 0, 0))
    signal_child(pid, signal.SIGWINCH)
    out += read_for(master, 0.5)
    send(master, b"q")
    return check("quit with q", pid, master, slave, before, out)


def main():
    binary, data_dir = sys.argv[1], sys.argv[2]
    # Every way of ending runs even when an earlier one fails, so one CI run
    # shows all of them.
    results = [
        quit_with_q(binary, data_dir),
        run_and_signal(binary, data_dir, signal.SIGTERM, "SIGTERM"),
        run_and_signal(binary, data_dir, signal.SIGHUP, "SIGHUP"),
    ]
    if not all(results):
        sys.exit(1)


if __name__ == "__main__":
    main()
