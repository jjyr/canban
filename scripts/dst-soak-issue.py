#!/usr/bin/env python3
"""Format GitHub Issues and Actions step summaries for Canban DST soak runs."""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import Any, Optional

MAX_TITLE_CHARS = 120
MAX_BODY_BYTES = 59_000
MAX_ERROR_BLOCK_BYTES = 24_000
MAX_OPS_BLOCK_BYTES = 12_000
MAX_LOG_TAIL_LINES = 60
MAX_ADDITIONAL_FAILURES = 25

_LEADING_BRACKETS_RE = re.compile(r"^(?:\[[^\]]+\]\s*)+")


def load_report(path: Optional[str]) -> tuple[Optional[dict[str, Any]], Optional[str]]:
    """Load a JSON report file, returning (data, error_reason)."""
    if not path:
        return None, "not provided"
    p = pathlib.Path(path)
    if not p.exists():
        return None, f"missing ({path})"
    try:
        raw = p.read_text(encoding="utf-8", errors="replace")
        data = json.loads(raw)
        if not isinstance(data, dict):
            return None, f"invalid JSON root in {path}"
        return data, None
    except (OSError, json.JSONDecodeError) as exc:
        return None, f"corrupt JSON in {path}: {exc}"


def read_log_tail(path: Optional[str], max_lines: int = MAX_LOG_TAIL_LINES) -> str:
    """Return the last `max_lines` lines from `--log-file`, or an empty string."""
    if not path:
        return ""
    p = pathlib.Path(path)
    if not p.exists():
        return ""
    try:
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""
    if not lines:
        return ""
    return "\n".join(lines[-max_lines:])


def _clean_single_line(text: str) -> str:
    return " ".join(text.replace("\r", " ").replace("\n", " ").split())


def _escape_table_cell(text: str) -> str:
    return _clean_single_line(text).replace("|", "\\|")


def _fence_block(text: str, lang: str = "text") -> str:
    """Wrap `text` in a Markdown fenced code block that cannot be broken by backticks inside `text`."""
    longest_run = 0
    for match in re.finditer(r"`+", text):
        longest_run = max(longest_run, len(match.group(0)))
    fence = "`" * max(3, longest_run + 1)
    content = text.rstrip("\n")
    return f"{fence}{lang}\n{content}\n{fence}"


def _truncate_utf8(text: str, max_bytes: int, suffix: str = "") -> str:
    """Truncate `text` safely at a UTF-8 character boundary so `result + suffix` fits in `max_bytes`."""
    encoded = text.encode("utf-8")
    if len(encoded) <= max_bytes:
        return text
    suffix_bytes = suffix.encode("utf-8")
    budget = max(0, max_bytes - len(suffix_bytes))
    trimmed = encoded[:budget].decode("utf-8", errors="ignore").rstrip()
    return f"{trimmed}{suffix}"


def _truncate_title(prefix: str, middle: str, suffix: str, max_len: int = MAX_TITLE_CHARS) -> str:
    full = f"{prefix}{middle}{suffix}"
    if len(full) <= max_len:
        return full
    avail = max_len - len(prefix) - len(suffix)
    if avail >= 4:
        cut = middle[: avail - 3].rstrip(" .,:;-")
        if not cut:
            cut = middle[: avail - 3]
        return f"{prefix}{cut}...{suffix}"
    return f"{full[: max_len - 3].rstrip()}..."


def extract_headline(error: str, fingerprint: str = "") -> str:
    """Extract a concise single-line headline from a simulation error string."""
    if error:
        lines = [line.strip() for line in error.splitlines() if line.strip()]
        chosen = ""
        for line in lines:
            if line.startswith("Reason:"):
                candidate = line[len("Reason:") :].strip()
                if candidate:
                    chosen = candidate
                    break
        if not chosen:
            for line in lines:
                if line.startswith("===") and line.endswith("==="):
                    continue
                chosen = line
                break
        if chosen:
            stripped = _LEADING_BRACKETS_RE.sub("", chosen).strip()
            headline = _clean_single_line(stripped or chosen)
            if headline:
                return headline
    if fingerprint:
        return _clean_single_line(fingerprint)
    return "simulation failure"


