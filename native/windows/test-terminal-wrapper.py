#!/usr/bin/env python3
"""PTY unit contract for the fixed WSL shell wrapper; no model or desktop calls."""
import json
import os
from pathlib import Path
import pty
import re
import select
import subprocess
import tempfile
import termios
import time

source = Path(__file__).with_name("app").joinpath("SessionTerminal.cs").read_text()
literal = re.search(r'internal const string WslScript = ("(?:[^"\\]|\\.)*");', source)
assert literal, "WSL wrapper constant missing"
script = json.loads(literal.group(1))
with tempfile.TemporaryDirectory(prefix="visor-terminal-") as directory:
    root = Path(directory)
    result = root / "result.json"
    mock = root / "cli with spaces $(literal)"
    mock.write_text("#!/usr/bin/env python3\nimport json, os, sys\n"
                    "print('READY', flush=True)\n"
                    "line = sys.stdin.buffer.readline()\n"
                    "with open(os.environ['CONTRACT_OUTPUT'], 'w') as output:\n"
                    "    json.dump({'args': sys.argv[1:], 'bytes': len(line)}, output)\n")
    mock.chmod(0o700)
    master, slave = pty.openpty()
    saved = termios.tcgetattr(slave)
    process = None
    try:
        process = subprocess.Popen(
            ["/bin/sh", "-c", script, "visor-session", str(mock), "00000000-0000-4000-8000-000000000001", "second-instance"],
            stdin=slave, stdout=slave, stderr=slave,
            env={**os.environ, "CONTRACT_OUTPUT": str(result)},
        )
        output = b""
        deadline = time.monotonic() + 10
        while b"READY" not in output:
            assert time.monotonic() < deadline, "wrapper did not start the literal CLI path"
            if select.select([master], [], [], 0.1)[0]:
                output += os.read(master, 4096)
                assert len(output) < 8192, "unexpected wrapper output"
        data = b"x" * 5000 + b"\n"
        while data:
            count = os.write(master, data)
            data = data[count:]
        assert process.wait(timeout=10) == 0, "wrapper failed"
        assert json.loads(result.read_text()) == {
            "args": ["session-terminal", "--raw-terminal", "--session", "00000000-0000-4000-8000-000000000001", "--instance", "second-instance"],
            "bytes": 5001,
        }, "wrapper changed arguments or truncated input"
        assert termios.tcgetattr(slave) == saved, "wrapper did not restore terminal settings"
    finally:
        if process and process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)
print("PASS WSL wrapper literal arguments, long prompt preservation, and terminal restoration")
