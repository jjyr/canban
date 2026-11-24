#!/usr/bin/env python3
"""GitHub Actions cache hygiene: stable [tools]-only mise hash & per-bucket pruning."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import sys
import urllib.error
import urllib.request
from typing import Optional

_SECTION_RE = re.compile(r"^\[([^\]]+)\]\s*(?:#.*)?$")
_RUST_CACHE_RE = re.compile(r"^(v\d+-rust-.+?)(?:-[0-9a-f]{8}){1,2}$")
_MISE_CACHE_RE = re.compile(r"^(mise-v\d+-.+?)-p\d+-[0-9a-f]{16,64}$")


def _strip_inline_comment(line: str) -> str:
    quote: Optional[str] = None
    escaped = False
    for idx, ch in enumerate(line):
        if escaped:
            escaped = False
            continue
        if quote == '"' and ch == "\\":
            escaped = True
            continue
        if ch in ('"', "'"):
            if quote is None:
                quote = ch
            elif quote == ch:
                quote = None
            continue
        if ch == "#" and quote is None:
            return line[:idx]
    return line


def extract_mise_tools_lines(text: str) -> list[str]:
    """Extract normalized non-empty lines belonging to [tools] (and [tools.*]) sections."""
    in_tools = False
    lines: list[str] = []
    for raw_line in text.splitlines():
        stripped = raw_line.strip()
        header = _SECTION_RE.match(stripped)
        if header is not None:
            section = header.group(1).strip()
            in_tools = section == "tools" or section.startswith("tools.")
            continue
        if not in_tools:
            continue
        clean = _strip_inline_comment(raw_line).strip()
        if clean:
            lines.append(clean)
    if not lines:
        raise RuntimeError("No [tools] entries found in mise config")
    return lines


def extract_mise_tools_hash(path: pathlib.Path) -> str:
    """Return deterministic 24-hex SHA-256 prefix covering only [tools] definitions."""
    lines = extract_mise_tools_lines(path.read_text(encoding="utf-8"))
    payload = "\n".join(lines).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()[:24]


def cache_bucket(key: str) -> Optional[str]:
    """Map an Actions cache key to its logical bucket, or None if unmanaged."""
    m = _RUST_CACHE_RE.fullmatch(key)
    if m is not None:
        return m.group(1)
    m = _MISE_CACHE_RE.fullmatch(key)
    if m is not None:
        return m.group(1)
    return None


def select_caches_to_prune(caches: list[dict], keep_per_bucket: int = 1) -> list[dict]:
    """Select older cache entries to delete while keeping the newest `keep_per_bucket` per bucket."""
    if keep_per_bucket < 1:
        raise ValueError(f"keep_per_bucket must be >= 1, got {keep_per_bucket}")

    buckets: dict[str, list[dict]] = {}
    for entry in caches:
        bucket = cache_bucket(str(entry.get("key") or ""))
        if bucket is None:
            continue
        buckets.setdefault(bucket, []).append(entry)

    prune: list[tuple[str, str, int, dict]] = []
    for bucket, entries in buckets.items():
        ordered = sorted(
            entries,
            key=lambda e: (
                str(e.get("created_at") or ""),
                str(e.get("last_accessed_at") or ""),
                int(e.get("id") or 0),
            ),
            reverse=True,
        )
        for obsolete in ordered[keep_per_bucket:]:
            prune.append(
                (
                    bucket,
                    str(obsolete.get("created_at") or ""),
                    int(obsolete.get("id") or 0),
                    obsolete,
                )
            )

    prune.sort(key=lambda item: (item[0], item[1], item[2]))
    return [item[3] for item in prune]


def api(method: str, path: str, token: str) -> Optional[dict]:
    req = urllib.request.Request(
        "https://api.github.com" + path,
        method=method,
        headers={
            "Authorization": "Bearer " + token,
            "Accept": "application/vnd.github+json",
            "User-Agent": "Canban ci-cache",
        },
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        raw = resp.read()
        return json.loads(raw) if raw else None


def list_caches(repo: str, token: str) -> list[dict]:
    caches: list[dict] = []
    for page in range(1, 51):
        data = api("GET", f"/repos/{repo}/actions/caches?per_page=100&page={page}", token) or {}
        batch = data.get("actions_caches") or []
        caches.extend(batch)
        if len(batch) < 100:
            break
    return caches


def prune_caches(
    repo: str,
    token: str,
    *,
    keep_per_bucket: int = 1,
    dry_run: bool = False,
    best_effort: bool = False,
) -> dict:
    try:
        caches = list_caches(repo, token)
        to_delete = select_caches_to_prune(caches, keep_per_bucket=keep_per_bucket)
        deleted_ids: set[int] = set()
        freed_bytes = 0
        for entry in to_delete:
            cid = entry.get("id")
            if cid is None:
                continue
            cid_int = int(cid)
            size_bytes = int(entry.get("size_in_bytes") or 0)
            key = str(entry.get("key") or "")
            if dry_run:
                print(f"[dry-run] Would delete cache {cid_int} ({key}, {size_bytes / 1048576:.1f} MB)")
                deleted_ids.add(cid_int)
                freed_bytes += size_bytes
                continue
            try:
                api("DELETE", f"/repos/{repo}/actions/caches/{cid_int}", token)
                print(f"Deleted cache {cid_int} ({key}, {size_bytes / 1048576:.1f} MB)")
                deleted_ids.add(cid_int)
                freed_bytes += size_bytes
            except urllib.error.HTTPError as err:
                if err.code == 404:
                    continue
                raise

        remaining = [c for c in caches if int(c.get("id") or -1) not in deleted_ids]
        remaining_bytes = sum(int(c.get("size_in_bytes") or 0) for c in remaining)
        summary = {
            "deleted": len(deleted_ids),
            "freed_mb": round(freed_bytes / 1048576, 1),
            "remaining": len(remaining),
            "remaining_mb": round(remaining_bytes / 1048576, 1),
        }
        print(
            f"Pruned {summary['deleted']} cache(s), freed {summary['freed_mb']:.1f} MB; "
            f"{summary['remaining']} remaining ({summary['remaining_mb']:.1f} MB)."
        )
        return summary
    except (urllib.error.URLError, urllib.error.HTTPError, OSError, RuntimeError) as err:
        if not best_effort:
            raise
        print(f"::warning::Cache prune skipped: {err}")
        return {"deleted": 0, "freed_mb": 0.0, "remaining": 0, "remaining_mb": 0.0, "skipped": True}


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Canban CI cache utilities")
    sub = parser.add_subparsers(dest="command", required=True)

    hash_p = sub.add_parser("mise-tools-hash", help="Hash [tools] entries from mise.toml")
    hash_p.add_argument("--file", default="mise.toml")
    hash_p.add_argument("--github-env", action="store_true")

    prune_p = sub.add_parser("prune", help="Prune superseded GitHub Actions caches per bucket")
    prune_p.add_argument("--repo", default="")
    prune_p.add_argument("--keep-per-bucket", type=int, default=1)
    prune_p.add_argument("--dry-run", action="store_true")
    prune_p.add_argument("--best-effort", action="store_true")

    args = parser.parse_args(argv)
    if args.command == "mise-tools-hash":
        digest = extract_mise_tools_hash(pathlib.Path(args.file))
        print(digest)
        if args.github_env:
            env_path = os.environ.get("GITHUB_ENV", "").strip()
            if env_path:
                with pathlib.Path(env_path).open("a", encoding="utf-8") as handle:
                    handle.write(f"MISE_TOOLS_HASH={digest}\n")
        return 0

    if args.command == "prune":
        repo = (args.repo or os.environ.get("GITHUB_REPOSITORY") or "").strip()
        if not repo:
            raise RuntimeError("Repository is required (--repo or GITHUB_REPOSITORY)")
        token = (os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or "").strip()
        if not token:
            if args.best_effort:
                print("::warning::Cache prune skipped: GH_TOKEN/GITHUB_TOKEN is not set")
                return 0
            raise RuntimeError("GitHub token is required (GH_TOKEN or GITHUB_TOKEN)")
        prune_caches(
            repo,
            token,
            keep_per_bucket=args.keep_per_bucket,
            dry_run=args.dry_run,
            best_effort=args.best_effort,
        )
        return 0

    return 1


if __name__ == "__main__":
    raise SystemExit(main())