def report_has_failure(report: Optional[dict[str, Any]], rc: Optional[int] = None) -> bool:
    if rc is not None and rc != 0:
        return True
    if not report:
        return False
    if int(report.get("failed") or 0) > 0:
        return True
    if int(report.get("timed_out") or 0) > 0:
        return True
    if report.get("failures"):
        return True
    cov = report.get("coverage") or {}
    if cov.get("coverage_gate_passed") is False:
        return True
    if cov.get("never_gate_passed") is False:
        return True
    return False


def coverage_failure_summary(report: dict[str, Any]) -> str:
    cov = report.get("coverage") or {}
    reasons: list[str] = []
    if cov.get("never_gate_passed") is False:
        violations = cov.get("never_violations") or []
        detail = ", ".join(f"{v.get('name')}={v.get('count')}" for v in violations) or "violation"
        reasons.append(f"Never-property gate failed ({detail})")
    vacuous = cov.get("vacuous_faults") or []
    if vacuous and cov.get("coverage_gate_passed") is False:
        reasons.append(f"Vacuous armed faults ({', '.join(vacuous)})")
    req_passed = int(cov.get("required_passed") or 0)
    req_total = int(cov.get("required_total") or 0)
    if cov.get("coverage_gate_passed") is False and req_total > 0 and req_passed < req_total:
        reasons.append(f"Required coverage {req_passed}/{req_total}")
    if not reasons:
        reasons.append("Coverage gate failed")
    return "; ".join(reasons)


def _format_phase_result_line(report: Optional[dict[str, Any]], load_err: Optional[str], rc: Optional[int]) -> str:
    if report is None:
        if rc is not None and rc != 0:
            return f"FAIL (exit {rc}; report {load_err})"
        if load_err and load_err != "not provided":
            return f"MISSING ({load_err})"
        return "NOT RUN"
    passed = int(report.get("passed") or 0)
    failed = int(report.get("failed") or 0)
    timed_out = int(report.get("timed_out") or 0)
    total = int(report.get("total") or (passed + failed + timed_out))
    elapsed = float(report.get("elapsed_secs") or 0.0)
    cov = report.get("coverage") or {}
    status = "FAIL" if report_has_failure(report, rc) else "PASS"
    extras: list[str] = []
    if timed_out > 0:
        extras.append(f"{timed_out} timed out")
    req_total = int(cov.get("required_total") or 0)
    req_passed = int(cov.get("required_passed") or 0)
    if req_total > 0:
        extras.append(f"coverage {req_passed}/{req_total}")
    if cov.get("never_gate_passed") is False:
        extras.append("never-gate FAIL")
    vacuous = cov.get("vacuous_faults") or []
    if vacuous and cov.get("coverage_gate_passed") is False:
        extras.append(f"{len(vacuous)} vacuous")
    extra_str = f", {'; '.join(extras)}" if extras else ""
    return f"{status} ({passed}/{total} passed, {failed} failed{extra_str} in {elapsed:.1f}s)"


