"""Exercise the packaged Conductor with a recipe exported by the portal editor."""
import os
import signal
import sqlite3
import time
import uuid
import hashlib
import json
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


class Provider(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/tokenize":
            count = 29 if request["messages"][-1]["content"] == "Over window" else 28
            self.server.counts.append(count)
            response = json.dumps({"count": count, "max_model_len": 64}).encode()
        elif self.path == "/v1/chat/completions":
            self.server.calls.append((request["model"], request["max_tokens"]))
            response = json.dumps({
                "model": request["model"],
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"},
                             "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 28, "completion_tokens": 1},
            }).encode()
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *args):
        pass


def check(binary, folder, server):
    recipe_bytes = (Path(__file__).parent / "fixtures/conductor-context-recipe.json").read_bytes()
    recipe = json.loads(recipe_bytes)
    (folder / "recipe.json").write_bytes(recipe_bytes)
    tiers = {
        name: {"model": recipe[name]["model"], "auth": "passthrough", "input_counter": "vllm",
               "base_url": f"http://127.0.0.1:{server.server_port}/v1"}
        for name in ("cheap", "frontier")
    }
    config = folder / "endpoints.json"
    config.write_text(json.dumps({"default_class": "code", "tiers": tiers, "recipe_file": "recipe.json",
                                  "prefix": {"system": "Coding fixture"}}))
    journal = folder / "journal.jsonl"
    subprocess.run([binary, "run", "--config", config, "--journal", journal,
                    "--task", "visor-package", "--step", "one", "--class", "code",
                    "--message", "Reply ok"], check=True, timeout=15, stdout=subprocess.PIPE)
    assert server.calls == [("coding", 16)] and server.counts == [28]
    row = json.loads(journal.read_text())
    assert row["recipe_sha256"] == hashlib.sha256(recipe_bytes).hexdigest()
    assert row["estimated_input_tokens"] > 0 and row["counted_input_tokens"] == 28
    assert (row["task_id"], row["input_tokens"], row["output_tokens"]) == ("visor-package", 28, 1)

    cheap = subprocess.run([binary, "run", "--config", config, "--journal", journal,
                            "--task", "visor-package", "--step", "cheap", "--class", "classify",
                            "--message", "Reply ok"], capture_output=True, timeout=15)
    assert cheap.returncode and b"exceed the effective context window" in cheap.stderr, cheap.stderr
    assert server.calls == [("coding", 16)] and server.counts == [28, 28]

    if len(sys.argv) > 2:
        visor = Path(sys.argv[2]).resolve()
        for stop in (signal.SIGTERM, signal.SIGKILL, None):
            check_managed(visor, binary, folder, config, stop, server)


def process_identity(pid):
    # comm may contain spaces or parentheses; starttime is field 22.
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return fields[0], fields[19]


def process_exited(pid, started):
    try:
        state, current = process_identity(pid)
    except FileNotFoundError:
        return True
    except PermissionError:
        # Loss of access alone must never count as successful shutdown.
        return False
    # Zombies have already released descriptors. PID reuse is a different process.
    return current != started or state in ("Z", "X", "x")


def check_managed(visor, conductor, folder, config, stop, server):
    lease = str(uuid.uuid4())
    journal = folder / (lease + ".jsonl")
    runtime = folder / lease
    env = dict(os.environ, HYPERMESH_CONDUCTOR_CONFIG=str(config),
               HYPERMESH_CONDUCTOR_JOURNAL=str(journal), HYPERMESH_CONDUCTOR_LEASE_ID=lease,
               HYPERMESH_DEFAULT_MODEL="coding", HYPERMESH_RUNTIME_DIR=str(runtime))
    with subprocess.Popen([visor], env=env) as child:
        try:
            for _ in range(200):
                if child.poll() is not None:
                    raise RuntimeError("Visor exited before readiness")
                if (runtime / "control.sock").exists():
                    break
                time.sleep(.05)
            pid, = Path(f"/proc/{child.pid}/task/{child.pid}/children").read_text().split()
            assert Path(f"/proc/{pid}/exe").resolve() == conductor
            _, started = process_identity(pid)
            listener = os.readlink(f"/proc/{pid}/fd/0")
            assert listener.startswith("socket:[")
            assert listener in [os.readlink(fd) for fd in Path(f"/proc/{child.pid}/fd").iterdir()]
            command = [
                "curl", "--fail-with-body", "--silent", "--show-error", "--max-time", "15",
                "--unix-socket", str(runtime / "control.sock"), "-H", "Content-Type: application/json",
                "--data-binary", "@-", "http://localhost/prompt",
            ]
            calls_before = len(server.calls)
            response = subprocess.check_output(command, input=b'{"api_key":"org_fixture_ok","prompt":"Reply ok"}')
            assert json.loads(response)["response"] == "ok"
            row = json.loads(journal.read_text())
            assert row["task_id"] == lease and row["step_id"] and row["model_id"] == "coding"
            assert row["counted_input_tokens"] == 28 and row["input_tokens"] == 28
            assert row["recipe_sha256"] == hashlib.sha256((folder / "recipe.json").read_bytes()).hexdigest()
            # This second prompt fits the frontier context window. Only the
            # combined task budget is exhausted: 29 already used + 44 requested.
            budget = subprocess.run(command, input=b'{"api_key":"org_fixture_ok","prompt":"Next step"}',
                                    capture_output=True, timeout=20)
            assert budget.returncode == 22 and b"Conductor HTTP 409" in budget.stdout, budget
            assert server.counts[-2:] == [28, 28] and len(server.calls) == calls_before + 1
            assert json.loads(journal.read_text()) == row
            with sqlite3.connect(f"file:{journal}.output.sqlite?mode=ro", uri=True) as ledger:
                attempts = ledger.execute("SELECT step,input_charge,charge FROM output_attempts WHERE task=?",
                                          (lease,)).fetchall()
            assert attempts == [(row["step_id"], 28, 1)], attempts
            print(f"lease={lease} step={row['step_id']} used=29 requested=44 limit=72 "
                  f"conductor_http=409 generations={len(server.calls) - calls_before}", flush=True)
            refused = subprocess.run(command, input=b'{"api_key":"org_fixture_ok","prompt":"Over window"}',
                                     capture_output=True, timeout=20)
            assert refused.returncode == 22, refused
            assert server.counts[-2:] == [28, 29] and len(server.calls) == calls_before + 1
            if stop is None:
                os.kill(int(pid), signal.SIGKILL)
                assert child.wait(timeout=5) == 1, "Visor should report managed child failure"
                assert not Path(f"/proc/{pid}").exists(), "Conductor was not reaped"
            else:
                child.send_signal(stop)
                child.wait(timeout=10)
            for _ in range(100):
                if process_exited(pid, started):
                    break
                time.sleep(.05)
            assert process_exited(pid, started), "Conductor did not exit after Visor exited"
        finally:
            if child.poll() is None:
                child.kill()


if __name__ == "__main__":
    with HTTPServer(("127.0.0.1", 0), Provider) as fixture, tempfile.TemporaryDirectory() as staging:
        fixture.calls = []
        fixture.counts = []
        worker = threading.Thread(target=fixture.serve_forever, daemon=True)
        worker.start()
        try:
            check(Path(sys.argv[1]).resolve(), Path(staging), fixture)
        finally:
            fixture.shutdown()
            worker.join()
