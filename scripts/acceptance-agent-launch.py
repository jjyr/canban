#!/usr/bin/env python3
"""Real Codex parser + multi-thread shell acceptance, with no model requests.

Receives production LaunchSpecs from the ignored Rust acceptance test.
All servers use a temporary CODEX_HOME; no live daemon or Herdr Core is used.
"""
import json
import os
from pathlib import Path
import select
import shlex
import subprocess
import sys
import tempfile
import time


class Server:
    def __init__(self, argv, home):
        home.mkdir(exist_ok=True)
        # Verify injected settings coexist with existing user configuration.
        (home / "config.toml").write_text(
            '[shell_environment_policy]\nexclude = ["PRIVATE_*"]\n'
            '[shell_environment_policy.set]\nPRESERVED_USER_VALUE = "keep"\n'
        )
        env = dict(os.environ, CODEX_HOME=str(home), CANBAN_CARD="daemon-stale",
                   CANBAN_BATCH_ID="daemon-stale", CARD_ID="daemon-stale",
                   PRIVATE_TEST="must-not-leak")
        self.log = (home / "stderr.log").open("w")
        self.proc = subprocess.Popen(argv, env=env, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=self.log, bufsize=0)
        self.buffer = b""
        self.seq = 0
        self.events = []
        self.rpc("initialize", {"clientInfo": {"name": "canban-launch-test", "version": "1"},
                                "capabilities": {"experimentalApi": True}})

    def read(self):
        deadline = time.monotonic() + 30
        while b"\n" not in self.buffer:
            if not select.select([self.proc.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                raise TimeoutError("Codex protocol response timeout")
            data = os.read(self.proc.stdout.fileno(), 65536)
            if not data:
                raise RuntimeError("Codex exited; inspect isolated stderr.log")
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def rpc(self, method, params):
        self.seq += 1
        request_id = self.seq
        self.proc.stdin.write((json.dumps({"id": request_id, "method": method, "params": params}) + "\n").encode())
        self.proc.stdin.flush()
        while True:
            response = self.read()
            if response.get("id") == request_id:
                if "error" in response:
                    raise RuntimeError(response["error"])
                return response["result"]
            self.events.append(response)

    def close(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        self.log.close()


def main():
    specs = json.load(sys.stdin)
    with tempfile.TemporaryDirectory(prefix="canban-launch-acceptance-") as tmp:
        root = Path(tmp)
        configs = []
        for index, spec in enumerate(specs):
            parser = Server(spec["argv"], root / f"parser-{index}")
            try:
                config = parser.rpc("config/read", {"includeLayers": False})["config"]
                policy = {key: value for key, value in config["shell_environment_policy"].items() if value is not None}
                for key, value in spec["env"].items():
                    assert policy["set"][key] == value, (key, policy)
                configs.append({"shell_environment_policy": policy})
            finally:
                parser.close()

        server = Server([specs[0]["argv"][0], "app-server", "--stdio"], root / "shared")
        try:
            threads = []
            for index in [0, 1, 3]:
                result = server.rpc("thread/start", {"cwd": str(root), "config": configs[index],
                                                     "approvalPolicy": "never"})
                threads.append((result["thread"]["id"], index))

            keys = sorted(set().union(*(spec["env"] for spec in specs)) |
                          {"CANBAN_BATCH_ID", "CARD_ID", "PRIVATE_TEST", "PRESERVED_USER_VALUE"})

            def launch_shell(thread, index, output):
                code = "import os,json;print(json.dumps({k:os.environ.get(k) for k in " + repr(keys) + "}))"
                command = shlex.join([sys.executable, "-c", code]) + " > " + shlex.quote(str(output))
                server.rpc("thread/shellCommand", {"threadId": thread, "command": command, "timeoutMs": 10000})

            def finish_shells(count):
                completed = 0
                while completed < count:
                    event = server.events.pop(0) if server.events else server.read()
                    if event.get("method") == "item/completed":
                        item = event["params"]["item"]
                        if item.get("type") == "commandExecution":
                            assert item["exitCode"] == 0, item
                    if event.get("method") == "turn/completed":
                        completed += 1

            for thread, index in threads:
                launch_shell(thread, index, root / f"output-{index}")
            finish_shells(len(threads))

            # Resume the same persisted thread in a new isolated server process,
            # overriding its previous launch values without leaking the other Card.
            first = threads[0][0]
            server.close()
            server = Server([specs[0]["argv"][0], "app-server", "--stdio"], root / "shared")
            server.rpc("thread/resume", {"threadId": first, "config": configs[2],
                                         "cwd": str(root), "approvalPolicy": "never"})
            launch_shell(first, 2, root / "output-2")
            finish_shells(1)
            for index, spec in enumerate(specs):
                actual = json.loads((root / f"output-{index}").read_text())
                expected = {key: spec["env"].get(key) for key in keys}
                expected["PRESERVED_USER_VALUE"] = "keep"
                # Deliberately preserve the caller's inheritance policy. We only
                # override supplied keys; stale-context sanitization is separate.
                for key in ["CANBAN_CARD", "CANBAN_BATCH_ID", "CARD_ID"]:
                    if key not in spec["env"]:
                        expected[key] = "daemon-stale"
                assert actual == expected, (index, actual, expected)
            print("PASS: real Codex parsing, concurrent Card/cardless threads, resumed overrides, unchanged user filters/inheritance")
        finally:
            server.close()


if __name__ == "__main__":
    main()