def _format_repro_commands(
    primary_phase: str,
    primary_report: Optional[dict[str, Any]],
    primary_failure: Optional[dict[str, Any]],
) -> str:
    base = "env -u HC_SESSION -u HC_SOCKET -u HC_HOME cargo run --release --locked -p canban-sim --bin canban-sim --"
    cmds: list[str] = []
    if primary_failure is not None:
        scenario = str(primary_failure.get("scenario") or "chaos")
        seed = primary_failure.get("seed")
        ops = primary_failure.get("ops")
        if scenario == "chaos":
            ops_part = f" --ops {ops}" if ops is not None else ""
            cmds.append(f"{base} run --scenario chaos --seed {seed}{ops_part}")
            cmds.append(f"{base} shrink --seed {seed}{ops_part}")
        else:
            cmds.append(f"{base} run --scenario {scenario} --seed {seed}")
    elif primary_phase == "soak" and primary_report is not None:
        ms = primary_report.get("master_seed")
        dur = primary_report.get("duration_secs") or 300
        ops = primary_report.get("ops") or 60
        ms_flag = f" --master-seed {ms}" if ms is not None else ""
        cmds.append(
            f"{base} random --scenario chaos --duration {dur} --jobs 4 --ops {ops}{ms_flag} --seed-timeout 60 --assert-coverage --auto-shrink"
        )
    elif primary_phase == "corpus":
        cmds.append(f"{base} corpus --jobs 4 --seed-timeout 60")
    elif primary_phase == "Gate A":
        cmds.append(f"{base} run --seeds 1 --assert-coverage --seed-timeout 60")
    else:
        cmds.append(f"{base} corpus --jobs 4 --seed-timeout 60")
        cmds.append(f"{base} run --seeds 1 --assert-coverage --seed-timeout 60")
    return "\n".join(cmds)


def _enforce_body_limit(body: str, artifact_name: str, max_bytes: int = MAX_BODY_BYTES) -> str:
    """Hard UTF-8 byte-length guard guaranteeing the rendered Markdown stays below 60,000 bytes."""
    if len(body.encode("utf-8")) <= max_bytes:
        return body
    art_hint = f" (see artifact `{artifact_name}` for full logs)" if artifact_name else ""
    notice = f"\n\n... [truncated to stay within GitHub issue body limit{art_hint}]\n"
    # Reserve space in case we need to close an unclosed code fence before the notice.
    reserve = len("\n````\n".encode("utf-8")) + len(notice.encode("utf-8"))
    cut = _truncate_utf8(body, max(0, max_bytes - reserve))
    # Close any open fenced code block cleanly.
    fence_open: Optional[str] = None
    for line in cut.splitlines():
        stripped = line.lstrip()
        m = re.match(r"^(`{3,})", stripped)
        if m:
            ticks = m.group(1)
            if fence_open is None:
                fence_open = ticks
            elif len(ticks) >= len(fence_open) and stripped[len(ticks) :].strip() == "":
                fence_open = None
    closing = f"\n{fence_open}" if fence_open is not None else ""
    result = f"{cut}{closing}{notice}"
    if len(result.encode("utf-8")) > max_bytes:
        result = _truncate_utf8(result, max_bytes)
    return result


