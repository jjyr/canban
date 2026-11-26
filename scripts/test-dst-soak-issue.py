#!/usr/bin/env python3
"""Unit tests for scripts/dst-soak-issue.py."""
from __future__ import annotations

import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

_SPEC = importlib.util.spec_from_file_location(
    "dst_soak_issue",
    pathlib.Path(__file__).with_name("dst-soak-issue.py"),
)
assert _SPEC is not None and _SPEC.loader is not None
dst_soak_issue = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(dst_soak_issue)


def _passing_report(command: str, scenario: str, passed: int, required_total: int = 0) -> dict:
    return {
        "command": command,
        "scenario": scenario,
        "master_seed": 2026 if command == "random" else None,
        "ops": 60 if command == "random" else 40,
        "duration_secs": 300 if command == "random" else None,
        "elapsed_secs": 18.5,
        "passed": passed,
        "failed": 0,
        "timed_out": 0,
        "total": passed,
        "coverage": {
            "distinct_properties": 95,
            "required_passed": required_total,
            "required_total": required_total,
            "coverage_gate_passed": True,
            "never_gate_passed": True,
            "never_violations": [],
            "vacuous_faults": [],
            "armed_vs_fired": [
                {"name": "herdr.after_terminal_run_before_db_commit", "armed": 4, "fired": 4, "vacuous": False},
                {"name": "git.after_push_before_batch_finalize", "armed": 2, "fired": 2, "vacuous": False},
            ],
            "cold_top_10": [{"name": "chaos.auth_revoke_recover", "count": 2}],
            "highlights": {
                "delivery_batch_entered": 14,
                "shell_executed": 27,
                "read_your_writes_checked": 410,
                "at_most_once_checked": 190,
                "committed_durability_checked": 88,
                "natural_quiesce_recovered": 31,
            },
        },
        "failures": [],
    }


