"""Exercise the packaged Conductor against a local provider using a portal-format recipe."""
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
        self.server.calls.append((self.path, request["model"], request["max_tokens"]))
        response = json.dumps({
            "model": request["model"],
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 1},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *args):
        pass


def check(binary, folder, server):
    recipe = {
        "schema": "hypermesh.conductor.recipe.v1",
        "cheap": {"model": "small", "max_output_tokens": 8},
        "frontier": {"model": "coding", "max_output_tokens": 32},
    }
    recipe_bytes = json.dumps(recipe, indent=2).encode()
    (folder / "recipe.json").write_bytes(recipe_bytes)
    tiers = {
        name: {"model": recipe[name]["model"], "auth": "passthrough",
               "base_url": f"http://127.0.0.1:{server.server_port}/v1"}
        for name in ("cheap", "frontier")
    }
    config = folder / "endpoints.json"
    config.write_text(json.dumps({"tiers": tiers, "recipe_file": "recipe.json",
                                  "prefix": {"system": "Coding fixture"}}))
    journal = folder / "journal.jsonl"
    subprocess.run([binary, "run", "--config", config, "--journal", journal,
                    "--task", "visor-package", "--step", "one", "--class", "code",
                    "--message", "Reply ok"], check=True, timeout=15, stdout=subprocess.PIPE)
    assert server.calls == [("/v1/chat/completions", "coding", 32)], server.calls
    row = json.loads(journal.read_text())
    assert row["recipe_sha256"] == hashlib.sha256(recipe_bytes).hexdigest()
    assert row["estimated_input_tokens"] > 0
    assert (row["task_id"], row["input_tokens"], row["output_tokens"]) == ("visor-package", 12, 1)


if __name__ == "__main__":
    with HTTPServer(("127.0.0.1", 0), Provider) as fixture, tempfile.TemporaryDirectory() as staging:
        fixture.calls = []
        worker = threading.Thread(target=fixture.serve_forever, daemon=True)
        worker.start()
        try:
            check(Path(sys.argv[1]).resolve(), Path(staging), fixture)
        finally:
            fixture.shutdown()
            worker.join()