def format_issue(
    *,
    corpus_report: Optional[dict[str, Any]],
    corpus_err: Optional[str],
    gate_a_report: Optional[dict[str, Any]],
    gate_a_err: Optional[str],
    soak_report: Optional[dict[str, Any]],
    soak_err: Optional[str],
    log_tail: str,
    run_url: str,
    run_number: str,
    commit: str,
    ref: str,
    runner: str,
    artifact_name: str,
    corpus_rc: Optional[int] = None,
    gate_a_rc: Optional[int] = None,
    soak_rc: Optional[int] = None,
) -> tuple[str, str]:
    """Render `(title, body)` for a failed DST soak workflow run."""
    soak_failed = report_has_failure(soak_report, soak_rc)
    corpus_failed = report_has_failure(corpus_report, corpus_rc)
    gate_a_failed = report_has_failure(gate_a_report, gate_a_rc)

    primary_phase = "fallback"
    primary_report: Optional[dict[str, Any]] = None
    primary_failure: Optional[dict[str, Any]] = None

    if soak_failed and soak_report is not None:
        primary_phase = "soak"
        primary_report = soak_report
        failures = soak_report.get("failures") or []
        if failures:
            primary_failure = failures[0]
    elif corpus_failed and corpus_report is not None:
        primary_phase = "corpus"
        primary_report = corpus_report
        failures = corpus_report.get("failures") or []
        if failures:
            primary_failure = failures[0]
    elif gate_a_failed and gate_a_report is not None:
        primary_phase = "Gate A"
        primary_report = gate_a_report
        failures = gate_a_report.get("failures") or []
        if failures:
            primary_failure = failures[0]

    # 1. Title
    if primary_phase == "soak" and primary_report is not None:
        if primary_failure is not None:
            sc = str(primary_failure.get("scenario") or "chaos")
            seed = primary_failure.get("seed")
            headline = extract_headline(
                str(primary_failure.get("error") or ""),
                str(primary_failure.get("fingerprint") or ""),
            )
            title = _truncate_title("[DST] ", headline, f" in {sc} (seed={seed})")
        else:
            cov_reason = coverage_failure_summary(primary_report)
            ms = primary_report.get("master_seed")
            suffix = f" in chaos (master_seed={ms})" if ms is not None else " in chaos"
            title = _truncate_title("[DST] ", cov_reason, suffix)
    elif primary_phase in ("corpus", "Gate A") and primary_report is not None:
        if primary_failure is not None:
            detail = f"{primary_failure.get('scenario', 'unknown')} seed={primary_failure.get('seed', '?')}"
        else:
            detail = coverage_failure_summary(primary_report)
        title = _truncate_title(f"[DST] Scheduled DST {primary_phase} failed (", detail, ")")
    else:
        ref_label = ref.strip() or "unknown"
        run_label = f"run #{run_number}" if run_number else "run #unknown"
        title = _truncate_title("[DST] Scheduled DST soak failed on ", ref_label, f" ({run_label})")

    # 2. Gather additional failures across all reports (excluding the exact primary failure slot).
    additional_failures: list[tuple[str, dict[str, Any]]] = []
    phase_sources = [
        ("soak", soak_report),
        ("corpus", corpus_report),
        ("Gate A", gate_a_report),
    ]
    for phase_name, rep in phase_sources:
        if not rep:
            continue
        for idx, item in enumerate(rep.get("failures") or []):
            if phase_name == primary_phase and idx == 0 and primary_failure is not None:
                continue
            additional_failures.append((phase_name, item))

    # 3. Summary table values
    run_cell = f"[#{run_number}]({run_url})" if (run_number and run_url) else (f"#{run_number}" if run_number else (run_url or "n/a"))
    short_sha = commit[:7] if commit else "n/a"
    commit_cell = f"`{short_sha}` on `{ref}`" if ref else f"`{short_sha}`"
    runner_cell = f"`{runner}`" if runner else "n/a"

    if primary_failure is not None:
        sc_name = str(primary_failure.get("scenario") or "unknown")
        profile = primary_failure.get("profile")
        phase_scenario_cell = f"`{primary_phase}` / `{sc_name}` (`{profile}`)" if profile else f"`{primary_phase}` / `{sc_name}`"
    elif primary_phase != "fallback":
        sc_name = str((primary_report or {}).get("scenario") or primary_phase)
        phase_scenario_cell = f"`{primary_phase}` / `{sc_name}` (gate failure)"
    else:
        phase_scenario_cell = "`build / crash` (no failing JSON report)"

    master_seed = (soak_report or {}).get("master_seed")
    if master_seed is None and primary_report is not None:
        master_seed = primary_report.get("master_seed")
    master_seed_cell = f"`{master_seed}`" if master_seed is not None else "n/a"

    if primary_failure is not None:
        seed_val = primary_failure.get("seed")
        ops_val = primary_failure.get("ops")
        timeout_tag = " **TIMEOUT**" if primary_failure.get("timed_out") else ""
        failing_seed_cell = f"`{seed_val}` (`ops={ops_val}`){timeout_tag}"
        fp = str(primary_failure.get("fingerprint") or "").strip()
        fingerprint_cell = f"`{_escape_table_cell(fp)}`" if fp else "n/a"
    else:
        failing_seed_cell = "n/a"
        fingerprint_cell = "n/a"

    if soak_report is not None:
        soak_result_cell = _format_phase_result_line(soak_report, soak_err, soak_rc)
    elif primary_report is not None:
        soak_result_cell = f"{primary_phase}: {_format_phase_result_line(primary_report, None, None)}"
    else:
        soak_result_cell = "No JSON report produced"

    artifact_cell = f"`{artifact_name}` (retained 14 days)" if artifact_name else "n/a"

    sections: list[str] = []
    sections.append(
        "\n".join(
            [
                "## Summary",
                "",
                "| Field | Value |",
                "| --- | --- |",
                f"| Workflow Run | {run_cell} |",
                f"| Commit | {commit_cell} |",
                f"| Runner | {runner_cell} |",
                f"| Phase / Scenario | {_escape_table_cell(phase_scenario_cell)} |",
                f"| Master Seed | {master_seed_cell} |",
                f"| Failing Seed | {_escape_table_cell(failing_seed_cell)} |",
                f"| Soak Result | {_escape_table_cell(soak_result_cell)} |",
                f"| Fingerprint | {fingerprint_cell} |",
                f"| Artifacts | {_escape_table_cell(artifact_cell)} |",
            ]
        )
    )

    # 4. Failure Message
    if primary_failure is not None:
        raw_err = str(primary_failure.get("error") or "(empty error message)")
        capped_err = _truncate_utf8(
            raw_err,
            MAX_ERROR_BLOCK_BYTES,
            "\n... [error message truncated; see full log in workflow artifact]",
        )
        sections.append(f"## Failure Message\n\n{_fence_block(capped_err, 'text')}")
    elif primary_report is not None:
        cov_msg = coverage_failure_summary(primary_report)
        sections.append(f"## Failure Message\n\n{_fence_block(cov_msg, 'text')}")
    else:
        notes: list[str] = [
            "No structured failure JSON report was produced (build failure, runner crash, or corrupt report)."
        ]
        for label, err_txt in (("corpus", corpus_err), ("Gate A", gate_a_err), ("soak", soak_err)):
            if err_txt and err_txt != "not provided":
                notes.append(f"- {label} report: {err_txt}")
        if log_tail:
            capped_tail = _truncate_utf8(
                log_tail,
                MAX_ERROR_BLOCK_BYTES,
                "\n... [log tail truncated; see workflow artifact]",
            )
            sections.append(
                "## Failure Message\n\n"
                + "\n".join(notes)
                + f"\n\nLast {MAX_LOG_TAIL_LINES} lines of workflow log:\n\n"
                + _fence_block(capped_tail, "text")
            )
        else:
            sections.append("## Failure Message\n\n" + "\n".join(notes))

    # 5. Local Reproduction
    repro_cmds = _format_repro_commands(primary_phase, primary_report, primary_failure)
    sections.append(f"## Local Reproduction\n\n{_fence_block(repro_cmds, 'bash')}")

    # 6. Minimized Repro Sequence (when shrunk is present)
    shrunk = (primary_failure or {}).get("shrunk")
    if isinstance(shrunk, dict):
        orig_ops = shrunk.get("orig_ops", "?")
        shrunk_ops = shrunk.get("shrunk_ops", "?")
        digest = shrunk.get("digest")
        ops_lines = shrunk.get("ops_lines") or []
        header_line = f"Minimized: **{orig_ops} -> {shrunk_ops} ops**"
        if digest:
            header_line += f" | Digest: `{digest}`"
        ops_text = "\n".join(str(line) for line in ops_lines) if ops_lines else "(empty op sequence)"
        capped_ops = _truncate_utf8(
            ops_text,
            MAX_OPS_BLOCK_BYTES,
            "\n... [minimized op sequence truncated]",
        )
        sections.append(f"## Minimized Repro Sequence\n\n{header_line}\n\n{_fence_block(capped_ops, 'text')}")

    # 7. Additional Failing Seeds
    if additional_failures:
        rows = [
            "## Additional Failing Seeds",
            "",
            "| Phase | Scenario | Seed | Ops | Shrunk | Fingerprint / Error |",
            "| --- | --- | --- | --- | --- | --- |",
        ]
        shown = additional_failures[:MAX_ADDITIONAL_FAILURES]
        for phase_name, item in shown:
            sc = str(item.get("scenario") or "unknown")
            seed = item.get("seed", "?")
            ops = item.get("ops", "?")
            s_info = item.get("shrunk")
            shrunk_cell = (
                f"{s_info.get('orig_ops', '?')} -> {s_info.get('shrunk_ops', '?')} ops"
                if isinstance(s_info, dict)
                else "-"
            )
            fp = str(item.get("fingerprint") or "").strip()
            headline = extract_headline(str(item.get("error") or ""), fp)
            summary = fp if fp else headline
            if len(summary) > 90:
                summary = summary[:87].rstrip() + "..."
            if item.get("timed_out"):
                summary = f"[TIMEOUT] {summary}"
            rows.append(
                f"| `{phase_name}` | `{sc}` | `{seed}` | `{ops}` | `{shrunk_cell}` | `{_escape_table_cell(summary)}` |"
            )
        remaining = len(additional_failures) - len(shown)
        if remaining > 0:
            rows.append("")
            rows.append(f"_...and {remaining} more failing seed(s); see workflow artifact `{artifact_name}`._")
        sections.append("\n".join(rows))

    # 8. Coverage & Fault Gate Snapshot
    cov_lines = [
        "## Coverage & Fault Gate Snapshot",
        "",
        f"- **Corpus**: {_format_phase_result_line(corpus_report, corpus_err, corpus_rc)}",
        f"- **Gate A**: {_format_phase_result_line(gate_a_report, gate_a_err, gate_a_rc)}",
        f"- **Chaos Soak**: {_format_phase_result_line(soak_report, soak_err, soak_rc)}",
    ]
    cov_source_name = ""
    cov_source: Optional[dict[str, Any]] = None
    for name, rep in (("Chaos Soak", soak_report), ("Gate A", gate_a_report), ("Corpus", corpus_report)):
        if rep is not None and isinstance(rep.get("coverage"), dict):
            cov_source_name = name
            cov_source = rep["coverage"]
            break

    if cov_source is not None:
        armed_rows = cov_source.get("armed_vs_fired") or []
        fired_faults = sum(1 for r in armed_rows if int(r.get("fired") or 0) > 0)
        total_faults = len(armed_rows)
        vacuous_list = cov_source.get("vacuous_faults") or []
        vacuous_detail = f" (`{', '.join(vacuous_list)}`)" if vacuous_list else ""

        never_ok = cov_source.get("never_gate_passed", True)
        if never_ok:
            never_str = "PASS (`exec.blocked_command=0`)"
        else:
            viols = cov_source.get("never_violations") or []
            v_str = ", ".join(f"{v.get('name')}={v.get('count')}" for v in viols) or "violation"
            never_str = f"FAIL (`{v_str}`)"

        req_passed = int(cov_source.get("required_passed") or 0)
        req_total = int(cov_source.get("required_total") or 0)
        distinct_props = int(cov_source.get("distinct_properties") or 0)
        cov_gate_str = "PASS" if cov_source.get("coverage_gate_passed", True) else "FAIL"

        hl = cov_source.get("highlights") or {}
        cov_lines.extend(
            [
                "",
                f"Snapshot from **{cov_source_name}** (`{distinct_props}` distinct properties):",
                f"- **Armed vs Fired**: `{fired_faults}/{total_faults}` fired, `{len(vacuous_list)}` vacuous{vacuous_detail}",
                f"- **Never-Property Gate**: {never_str}",
                f"- **Coverage Gate**: {cov_gate_str} (`{req_passed}/{req_total}` required properties)",
                "- **Key Counters**: "
                f"`delivery_batch_entered={int(hl.get('delivery_batch_entered') or 0)}`, "
                f"`shell_executed={int(hl.get('shell_executed') or 0)}`, "
                f"`read_your_writes_checked={int(hl.get('read_your_writes_checked') or 0)}`, "
                f"`at_most_once_checked={int(hl.get('at_most_once_checked') or 0)}`, "
                f"`committed_durability_checked={int(hl.get('committed_durability_checked') or 0)}`, "
                f"`natural_quiesce_recovered={int(hl.get('natural_quiesce_recovered') or 0)}`",
            ]
        )
    sections.append("\n".join(cov_lines))

    # 9. Checklist
    if primary_failure is not None:
        c_sc = str(primary_failure.get("scenario") or "chaos")
        c_seed = primary_failure.get("seed", "<seed>")
        c_ops = primary_failure.get("ops", 60)
        corpus_entry = f"{c_sc} {c_seed} {c_ops}"
    else:
        corpus_entry = "<scenario> <seed> <ops>"

    sections.append(
        "\n".join(
            [
                "## Checklist",
                "",
                "- [ ] Triage: real production bug vs harness/oracle bug",
                "- [ ] Add unit test + fix",
                f"- [ ] Append `{corpus_entry}` to `crates/sim/corpus/regressions.txt`",
            ]
        )
    )

    body = "\n\n".join(sections) + "\n"
    body = _enforce_body_limit(body, artifact_name, MAX_BODY_BYTES)
    return title, body