class DstSoakIssueTests(unittest.TestCase):
    def test_chaos_failure_with_shrunk_ops_and_multiple_failing_seeds(self) -> None:
        with tempfile.TemporaryDirectory(prefix="dst-soak-issue-") as tmpdir:
            tmp = pathlib.Path(tmpdir)
            corpus_path = tmp / "corpus.json"
            gate_a_path = tmp / "gate-a.json"
            soak_path = tmp / "soak.json"
            title_path = tmp / "title.txt"
            body_path = tmp / "body.md"
            summary_path = tmp / "summary.md"

            corpus_path.write_text(json.dumps(_passing_report("corpus", "corpus", 12)), encoding="utf-8")
            gate_a_path.write_text(json.dumps(_passing_report("run", "all", 18, 79)), encoding="utf-8")

            soak = _passing_report("random", "chaos", 142, 40)
            soak["master_seed"] = 99887766
            soak["failed"] = 2
            soak["total"] = 144
            soak["elapsed_secs"] = 301.25
            soak["failures"] = [
                {
                    "scenario": "chaos",
                    "seed": 7914532405096322517,
                    "ops": 60,
                    "profile": "delivery_and_storage (desktop, full_durability)",
                    "error": (
                        "=== CANBAN DST FAILURE ===\n"
                        "Scenario: chaos\n"
                        "Seed: 7914532405096322517\n"
                        "Ops: 60\n"
                        "Digest: 0x123456789abcdef0\n"
                        "Reason: [primary][remote-a] RYW VIOLATION: Card c99 regressed to rev 2 < 3\n"
                        "Replay: mise run sim -- --seed 7914532405096322517 --scenario chaos --ops 60\n"
                    ),
                    "fingerprint": "RYW VIOLATION: Card # regressed to rev # < #",
                    "digest": "0x123456789abcdef0",
                    "timed_out": False,
                    "shrunk": {
                        "orig_ops": 60,
                        "shrunk_ops": 3,
                        "digest": "0x0fedcba987654321",
                        "ops_lines": [
                            "[00] CreateCard { host: \"remote-a\" }",
                            "[01] Partition { host: \"remote-a\" }",
                            "[02] BounceClient",
                        ],
                    },
                },
                {
                    "scenario": "chaos",
                    "seed": 5215407837432958784,
                    "ops": 60,
                    "profile": "full_mix (mobile)",
                    "error": "OUTBOX STUCK: 1 queued item after quiesce",
                    "fingerprint": "OUTBOX STUCK: # queued item after quiesce",
                    "digest": None,
                    "timed_out": False,
                    "shrunk": {
                        "orig_ops": 60,
                        "shrunk_ops": 5,
                        "digest": None,
                        "ops_lines": ["[00] BounceServer"],
                    },
                },
            ]
            soak_path.write_text(json.dumps(soak), encoding="utf-8")

            rc = dst_soak_issue.main(
                [
                    "--corpus-report",
                    str(corpus_path),
                    "--gate-a-report",
                    str(gate_a_path),
                    "--soak-report",
                    str(soak_path),
                    "--run-url",
                    "https://github.com/jjyr/canban/actions/runs/123456",
                    "--run-number",
                    "42",
                    "--commit",
                    "a2938ba1234567890",
                    "--ref",
                    "main",
                    "--runner",
                    "ubuntu-24.04",
                    "--artifact-name",
                    "dst-soak-logs-42-1",
                    "--out-title",
                    str(title_path),
                    "--out-body",
                    str(body_path),
                    "--out-summary",
                    str(summary_path),
                ]
            )
            self.assertEqual(rc, 0)

            title = title_path.read_text(encoding="utf-8").strip()
            body = body_path.read_text(encoding="utf-8")
            summary = summary_path.read_text(encoding="utf-8")

            self.assertLessEqual(len(title), 120)
            self.assertTrue(title.startswith("[DST] RYW VIOLATION:"))
            self.assertIn("in chaos (seed=7914532405096322517)", title)

            for heading in (
                "## Summary",
                "## Failure Message",
                "## Local Reproduction",
                "## Minimized Repro Sequence",
                "## Additional Failing Seeds",
                "## Coverage & Fault Gate Snapshot",
                "## Checklist",
            ):
                self.assertIn(heading, body)

            self.assertIn("[#42](https://github.com/jjyr/canban/actions/runs/123456)", body)
            self.assertIn("`a2938ba` on `main`", body)
            self.assertIn("`99887766`", body)
            self.assertIn("`7914532405096322517` (`ops=60`)", body)
            self.assertIn("RYW VIOLATION: Card # regressed to rev # < #", body)
            self.assertIn("`dst-soak-logs-42-1`", body)
            self.assertIn(
                "env -u HC_SESSION -u HC_SOCKET -u HC_HOME cargo run --release --locked -p canban-sim --bin canban-sim -- run --scenario chaos --seed 7914532405096322517 --ops 60",
                body,
            )
            self.assertIn(
                "env -u HC_SESSION -u HC_SOCKET -u HC_HOME cargo run --release --locked -p canban-sim --bin canban-sim -- shrink --seed 7914532405096322517 --ops 60",
                body,
            )
            self.assertIn("Minimized: **60 -> 3 ops** | Digest: `0x0fedcba987654321`", body)
            self.assertIn("[00] CreateCard", body)
            self.assertIn("5215407837432958784", body)
            self.assertIn("60 -> 5 ops", body)
            self.assertIn("`2/2` fired, `0` vacuous", body)
            self.assertIn("PASS (`exec.blocked_command=0`)", body)
            self.assertIn("`delivery_batch_entered=14`", body)
            self.assertIn("`shell_executed=27`", body)
            self.assertIn("`read_your_writes_checked=410`", body)
            self.assertIn("`chaos 7914532405096322517 60`", body)
            self.assertIn("FAILED", summary)

    def test_corpus_and_gate_a_failures(self) -> None:
        corpus_fail = _passing_report("corpus", "corpus", 11)
        corpus_fail["failed"] = 1
        corpus_fail["total"] = 12
        corpus_fail["failures"] = [
            {
                "scenario": "chaos",
                "seed": 6581024343186838594,
                "ops": 40,
                "profile": "delivery_and_storage (desktop)",
                "error": "Outbox item permanently marked Failed after revision_conflict",
                "fingerprint": "Outbox item permanently marked Failed after revision_conflict",
                "digest": None,
                "timed_out": False,
                "shrunk": None,
            }
        ]
        gate_a_ok = _passing_report("run", "all", 18, 79)
        soak_ok = _passing_report("random", "chaos", 40, 40)

        title, body = dst_soak_issue.format_issue(
            corpus_report=corpus_fail,
            corpus_err=None,
            gate_a_report=gate_a_ok,
            gate_a_err=None,
            soak_report=soak_ok,
            soak_err=None,
            log_tail="",
            run_url="https://github.com/jjyr/canban/actions/runs/200",
            run_number="200",
            commit="deadbeef123",
            ref="main",
            runner="ubuntu-24.04",
            artifact_name="dst-soak-logs-200-1",
        )
        self.assertEqual(title, "[DST] Scheduled DST corpus failed (chaos seed=6581024343186838594)")
        self.assertNotIn("## Minimized Repro Sequence", body)
        self.assertIn("run --scenario chaos --seed 6581024343186838594 --ops 40", body)
        self.assertIn("Append `chaos 6581024343186838594 40` to `crates/sim/corpus/regressions.txt`", body)

        # Gate A failure (scripted scenario) when corpus and soak passed
        gate_a_fail = _passing_report("run", "all", 17, 79)
        gate_a_fail["failed"] = 1
        gate_a_fail["total"] = 18
        gate_a_fail["failures"] = [
            {
                "scenario": "transport_auth",
                "seed": 2026,
                "ops": 40,
                "profile": None,
                "error": "AuthFailed host dialed 2 times after revoke",
                "fingerprint": "AuthFailed host dialed # times after revoke",
                "digest": "0xabcdef0123456789",
                "timed_out": False,
                "shrunk": None,
            }
        ]
        title_a, body_a = dst_soak_issue.format_issue(
            corpus_report=_passing_report("corpus", "corpus", 12),
            corpus_err=None,
            gate_a_report=gate_a_fail,
            gate_a_err=None,
            soak_report=soak_ok,
            soak_err=None,
            log_tail="",
            run_url="https://github.com/jjyr/canban/actions/runs/201",
            run_number="201",
            commit="deadbeef123",
            ref="main",
            runner="ubuntu-24.04",
            artifact_name="dst-soak-logs-201-1",
        )
        self.assertEqual(title_a, "[DST] Scheduled DST Gate A failed (transport_auth seed=2026)")
        self.assertIn("run --scenario transport_auth --seed 2026", body_a)
        self.assertNotIn("shrink --seed 2026", body_a)

    def test_missing_or_corrupt_json_falls_back_to_log_tail(self) -> None:
        with tempfile.TemporaryDirectory(prefix="dst-soak-fallback-") as tmpdir:
            tmp = pathlib.Path(tmpdir)
            corrupt_path = tmp / "soak.json"
            corrupt_path.write_text("{not valid json", encoding="utf-8")
            log_path = tmp / "dst-soak.log"
            log_lines = [f"log-line-{i:03d}" for i in range(1, 101)]
            log_lines[-1] = "error[E0425]: cannot find value `broken` in this scope"
            log_path.write_text("\n".join(log_lines) + "\n", encoding="utf-8")

            title_path = tmp / "title.txt"
            body_path = tmp / "body.md"
            rc = dst_soak_issue.main(
                [
                    "--corpus-report",
                    str(tmp / "missing-corpus.json"),
                    "--gate-a-report",
                    str(tmp / "missing-gate-a.json"),
                    "--soak-report",
                    str(corrupt_path),
                    "--log-file",
                    str(log_path),
                    "--run-url",
                    "https://github.com/jjyr/canban/actions/runs/300",
                    "--run-number",
                    "77",
                    "--commit",
                    "cafebabe1234",
                    "--ref",
                    "main",
                    "--runner",
                    "ubuntu-24.04",
                    "--artifact-name",
                    "dst-soak-logs-77-1",
                    "--out-title",
                    str(title_path),
                    "--out-body",
                    str(body_path),
                ]
            )
            self.assertEqual(rc, 0)

            title = title_path.read_text(encoding="utf-8").strip()
            body = body_path.read_text(encoding="utf-8")
            self.assertEqual(title, "[DST] Scheduled DST soak failed on main (run #77)")
            self.assertIn("corrupt JSON", body)
            self.assertIn("error[E0425]: cannot find value `broken` in this scope", body)
            self.assertIn("log-line-041", body)
            self.assertNotIn("log-line-010", body)
            self.assertIn("## Checklist", body)

    def test_body_size_truncation_guard_stays_under_60kb_and_caps_long_title(self) -> None:
        soak = _passing_report("random", "chaos", 10, 34)
        soak["failed"] = 1
        soak["total"] = 11
        huge_error = "Reason: " + ("VERY_LONG_ERROR_CLASS_" * 15) + "\n" + ("X" * 90_000)
        soak["failures"] = [
            {
                "scenario": "chaos",
                "seed": 18446744073709551615,
                "ops": 60,
                "profile": "full_mix (desktop)",
                "error": huge_error,
                "fingerprint": "VERY_LONG_ERROR_CLASS",
                "digest": "0x0000000000000001",
                "timed_out": False,
                "shrunk": {
                    "orig_ops": 60,
                    "shrunk_ops": 50,
                    "digest": "0x0000000000000001",
                    "ops_lines": [f"[{i:02d}] " + ("OpDetail " * 80) for i in range(50)],
                },
            }
        ]
        # Add 30 additional failures with multi-byte UTF-8 characters to stress both per-section
        # capping and the final UTF-8 hard byte limit.
        for idx in range(30):
            soak["failures"].append(
                {
                    "scenario": "chaos",
                    "seed": 1000 + idx,
                    "ops": 60,
                    "profile": "full_mix (mobile)",
                    "error": "Failure detail " + ("é" * 500),
                    "fingerprint": "Failure detail " + ("é" * 500),
                    "digest": None,
                    "timed_out": False,
                    "shrunk": None,
                }
            )

        title, body = dst_soak_issue.format_issue(
            corpus_report=_passing_report("corpus", "corpus", 12),
            corpus_err=None,
            gate_a_report=_passing_report("run", "all", 18, 79),
            gate_a_err=None,
            soak_report=soak,
            soak_err=None,
            log_tail="ignored",
            run_url="https://github.com/jjyr/canban/actions/runs/999",
            run_number="999",
            commit="1234567890abcdef",
            ref="main",
            runner="ubuntu-24.04",
            artifact_name="dst-soak-logs-999-1",
        )
        self.assertLessEqual(len(title), 120)
        self.assertTrue(title.endswith(" in chaos (seed=18446744073709551615)"))
        self.assertLess(len(body.encode("utf-8")), 60_000)
        self.assertIn("truncated", body)
        self.assertIn("## Checklist", body)

        # Direct stress test on the final hard UTF-8 guard with an open code fence.
        huge_raw = "## Heading\n\n```text\n" + ("λ" * 35_000) + "\n```\n"
        guarded = dst_soak_issue._enforce_body_limit(huge_raw, "dst-soak-logs-999-1", 59_000)
        self.assertLess(len(guarded.encode("utf-8")), 60_000)
        self.assertEqual(guarded.count("```") % 2, 0, "truncated body must close open code fences")


if __name__ == "__main__":
    unittest.main()