def format_step_summary(
    *,
    corpus_report: Optional[dict[str, Any]],
    corpus_err: Optional[str],
    gate_a_report: Optional[dict[str, Any]],
    gate_a_err: Optional[str],
    soak_report: Optional[dict[str, Any]],
    soak_err: Optional[str],
    commit: str,
    ref: str,
    runner: str,
    corpus_rc: Optional[int] = None,
    gate_a_rc: Optional[int] = None,
    soak_rc: Optional[int] = None,
) -> str:
    """Render a concise Markdown summary for `$GITHUB_STEP_SUMMARY` (both pass and fail)."""
    any_failed = (
        report_has_failure(corpus_report, corpus_rc)
        or report_has_failure(gate_a_report, gate_a_rc)
        or report_has_failure(soak_report, soak_rc)
        or (soak_report is None and gate_a_report is None and corpus_report is None)
    )
    badge = "FAILED" if any_failed else "PASSED"
    short_sha = commit[:7] if commit else "n/a"
    lines = [
        f"## DST Soak Summary — {badge}",
        "",
        f"Commit `{short_sha}` on `{ref or 'n/a'}` (`{runner or 'ubuntu-24.04'}`)",
        "",
        "| Phase | Result |",
        "| --- | --- |",
        f"| Corpus | {_escape_table_cell(_format_phase_result_line(corpus_report, corpus_err, corpus_rc))} |",
        f"| Gate A | {_escape_table_cell(_format_phase_result_line(gate_a_report, gate_a_err, gate_a_rc))} |",
        f"| Chaos Soak | {_escape_table_cell(_format_phase_result_line(soak_report, soak_err, soak_rc))} |",
    ]
    if soak_report is not None:
        cov = soak_report.get("coverage") or {}
        armed_rows = cov.get("armed_vs_fired") or []
        fired_faults = sum(1 for r in armed_rows if int(r.get("fired") or 0) > 0)
        vacuous = cov.get("vacuous_faults") or []
        hl = cov.get("highlights") or {}
        lines.extend(
            [
                "",
                f"- **Master Seed**: `{soak_report.get('master_seed')}`",
                f"- **Distinct Properties**: `{int(cov.get('distinct_properties') or 0)}` "
                f"(`{int(cov.get('required_passed') or 0)}/{int(cov.get('required_total') or 0)}` required)",
                f"- **Armed vs Fired**: `{fired_faults}/{len(armed_rows)}` fired, `{len(vacuous)}` vacuous",
                f"- **Never-Property Gate**: `{'PASS' if cov.get('never_gate_passed', True) else 'FAIL'}`",
                "- **Highlights**: "
                f"`delivery_batch_entered={int(hl.get('delivery_batch_entered') or 0)}`, "
                f"`shell_executed={int(hl.get('shell_executed') or 0)}`, "
                f"`read_your_writes_checked={int(hl.get('read_your_writes_checked') or 0)}`, "
                f"`at_most_once_checked={int(hl.get('at_most_once_checked') or 0)}`, "
                f"`natural_quiesce_recovered={int(hl.get('natural_quiesce_recovered') or 0)}`",
            ]
        )
    return "\n".join(lines) + "\n"


def _write_text(path: str, content: str) -> None:
    target = pathlib.Path(path)
    if target.parent and str(target.parent) != "":
        target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Format GitHub Issues and step summaries for Canban DST soak runs")
    parser.add_argument("--corpus-report", default=None)
    parser.add_argument("--gate-a-report", default=None)
    parser.add_argument("--soak-report", default=None)
    parser.add_argument("--log-file", default=None)
    parser.add_argument("--run-url", default="")
    parser.add_argument("--run-number", default="")
    parser.add_argument("--commit", default="")
    parser.add_argument("--ref", default="")
    parser.add_argument("--runner", default="ubuntu-24.04")
    parser.add_argument("--artifact-name", default="")
    parser.add_argument("--out-title", default=None)
    parser.add_argument("--out-body", default=None)
    parser.add_argument("--out-summary", default=None)
    parser.add_argument("--corpus-rc", type=int, default=None)
    parser.add_argument("--gate-a-rc", type=int, default=None)
    parser.add_argument("--soak-rc", type=int, default=None)

    args = parser.parse_args(argv)
    corpus_report, corpus_err = load_report(args.corpus_report)
    gate_a_report, gate_a_err = load_report(args.gate_a_report)
    soak_report, soak_err = load_report(args.soak_report)
    log_tail = read_log_tail(args.log_file, MAX_LOG_TAIL_LINES)

    if args.out_summary:
        summary = format_step_summary(
            corpus_report=corpus_report,
            corpus_err=corpus_err,
            gate_a_report=gate_a_report,
            gate_a_err=gate_a_err,
            soak_report=soak_report,
            soak_err=soak_err,
            commit=args.commit,
            ref=args.ref,
            runner=args.runner,
            corpus_rc=args.corpus_rc,
            gate_a_rc=args.gate_a_rc,
            soak_rc=args.soak_rc,
        )
        _write_text(args.out_summary, summary)

    if args.out_title or args.out_body or not args.out_summary:
        title, body = format_issue(
            corpus_report=corpus_report,
            corpus_err=corpus_err,
            gate_a_report=gate_a_report,
            gate_a_err=gate_a_err,
            soak_report=soak_report,
            soak_err=soak_err,
            log_tail=log_tail,
            run_url=args.run_url,
            run_number=args.run_number,
            commit=args.commit,
            ref=args.ref,
            runner=args.runner,
            artifact_name=args.artifact_name,
            corpus_rc=args.corpus_rc,
            gate_a_rc=args.gate_a_rc,
            soak_rc=args.soak_rc,
        )
        if args.out_title:
            _write_text(args.out_title, title + "\n")
        if args.out_body:
            _write_text(args.out_body, body)
        if not args.out_title and not args.out_body and not args.out_summary:
            sys.stdout.write(f"{title}\n\n{body}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
