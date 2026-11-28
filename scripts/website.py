#!/usr/bin/env python3
"""Build the 5-language, JavaScript-optional product website using the stdlib."""
# Copyright 2026 JJy <jjyruby@gmail.com>
# SPDX-License-Identifier: Apache-2.0

import argparse
import copy
import html
import json
import re
import shutil
import subprocess
from datetime import datetime
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "website"
ORIGIN = "https://canban.dev"
REPOSITORY = "https://github.com/jjyr/canban"
DESKTOP = "curl -fsSL https://canban.dev/install.sh | bash"
HEADLESS = "curl -fsSL https://canban.dev/install.sh | bash -s -- --headless"
ANDROID_APK_URL = f"{REPOSITORY}/releases/latest/download/Canban-android-arm64-v8a.apk"
OBTAINIUM_URL = (
    "https://apps.obtainium.imranr.dev/redirect?r=obtainium://app/"
    "%7B%22id%22%3A%22dev.canban.mobile%22%2C%22url%22%3A%22https%3A%2F%2Fgithub.com%2Fjjyr%2Fcanban%22%2C"
    "%22author%22%3A%22jjyr%22%2C%22name%22%3A%22Canban%22%7D"
)

LANGUAGES = (
    {
        "code": "en",
        "prefix": "",
        "html_lang": "en",
        "hreflang": "en",
        "og_locale": "en_US",
        "short": "EN",
        "label": "English",
    },
    {
        "code": "zh",
        "prefix": "zh",
        "html_lang": "zh-CN",
        "hreflang": "zh-CN",
        "og_locale": "zh_CN",
        "short": "简中",
        "label": "简体中文",
    },
    {
        "code": "zh-tw",
        "prefix": "zh-tw",
        "html_lang": "zh-TW",
        "hreflang": "zh-TW",
        "og_locale": "zh_TW",
        "short": "繁中",
        "label": "繁體中文",
    },
    {
        "code": "ja",
        "prefix": "ja",
        "html_lang": "ja",
        "hreflang": "ja",
        "og_locale": "ja_JP",
        "short": "日本語",
        "label": "日本語",
    },
    {
        "code": "ko",
        "prefix": "ko",
        "html_lang": "ko",
        "hreflang": "ko",
        "og_locale": "ko_KR",
        "short": "한국어",
        "label": "한국어",
    },
)
LANGUAGE_CODES = tuple(item["code"] for item in LANGUAGES)
LANGUAGE_BY_CODE = {item["code"]: item for item in LANGUAGES}

DOC_PAGES = ("docs", "docs/workflows", "docs/agents", "docs/cli", "docs/remote-hosts")
GUIDE_PAGES = (
    "guides/parallel-coding-agents",
    "guides/native-terminal-and-multi-machine",
    "guides/scheduled-worktree-cleanup",
)
ARTICLE_PAGES = (*DOC_PAGES, *GUIDE_PAGES)
PAGES = ("", "install", *ARTICLE_PAGES, "changelog")
NAV_PAGES = (
    ("", "home"),
    ("install", "install"),
    ("docs", "docs"),
    ("docs/workflows", "workflows"),
    ("docs/cli", "cli"),
    ("changelog", "changelog"),
)


def load(name):
    return json.loads((SOURCE / name).read_text())


def load_docs(lang):
    return json.loads((SOURCE / "docs" / f"{lang}.json").read_text())


def route(lang, page=""):
    prefix = LANGUAGE_BY_CODE[lang]["prefix"]
    base = f"/{prefix}/" if prefix else "/"
    return base + (page + "/" if page else "")


def md_route(lang, page=""):
    prefix = LANGUAGE_BY_CODE[lang]["prefix"]
    base = f"/{prefix}/" if prefix else "/"
    return base + (f"{page}.md" if page else "index.md")


def is_indexable(lang, page):
    return not (page == "changelog" and lang != "en")


def script_json(value):
    return json.dumps(value, ensure_ascii=False).replace("<", "\\u003c")


def published_releases(values):
    """Keep only published records and public fields, even with an authenticated API."""
    records = []
    for value in values:
        if value.get("draft") or not value.get("published_at"):
            continue
        url = value.get("html_url", "")
        if not url.startswith(REPOSITORY + "/releases/tag/"):
            raise ValueError("Release URL must point to this repository")
        # Parse instead of accepting arbitrary text in HTML datetime attributes.
        datetime.fromisoformat(value["published_at"].replace("Z", "+00:00"))
        records.append({key: value.get(key) for key in (
            "tag_name", "name", "html_url", "published_at", "body", "prerelease"
        )})

    def version_key(item):
        version = re.fullmatch(r"(\d{8})-(\d+)", item["tag_name"])
        return (1, int(version[1]), int(version[2])) if version else (0, item["published_at"])

    return sorted(records, key=version_key, reverse=True)


def fetch_releases():
    result = subprocess.run(
        ["gh", "api", "repos/jjyr/canban/releases?per_page=100", "--paginate", "--slurp"],
        check=True, capture_output=True, text=True,
    )
    return published_releases([value for page in json.loads(result.stdout) for value in page])


def inline_markdown(text, allow_relative=False):
    """Render safe inline markdown; untrusted release notes forbid relative links."""
    pattern = r"`([^`]+)`|\[([^\]]+)\]\(([^\s)]+)\)|\*\*([^*]+)\*\*"
    output, cursor = [], 0
    for match in re.finditer(pattern, text):
        output.append(html.escape(text[cursor:match.start()]))
        code, label, url, strong = match.groups()
        if code is not None:
            output.append("<code>" + html.escape(code) + "</code>")
        elif label is not None and (
            urlsplit(url).scheme in ("https", "http")
            or (allow_relative and (url.startswith("/") or url.startswith("#")))
        ):
            output.append(f'<a href="{html.escape(url, quote=True)}">{inline_markdown(label, allow_relative=allow_relative)}</a>')
        elif strong is not None:
            output.append("<strong>" + inline_markdown(strong, allow_relative=allow_relative) + "</strong>")
        else:
            output.append(html.escape(match.group()))
        cursor = match.end()
    output.append(html.escape(text[cursor:]))
    return "".join(output)


def release_markdown(text):
    """Render untrusted GitHub release notes strictly (HTTP/HTTPS links only, no raw HTML)."""
    blocks, paragraph = [], []
    in_list = False
    code = None

    def flush():
        if paragraph:
            blocks.append("<p>" + inline_markdown(" ".join(paragraph)) + "</p>")
            paragraph.clear()

    for line in text.splitlines():
        if line.startswith("```"):
            flush()
            if in_list:
                blocks.append("</ul>")
                in_list = False
            if code is None:
                code = []
            else:
                blocks.append("<pre><code>" + html.escape("\n".join(code)) + "</code></pre>")
                code = None
            continue
        if code is not None:
            code.append(line)
            continue
        bullet = re.match(r"^\s*[-*] (.*)", line)
        if bullet:
            flush()
            if not in_list:
                blocks.append("<ul>")
                in_list = True
            blocks.append("<li>" + inline_markdown(bullet[1]) + "</li>")
            continue
        if in_list:
            blocks.append("</ul>")
            in_list = False
        heading = re.match(r"^#{1,6}\s+(.*)", line)
        if heading:
            flush()
            blocks.append("<h3>" + inline_markdown(heading[1]) + "</h3>")
        elif not line.strip():
            flush()
        else:
            paragraph.append(line.strip())
    flush()
    if in_list:
        blocks.append("</ul>")
    if code is not None:
        blocks.append("<pre><code>" + html.escape("\n".join(code)) + "</code></pre>")
    return "\n".join(blocks)


def split_table_row(line):
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def is_table_separator(line):
    cells = split_table_row(line)
    return bool(cells) and all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells)


def _tok(cls, text):
    esc = html.escape(text, quote=False)
    return f'<span class="{cls}">{esc}</span>' if cls else esc


def _highlight_exprs(text, base_cls=None):
    parts = []
    pos = 0
    for m in re.finditer(r"\$\{\{[^{}]*\}\}", text):
        if m.start() > pos:
            parts.append(_tok(base_cls, text[pos:m.start()]))
        parts.append(_tok("tok-expr", m.group(0)))
        pos = m.end()
    if pos < len(text):
        parts.append(_tok(base_cls, text[pos:]))
    return "".join(parts)


_PY_KEYWORDS = frozenset({
    "and", "as", "assert", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if",
    "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass",
    "raise", "return", "True", "try", "while", "with", "yield",
})
_PY_TOKEN_RE = re.compile(
    r'(""".*?"""|\'\'\'.*?\'\'\'|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|#.*$|\b[A-Za-z_][A-Za-z0-9_]*\b|\b\d+(?:\.\d+)?\b)'
)


def _highlight_python_line(line):
    out, pos, after_def = [], 0, False
    for m in _PY_TOKEN_RE.finditer(line):
        if m.start() > pos:
            out.append(_tok(None, line[pos:m.start()]))
        tok = m.group(0)
        if tok.startswith("#"):
            out.append(_tok("tok-cm", tok))
            after_def = False
        elif tok[0] in ("'", '"'):
            out.append(_tok("tok-str", tok))
            after_def = False
        elif tok[0].isdigit():
            out.append(_tok("tok-num", tok))
            after_def = False
        elif after_def:
            out.append(_tok("tok-fn", tok))
            after_def = False
        elif tok in _PY_KEYWORDS:
            out.append(_tok("tok-kw", tok))
            after_def = (tok == "def")
        else:
            out.append(_tok(None, tok))
            after_def = False
        pos = m.end()
    if pos < len(line):
        out.append(_tok(None, line[pos:]))
    return "".join(out)


_SH_TOKEN_RE = re.compile(
    r'("(?:\\.|[^"\\])*"|\'(?:[^\']*)\'|\$\{[A-Za-z_][A-Za-z0-9_]*\}|\$[A-Za-z_@*?#!0-9][A-Za-z0-9_]*|&&|\|\||>>|[|;<>]|--?[A-Za-z0-9][A-Za-z0-9_-]*)'
)


def _highlight_sh_exprs(text, base_cls=None):
    parts, pos = [], 0
    for m in re.finditer(r"\$\{[A-Za-z_][A-Za-z0-9_]*\}|\$[A-Za-z_@*?#!0-9][A-Za-z0-9_]*", text):
        if m.start() > pos:
            parts.append(_tok(base_cls, text[pos:m.start()]))
        parts.append(_tok("tok-expr", m.group(0)))
        pos = m.end()
    if pos < len(text):
        parts.append(_tok(base_cls, text[pos:]))
    return "".join(parts)


def _split_sh_comment(line):
    quote = None
    i = 0
    while i < len(line):
        ch = line[i]
        if quote:
            if ch == "\\" and quote == '"' and i + 1 < len(line):
                i += 2
                continue
            if ch == quote:
                quote = None
        else:
            if ch in ('"', "'"):
                quote = ch
            elif ch == "#" and (i == 0 or line[i - 1].isspace()):
                return line[:i], line[i:]
        i += 1
    return line, ""


def _highlight_sh_line(line):
    stripped = line.lstrip()
    if not stripped:
        return ""
    if stripped.startswith("#"):
        indent = line[:len(line) - len(stripped)]
        return indent + _tok("tok-cm", stripped)
    code, comment = _split_sh_comment(line)
    out, pos = [], 0
    indent_m = re.match(r"^\s*", code)
    if indent_m:
        out.append(indent_m.group(0))
        pos = indent_m.end()
    assign_m = re.match(r"([A-Za-z_][A-Za-z0-9_]*)(=)", code[pos:])
    if assign_m:
        out.append(_tok("tok-key", assign_m.group(1)))
        out.append(_tok("tok-op", "="))
        pos += assign_m.end()
    expect_cmd = not bool(assign_m)
    cmd_re = re.compile(r'[^\s|&;<>]+')
    idx = pos
    while idx < len(code):
        if code[idx].isspace():
            ws = idx
            while idx < len(code) and code[idx].isspace():
                idx += 1
            out.append(code[ws:idx])
            continue
        m = _SH_TOKEN_RE.match(code, idx)
        if m:
            tok = m.group(0)
            if tok[0] == "'":
                out.append(_tok("tok-str", tok))
                expect_cmd = False
            elif tok[0] == '"':
                out.append(_highlight_sh_exprs(tok, "tok-str"))
                expect_cmd = False
            elif tok.startswith("$"):
                out.append(_tok("tok-expr", tok))
                expect_cmd = False
            elif tok in ("&&", "||", "|", ";", ">", ">>", "<"):
                out.append(_tok("tok-op", tok))
                expect_cmd = tok in ("&&", "||", "|", ";")
            elif tok.startswith("-"):
                out.append(_tok("tok-key", tok))
            idx = m.end()
            continue
        if expect_cmd:
            cm = cmd_re.match(code, idx)
            if cm:
                out.append(_tok("tok-fn", cm.group(0)))
                expect_cmd = False
                idx = cm.end()
                continue
        out.append(_tok(None, code[idx]))
        idx += 1
    if comment:
        out.append(_tok("tok-cm", comment))
    return "".join(out)


def _split_yaml_comment(text):
    quote = None
    i = 0
    while i < len(text):
        ch = text[i]
        if quote:
            if ch == "\\" and quote == '"' and i + 1 < len(text):
                i += 2
                continue
            if ch == quote:
                quote = None
        else:
            if ch in ('"', "'"):
                quote = ch
            elif ch == "#" and (i == 0 or text[i - 1].isspace()):
                return text[:i], text[i:]
        i += 1
    return text, ""


def _highlight_yaml_value(val):
    if not val:
        return ""
    leading = val[:len(val) - len(val.lstrip())]
    rest = val[len(leading):]
    code_part, comment = _split_yaml_comment(rest)
    trail_ws = code_part[len(code_part.rstrip()):]
    core = code_part.rstrip()
    if not core:
        rendered = ""
    elif core in ("|", ">", "|-", ">-"):
        rendered = _tok("tok-op", core)
    elif (core.startswith('"') and core.endswith('"') and len(core) >= 2) or (
        core.startswith("'") and core.endswith("'") and len(core) >= 2
    ):
        rendered = _highlight_exprs(core, "tok-str")
    elif core in ("true", "false", "null", "~"):
        rendered = _tok("tok-kw", core)
    elif re.fullmatch(r"-?\d+(?:\.\d+)?", core):
        rendered = _tok("tok-num", core)
    elif "${{" in core:
        rendered = _highlight_exprs(core, None)
    else:
        rendered = _tok(None, core)
    return leading + rendered + trail_ws + (_tok("tok-cm", comment) if comment else "")


def _highlight_yaml(code_text):
    out = []
    scalar_indent = None
    scalar_kind = None
    in_py_heredoc = False
    for line in code_text.splitlines():
        if not line.strip():
            out.append("")
            continue
        indent_len = len(line) - len(line.lstrip(" "))
        if scalar_indent is not None and indent_len > scalar_indent:
            if in_py_heredoc:
                if line.strip() == "PY":
                    in_py_heredoc = False
                    out.append(line[:indent_len] + _tok("tok-op", "PY"))
                else:
                    out.append(_highlight_python_line(line))
            elif scalar_kind == "run":
                out.append(_highlight_sh_line(line))
                if "<<'PY'" in line or '<<"PY"' in line or "<<PY" in line:
                    in_py_heredoc = True
            else:
                out.append(_tok(None, line))
            continue
        scalar_indent = None
        scalar_kind = None
        in_py_heredoc = False

        indent = line[:indent_len]
        rest = line[indent_len:]
        if rest.startswith("#"):
            out.append(indent + _tok("tok-cm", rest))
            continue
        prefix = indent
        if rest.startswith("- "):
            prefix += _tok("tok-op", "-") + " "
            rest = rest[2:]
        elif rest == "-":
            out.append(indent + _tok("tok-op", "-"))
            continue
        kv = re.match(r"^([A-Za-z0-9_.-]+)(\s*:)(.*)$", rest)
        if kv:
            key, colon, val = kv.groups()
            out.append(prefix + _tok("tok-key", key) + _tok("tok-op", colon) + _highlight_yaml_value(val))
            val_stripped = _split_yaml_comment(val.strip())[0].strip()
            if val_stripped in ("|", ">", "|-", ">-"):
                scalar_indent = indent_len
                scalar_kind = "run" if key == "run" else "text"
        else:
            out.append(prefix + _highlight_yaml_value(rest))
    return "\n".join(out)


def highlight_code(code_text, lang_hint):
    if lang_hint in ("yaml", "yml"):
        return _highlight_yaml(code_text)
    if lang_hint in ("sh", "bash"):
        return "\n".join(_highlight_sh_line(line) for line in code_text.splitlines())
    return html.escape(code_text, quote=False)


def doc_markdown(text, labels):
    """Render trusted documentation markdown (headings, lists, tables, blockquotes, copyable code)."""
    blocks, paragraph = [], []
    list_tag = None
    lines = text.splitlines()
    idx = 0

    def flush_paragraph():
        if paragraph:
            blocks.append("<p>" + inline_markdown(" ".join(paragraph), allow_relative=True) + "</p>")
            paragraph.clear()

    def close_list():
        nonlocal list_tag
        if list_tag:
            blocks.append(f"</{list_tag}>")
            list_tag = None

    while idx < len(lines):
        line = lines[idx]
        if line.startswith("```"):
            flush_paragraph()
            close_list()
            lang_hint = line[3:].strip().lower()
            code_lines = []
            idx += 1
            while idx < len(lines) and not lines[idx].startswith("```"):
                code_lines.append(lines[idx])
                idx += 1
            code_text = "\n".join(code_lines)
            if lang_hint in ("yaml", "yml", "sh", "bash", "prompt"):
                label = html.escape(lang_hint or "code")
                extra_cls = " code-block-prompt" if lang_hint == "prompt" else ""
                rendered_code = highlight_code(code_text, lang_hint)
                blocks.append(
                    f'<div class="code-block{extra_cls}"><div class="code-block-head"><span>{label}</span>'
                    f'<button class="copy-btn" type="button" data-copy-btn data-copy-value="{html.escape(code_text, quote=True)}">'
                    f'<span class="copy-text">{html.escape(labels["copy"])}</span></button></div>'
                    f'<pre><code>{rendered_code}</code></pre></div>'
                )
            else:
                blocks.append(f"<pre><code>{html.escape(code_text)}</code></pre>")
            idx += 1
            continue

        if (
            "|" in line
            and idx + 1 < len(lines)
            and "|" in lines[idx + 1]
            and is_table_separator(lines[idx + 1])
        ):
            flush_paragraph()
            close_list()
            headers = split_table_row(line)
            idx += 2
            rows = []
            while idx < len(lines) and lines[idx].strip().startswith("|"):
                rows.append(split_table_row(lines[idx]))
                idx += 1
            head_html = "".join(f"<th>{inline_markdown(cell, allow_relative=True)}</th>" for cell in headers)
            body_html = "".join(
                "<tr>" + "".join(f"<td>{inline_markdown(cell, allow_relative=True)}</td>" for cell in row) + "</tr>"
                for row in rows
            )
            blocks.append(f'<div class="table-wrap"><table><thead><tr>{head_html}</tr></thead><tbody>{body_html}</tbody></table></div>')
            continue

        if line.startswith("> "):
            flush_paragraph()
            close_list()
            quote_lines = []
            while idx < len(lines) and lines[idx].startswith("> "):
                quote_lines.append(lines[idx][2:].strip())
                idx += 1
            blocks.append("<blockquote><p>" + inline_markdown(" ".join(quote_lines), allow_relative=True) + "</p></blockquote>")
            continue

        bullet = re.match(r"^\s*[-*] (.*)", line)
        ordered = re.match(r"^\s*\d+\.\s+(.*)", line)
        if bullet or ordered:
            flush_paragraph()
            needed = "ul" if bullet else "ol"
            if list_tag != needed:
                close_list()
                blocks.append(f"<{needed}>")
                list_tag = needed
            item_text = bullet[1] if bullet else ordered[1]
            blocks.append("<li>" + inline_markdown(item_text, allow_relative=True) + "</li>")
            idx += 1
            continue

        close_list()
        heading = re.match(r"^(#{3,4})\s+(.*)", line)
        if heading:
            flush_paragraph()
            level = len(heading[1])
            blocks.append(f"<h{level}>" + inline_markdown(heading[2], allow_relative=True) + f"</h{level}>")
        elif not line.strip():
            flush_paragraph()
        else:
            paragraph.append(line.strip())
        idx += 1

    flush_paragraph()
    close_list()
    return "\n".join(blocks)


def lang_switcher(lang, page):
    current = LANGUAGE_BY_CODE[lang]
    items = []
    for info in LANGUAGES:
        code = info["code"]
        attrs = [
            'class="lang-btn"',
            f'href="{route(code, page)}"',
            f'lang="{info["html_lang"]}"',
            'role="menuitem"',
        ]
        if page != "changelog":
            attrs.append(f'hreflang="{info["hreflang"]}"')
        if code == lang:
            attrs.append('aria-current="page"')
        items.append(f'<a {" ".join(attrs)}>{html.escape(info["label"])}</a>')
    return (
        '<details class="lang-menu">'
        f'<summary class="lang-trigger" aria-label="Language">{html.escape(current["short"])} <span class="lang-caret" aria-hidden="true">▾</span></summary>'
        f'<div class="lang-dropdown lang-switch" role="menu">{"".join(items)}</div>'
        '</details>'
    )


PROVIDER_CLIS = (
    ("claude", "Claude Code", "#d97757"),
    ("codex", "Codex", "#10a37f"),
    ("agy", "Antigravity (agy)", None),
    ("cursor", "Cursor Agent", "#38bdf8"),
    ("gemini", "Gemini CLI", None),
    ("opencode", "OpenCode", "#60a5fa"),
    ("pi", "Pi", "#2dd4bf"),
    ("copilot", "GitHub Copilot", None),
    ("kimi", "Kimi", None),
    ("qwen", "Qwen Code", None),
    ("amp", "Amp", "#f34e3f"),
    ("droid", "Droid", "#a3e635"),
    ("cline", "Cline", "#22d3ee"),
    ("devin", "Devin", None),
    ("grok", "Grok", "#f97316"),
    ("kiro", "Kiro", "#a855f7"),
    ("omp", "Omp", "#fb923c"),
    ("hermes", "Hermes", "#fbbf24"),
    ("kilo", "Kilo", "#f8f676"),
    ("qodercli", "Qoder", None),
    ("letta", "Letta", "#c084fc"),
    ("mastracode", "Mastra Code", "#34d399"),
    ("maki", "Maki", "#f472b6"),
    ("muse", "Muse", None),
)


def render_cli_ecosystem():
    items = []
    for cli_id, label, _ in PROVIDER_CLIS:
        items.append(
            f'<span class="cli-chip">'
            f'<img class="cli-chip-icon" src="/providers/{cli_id}.svg" width="15" height="15" alt="" aria-hidden="true">'
            f'<span>{html.escape(label)}</span>'
            f'</span>'
        )
    return "\n        ".join(items)


def copy_provider_icons(output):
    src_root = ROOT / "crates/ui-kit/assets/providers"
    dst_root = output / "providers"
    if dst_root.exists():
        shutil.rmtree(dst_root)
    dst_root.mkdir(parents=True, exist_ok=True)
    for cli_id, _, brand_hex in PROVIDER_CLIS:
        color_svg = src_root / "color" / f"{cli_id}.svg"
        mono_svg = src_root / f"{cli_id}.svg"
        if brand_hex is None and color_svg.is_file():
            shutil.copyfile(color_svg, dst_root / f"{cli_id}.svg")
        else:
            svg_text = mono_svg.read_text()
            if brand_hex:
                svg_text = svg_text.replace("currentColor", brand_hex)
            (dst_root / f"{cli_id}.svg").write_text(svg_text)


def localize_home(lang, strings):
    body = (SOURCE / "templates/home.html").read_text()
    pattern = r'(<(?P<tag>[\w-]+)\b[^>]*\bdata-i18n="(?P<key>[^"]+)"[^>]*>)(.*?)(</(?P=tag)>)'

    def replace(match):
        key = match["key"]
        value = strings[key]
        return match[1] + html.escape(value) + match[5]

    body = re.sub(pattern, replace, body, flags=re.DOTALL)
    for key, value in {
        "lang_switcher": lang_switcher(lang, ""),
        "home_url": route(lang, ""),
        "install_url": route(lang, "install"),
        "docs_url": route(lang, "docs"),
        "workflows_url": route(lang, "docs/workflows"),
        "cli_url": route(lang, "docs/cli"),
        "agents_url": route(lang, "docs/agents"),
        "guide_parallel_url": route(lang, "guides/parallel-coding-agents"),
        "guide_native_url": route(lang, "guides/native-terminal-and-multi-machine"),
        "guide_worktree_gc_url": route(lang, "guides/scheduled-worktree-cleanup"),
        "changelog_url": route(lang, "changelog"),
        "md_url": md_route(lang, ""),
        "android_apk_url": html.escape(ANDROID_APK_URL, quote=True),
        "obtainium_url": html.escape(OBTAINIUM_URL, quote=True),
        "cli_ecosystem_items": render_cli_ecosystem(),
        "tab_briefing_caption": html.escape(strings["tab_briefing_desc"], quote=True),
        "tab_board_caption": html.escape(strings["tab_board_desc"], quote=True),
        "tab_terminals_caption": html.escape(strings["tab_terminals_desc"], quote=True),
        "tab_review_caption": html.escape(strings["tab_review_desc"], quote=True),
        "tab_workflows_caption": html.escape(strings["tab_workflows_desc"], quote=True),
        "alt_daily_briefing": html.escape(strings["alt_daily_briefing"], quote=True),
        "alt_board": html.escape(strings["alt_board"], quote=True),
        "alt_terminals": html.escape(strings["alt_terminals"], quote=True),
        "alt_git_review": html.escape(strings["alt_git_review"], quote=True),
        "alt_workflows": html.escape(strings["alt_workflows"], quote=True),
    }.items():
        body = body.replace("{{" + key + "}}", value)
    return body


def nav_active(page, key):
    if page == key:
        return True
    return key == "docs" and (page.startswith("docs/") or page.startswith("guides/")) and not any(
        nav_key == page for nav_key, _ in NAV_PAGES
    )


def header(lang, page, labels):
    links = "".join(
        f'<a href="{route(lang, key)}"' + (' aria-current="page"' if nav_active(page, key) else "")
        + f'>{html.escape(labels[name])}</a>'
        for key, name in NAV_PAGES
    )
    nav_label = html.escape(labels.get("nav_aria", "Main navigation"))
    return f'''<header class="site-header"><div class="site-header-inner">
      <a class="site-brand" href="{route(lang)}">
      <img src="/favicon.svg" class="brand-mark" width="22" height="22" alt=""><span>Canban</span></a>
      <nav class="site-nav" aria-label="{nav_label}">{links}</nav>
      <div class="site-actions">{lang_switcher(lang, page)}</div></div></header>'''


def command(text, labels):
    return f'''<div class="doc-command"><pre><code>{html.escape(text)}</code></pre>
      <button class="copy-btn" type="button" data-copy-btn data-copy-value="{html.escape(text, quote=True)}">
      <span class="copy-text">{html.escape(labels['copy'])}</span></button></div>'''


def render_screenshot(shot):
    if not shot:
        return ""
    src = html.escape(shot["src"], quote=True)
    alt = html.escape(shot["alt"], quote=True)
    caption = html.escape(shot["caption"])
    width = int(shot.get("width", 2880))
    height = int(shot.get("height", 1800))
    loading = ' loading="lazy"' if shot.get("lazy", True) else ""
    return (
        f'<figure class="doc-screenshot">'
        f'<img src="{src}" alt="{alt}" width="{width}" height="{height}" decoding="async"{loading}>'
        f'<figcaption>{caption}</figcaption>'
        f'</figure>'
    )


def footer(lang, page, labels):
    return f'''<footer class="site-footer"><span>Apache-2.0 · © 2026 JJy</span><nav class="footer-links">
      <a href="{route(lang, 'install')}">{html.escape(labels['install'])}</a>
      <a href="{route(lang, 'docs')}">{html.escape(labels['docs'])}</a>
      <a href="{route(lang, 'docs/workflows')}">{html.escape(labels['workflows'])}</a>
      <a href="{route(lang, 'docs/cli')}">{html.escape(labels['cli'])}</a>
      <a href="{route(lang, 'guides/parallel-coding-agents')}">{html.escape(labels['guide_parallel'])}</a>
      <a href="{route(lang, 'guides/native-terminal-and-multi-machine')}">{html.escape(labels['guide_native'])}</a>
      <a href="{route(lang, 'guides/scheduled-worktree-cleanup')}">{html.escape(labels['guide_worktree_gc'])}</a>
      <a href="{route(lang, 'changelog')}">{html.escape(labels['changelog'])}</a>
      <a href="{md_route(lang, page)}">{html.escape(labels['markdown_mirror'])}</a>
      <a href="/llms-full.txt">{html.escape(labels['llms_full'])}</a>
      <a href="{REPOSITORY}">GitHub</a></nav></footer>'''


def render_faq_section(faq_heading, faq_items):
    items_html = "".join(
        f'<article class="faq-item"><h3>{html.escape(q)}</h3><p>{html.escape(a)}</p></article>'
        for q, a in faq_items
    )
    return f'<section id="faq"><h2>{html.escape(faq_heading)}</h2>{items_html}</section>'


def install_body(lang, labels):
    t = lambda key: html.escape(labels[key])
    dependencies = "".join(f'<h3>{name}</h3><pre><code>{html.escape(cmd)}</code></pre>' for name, cmd in (
        ("Ubuntu / Debian", "sudo apt install libgtk-4-1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0"),
        ("Arch Linux", "sudo pacman -S gtk4 gstreamer gst-plugins-base"),
        ("Fedora", "sudo dnf install gtk4 gstreamer1 gstreamer1-plugins-base"),
    ))
    intro_shot = render_screenshot(labels.get("install_screenshot"))
    return f'''<div class="doc-shell">{header(lang, 'install', labels)}
    <main id="main" class="doc-main"><div class="doc-intro"><p class="eyebrow">CANBAN // INSTALL</p>
      <h1>{t('install_title')}</h1><p>{t('install_intro')}</p>{intro_shot}</div>
    <section><h2>{t('requirements')}</h2><p>{t('requirements_text')}</p></section>
    <section id="desktop"><h2>{t('desktop')}</h2><p>{t('desktop_text')}</p>{command(DESKTOP, labels)}
      <div class="platform-grid"><article><h3>{t('macos')}</h3><p>{t('macos_text')}</p></article>
      <article><h3>{t('linux')}</h3><p>{t('linux_text')}</p></article></div>
      <details><summary>{t('linux_deps')}</summary>{dependencies}</details></section>
    <section id="headless"><h2>{t('headless')}</h2><p>{t('headless_text')}</p>{command(HEADLESS, labels)}</section>
    <section id="mobile"><h2>{t('mobile')}</h2><h3>{t('android')}</h3><p>{t('android_text')}</p>
      <div class="action-row">
        <a class="action-btn is-primary" href="{ANDROID_APK_URL}" target="_blank" rel="noopener noreferrer">{t('android_download')}</a>
        <a class="action-btn" href="{OBTAINIUM_URL}" target="_blank" rel="noopener noreferrer">{t('android_obtainium')}</a>
      </div>
      <a class="text-link" href="{REPOSITORY}/releases/latest" target="_blank" rel="noopener noreferrer">{t('android_releases_link')} →</a></section>
    <section id="updates"><h2>{t('updates')}</h2><p>{t('updates_text')}</p>
      <a class="text-link" href="{route(lang, 'changelog')}">{t('releases_link')} →</a></section>
    <section><h2>{t('source')}</h2><p>{t('source_text')}</p>
      <a class="text-link" href="{REPOSITORY}#build-from-source">GitHub README →</a></section>
    {render_faq_section(labels['faq'], labels['faq_items'])}</main>{footer(lang, 'install', labels)}</div>'''


def changelog_body(lang, labels, releases):
    entries = []
    for release in releases:
        tag = html.escape(release["tag_name"])
        url = html.escape(release["html_url"], quote=True)
        stamp = release["published_at"]
        date = stamp[:10]
        badge = f'<span class="release-badge">{html.escape(labels["prerelease"])}</span>' if release["prerelease"] else ""
        title = html.escape(release["name"] or release["tag_name"])
        notes = release_markdown(release["body"] or labels["no_notes"])
        entries.append(f'''<article class="release" id="{tag}"><div class="release-heading">
          <h2><a href="{url}">{title}</a></h2>{badge}<time datetime="{stamp}" title="{stamp}">{date}</time></div>
          <div class="release-notes">{notes}</div><a class="text-link" href="{url}">{html.escape(labels['github_release'])} →</a></article>''')
    return f'''<div class="doc-shell">{header(lang, 'changelog', labels)}
      <main id="main" class="doc-main"><div class="doc-intro"><p class="eyebrow">CANBAN // CHANGELOG</p>
      <h1>{html.escape(labels['changelog'])}</h1><p>{html.escape(labels['release_intro'])}</p><p class="muted">{html.escape(labels['release_note'])}</p></div>
      {''.join(entries)}<a class="text-link" href="{REPOSITORY}/releases">{html.escape(labels['all_releases'])} →</a>
      </main>{footer(lang, 'changelog', labels)}</div>'''


def render_details(details, labels):
    if not details:
        return ""
    return (
        f'<details class="doc-details"><summary>{html.escape(details["summary"])}</summary>'
        f'<div class="doc-details-body">{doc_markdown(details["markdown"], labels)}</div></details>'
    )


def doc_body(lang, page, spec, labels):
    crumbs = [f'<a href="{route(lang)}">{html.escape(labels["home"])}</a>']
    for crumb_label, crumb_slug in spec["breadcrumbs"]:
        if crumb_slug == page:
            crumbs.append(f'<span aria-current="page">{html.escape(crumb_label)}</span>')
        else:
            crumbs.append(f'<a href="{route(lang, crumb_slug)}">{html.escape(crumb_label)}</a>')
    breadcrumb_nav = f'<nav class="doc-breadcrumbs" aria-label="Breadcrumb">{" / ".join(crumbs)}</nav>'

    cards_html = ""
    if spec.get("cards"):
        cards = "".join(
            f'<a class="doc-card" href="{html.escape(card["href"], quote=True)}">'
            f'<span class="doc-card-title">{html.escape(card["title"])} →</span>'
            f'<p class="doc-card-desc">{html.escape(card["summary"])}</p></a>'
            for card in spec["cards"]
        )
        cards_html = f'<section aria-label="{html.escape(spec["title"])}"><div class="doc-card-grid">{cards}</div></section>'

    intro_shot = render_screenshot(spec.get("screenshot"))
    sections_html = "".join(
        f'<section id="{html.escape(sec["id"], quote=True)}"><h2>{html.escape(sec["heading"])}</h2>'
        f'{doc_markdown(sec["markdown"], labels)}{render_screenshot(sec.get("screenshot"))}'
        f'{render_details(sec.get("details"), labels)}</section>'
        for sec in spec["sections"]
    )
    faq_html = render_faq_section(labels["faq"], spec["faq_items"]) if spec.get("faq_items") else ""
    return f'''<div class="doc-shell">{header(lang, page, labels)}
      <main id="main" class="doc-main"><div class="doc-intro">{breadcrumb_nav}<p class="eyebrow">{html.escape(spec['eyebrow'])}</p>
      <h1>{html.escape(spec['title'])}</h1><p>{inline_markdown(spec['intro'], allow_relative=True)}</p>{intro_shot}</div>
      {cards_html}{sections_html}{faq_html}</main>{footer(lang, page, labels)}</div>'''


def breadcrumb_items(lang, page, labels, docs_spec=None):
    home_url = ORIGIN + route(lang)
    items = [{"@type": "ListItem", "position": 1, "name": labels["home"], "item": home_url}]
    if docs_spec and docs_spec.get("breadcrumbs"):
        for idx, (crumb_label, crumb_slug) in enumerate(docs_spec["breadcrumbs"], start=2):
            items.append({
                "@type": "ListItem",
                "position": idx,
                "name": crumb_label,
                "item": ORIGIN + route(lang, crumb_slug),
            })
        return items
    items.append({"@type": "ListItem", "position": 2, "name": labels[page], "item": ORIGIN + route(lang, page)})
    return items


def article_image_src(docs_spec):
    if docs_spec.get("screenshot"):
        return docs_spec["screenshot"]["src"]
    for sec in docs_spec.get("sections", ()):
        if sec.get("screenshot"):
            return sec["screenshot"]["src"]
    return "/og-image.png"


def structured_data(lang, page, title, description, labels, docs_spec=None, latest_release=None):
    """Build page-scoped Schema.org JSON-LD with localized properties and visible FAQs."""
    language = LANGUAGE_BY_CODE[lang]["hreflang"]
    home_url = ORIGIN + route(lang)
    page_url = ORIGIN + route(lang, page)
    schema = copy.deepcopy(load("schema.json"))
    localized = schema.pop("localized")[lang]
    graph = schema["@graph"]
    for item in graph:
        item["@id"] = home_url + ("#website" if item["@type"] == "WebSite" else "#software")
        item["url"] = home_url
        item["inLanguage"] = language
        if item["@type"] == "WebSite":
            item["description"] = localized["website_desc"]
        else:
            item["description"] = localized["software_desc"]
            item["applicationSubCategory"] = localized["application_sub_category"]
            item["softwareRequirements"] = localized["software_requirements"]
            item["keywords"] = localized["keywords"]
            item["installUrl"] = ORIGIN + route(lang, "install")
            item["releaseNotes"] = ORIGIN + route(lang, "changelog")
            if not page:
                item["featureList"] = localized["feature_list"]
            else:
                item.pop("featureList", None)
            if latest_release:
                item["softwareVersion"] = latest_release["tag_name"]
                item["dateModified"] = latest_release["published_at"]

    if page in ARTICLE_PAGES:
        article = {
            "@type": "TechArticle",
            "@id": page_url + "#webpage",
            "url": page_url,
            "mainEntityOfPage": {"@type": "WebPage", "@id": page_url},
            "headline": title,
            "name": title,
            "description": description,
            "image": ORIGIN + article_image_src(docs_spec),
            "inLanguage": language,
            "author": {"@type": "Person", "name": "JJy", "url": "https://github.com/jjyr"},
            "publisher": {
                "@type": "Organization",
                "name": "Canban",
                "url": ORIGIN + "/",
                "logo": {"@type": "ImageObject", "url": ORIGIN + "/icon-512.png"},
            },
            "datePublished": docs_spec["date_published"],
            "dateModified": docs_spec["updated"],
            "isPartOf": {"@id": home_url + "#website"},
            "about": {"@id": home_url + "#software"},
        }
        graph.append(article)
    else:
        graph.append({
            "@type": "CollectionPage" if page == "changelog" else "WebPage",
            "@id": page_url + "#webpage",
            "url": page_url,
            "name": title,
            "description": description,
            "inLanguage": language,
            "isPartOf": {"@id": home_url + "#website"},
            "about": {"@id": home_url + "#software"},
        })

    if page:
        graph.append({
            "@type": "BreadcrumbList",
            "@id": page_url + "#breadcrumb",
            "itemListElement": breadcrumb_items(lang, page, labels, docs_spec),
        })

    faq_items = (
        labels["faq_items"] if page == "install"
        else docs_spec.get("faq_items") if docs_spec
        else None
    )
    if faq_items:
        graph.append({
            "@type": "FAQPage",
            "@id": page_url + "#faq",
            "url": page_url,
            "inLanguage": language,
            "mainEntity": [
                {
                    "@type": "Question",
                    "name": question,
                    "acceptedAnswer": {"@type": "Answer", "text": answer},
                }
                for question, answer in faq_items
            ],
        })
    return schema


def page_meta(page, strings, labels, docs_spec=None):
    if not page:
        return strings["meta_title"], strings["meta_desc"]
    if page == "install":
        return labels["install_title"], labels["install_desc"]
    if page == "changelog":
        return labels["changelog"] + " — Canban", labels["release_intro"]
    return docs_spec["meta_title"], docs_spec["description"]


def page_updated(lang, page, locales, pages, docs, latest_release):
    if not page:
        return locales[lang]["updated"]
    if page == "install":
        return pages[lang]["install_updated"]
    if page == "changelog":
        return latest_release["published_at"][:10] if latest_release else ""
    return docs[lang][page]["updated"]


def document(lang, page, body, strings, labels, docs_spec=None, latest_release=None):
    info = LANGUAGE_BY_CODE[lang]
    path = route(lang, page)
    title, description = page_meta(page, strings, labels, docs_spec)
    locale, language = info["og_locale"], info["html_lang"]
    schema = structured_data(lang, page, title, description, labels, docs_spec, latest_release)
    robots = (
        "index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1"
        if is_indexable(lang, page)
        else "noindex, follow"
    )
    if page != "changelog":
        alternates = "\n".join([
            *(f'<link rel="alternate" hreflang="{item["hreflang"]}" href="{ORIGIN + route(item["code"], page)}">' for item in LANGUAGES),
            f'<link rel="alternate" hreflang="x-default" href="{ORIGIN + route("en", page)}">',
        ])
        og_alternates = "".join(
            f'<meta property="og:locale:alternate" content="{item["og_locale"]}">'
            for item in LANGUAGES
            if item["code"] != lang
        )
    else:
        alternates = ""
        og_alternates = ""
    md_alternate = f'<link rel="alternate" type="text/markdown" href="{ORIGIN + md_route(lang, page)}">'
    # A tiny early redirect preserves legacy bookmarks, other parameters and channel hashes.
    legacy = '''<script>(function(){var u=new URL(location.href),l=u.searchParams.get("lang");
      if(!l)return;var m={"en":"","zh":"/zh","zh-cn":"/zh","zh-tw":"/zh-tw","zh-hk":"/zh-tw","ja":"/ja","ja-jp":"/ja","ko":"/ko","ko-kr":"/ko"}[l.toLowerCase()];
      if(m===undefined)return;u.searchParams.delete("lang");
      var p=u.pathname.replace(/^\\/(?:zh-tw|zh|ja|ko)(?=\\/|$)/,"")||"/";
      location.replace(m+p+u.search+u.hash);})();</script>'''
    return f'''<!DOCTYPE html>
<html lang="{language}" data-lang="{lang}" class="{'home-root' if not page else 'document-root'}"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title><meta name="description" content="{html.escape(description, quote=True)}">
<meta name="application-name" content="Canban"><meta name="apple-mobile-web-app-title" content="Canban">
<meta name="author" content="JJy"><meta name="robots" content="{robots}">
<meta name="theme-color" content="#141413"><link rel="canonical" href="{ORIGIN + path}">{alternates}
{md_alternate}
<meta property="og:site_name" content="Canban"><meta property="og:type" content="website">
<meta property="og:title" content="{html.escape(title, quote=True)}"><meta property="og:description" content="{html.escape(description, quote=True)}">
<meta property="og:url" content="{ORIGIN + path}"><meta property="og:locale" content="{locale}">
{og_alternates}<meta property="og:image" content="{ORIGIN}/og-image.png"><meta property="og:image:type" content="image/png">
<meta property="og:image:width" content="1200"><meta property="og:image:height" content="630">
<meta property="og:image:alt" content="{html.escape(strings['hero_title'], quote=True)}">
<meta name="twitter:card" content="summary_large_image"><meta name="twitter:title" content="{html.escape(title, quote=True)}">
<meta name="twitter:description" content="{html.escape(description, quote=True)}"><meta name="twitter:image" content="{ORIGIN}/og-image.png">
<meta name="twitter:image:alt" content="{html.escape(strings['hero_title'], quote=True)}">
<link rel="icon" type="image/svg+xml" href="/favicon.svg"><link rel="icon" type="image/png" sizes="48x48" href="/favicon-48x48.png">
<link rel="icon" type="image/png" sizes="192x192" href="/icon-192.png">
<link rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon.png"><link rel="manifest" href="/site.webmanifest">
<script type="application/ld+json">{script_json(schema)}</script>{legacy}
<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600;700&amp;display=swap" rel="stylesheet">
<link rel="stylesheet" href="/styles.css"><link rel="stylesheet" href="/pages.css">
<noscript><style>.showcase-bar,.showcase-caption{{display:none}}.showcase-panel[hidden]{{display:block!important;border-top:1px solid var(--border)}}.showcase-panel figcaption{{display:block;padding:10px 14px;background:var(--bg-surface);color:var(--fg-sec);font-size:13px}}.install-shell-android[hidden]{{display:flex!important;margin-top:8px}}</style></noscript>
</head><body class="{'home-page' if not page else 'document-page'}">
{body}<script type="application/json" id="page-copy">{script_json({key: strings[key] for key in ('copy_label', 'copied_label')})}</script>
<script src="/app.js" defer></script></body></html>
'''


def render_home_markdown(lang, strings):
    lines = [
        f"# {strings['meta_title']}",
        "",
        "> Canban = Kanban + Workflow + Herdr",
        "",
        strings["hero_title"],
        "",
        strings["hero_description"],
        "",
        "## Highlights",
        "",
        f"- **{strings['tab_briefing']}**: {strings['tab_briefing_desc']}",
        f"- **{strings['tab_board']}**: {strings['tab_board_desc']}",
        f"- **{strings['tab_terminals']}**: {strings['tab_terminals_desc']}",
        f"- **{strings['tab_review']}**: {strings['tab_review_desc']}",
        f"- **{strings['tab_workflows']}**: {strings['tab_workflows_desc']}",
        "",
        "## Install",
        "",
        "```sh",
        DESKTOP,
        "```",
        "",
        "```sh",
        HEADLESS,
        "```",
        "",
        f"- **{strings['tab_android']} ({strings['android_meta']})**: [{strings['android_download_short']}]({ANDROID_APK_URL}) · [{strings['android_obtainium_short']}]({OBTAINIUM_URL}) · [{strings['android_guide_short']}]({ORIGIN + route(lang, 'install') + '#mobile'})",
        "",
        "## Navigation",
        "",
        f"- {strings['nav_install']}: {ORIGIN + route(lang, 'install')}",
        f"- {strings['nav_docs']}: {ORIGIN + route(lang, 'docs')}",
        f"- {strings['nav_workflows']}: {ORIGIN + route(lang, 'docs/workflows')}",
        f"- {strings['nav_cli']}: {ORIGIN + route(lang, 'docs/cli')}",
        f"- {strings['nav_guide_parallel']}: {ORIGIN + route(lang, 'guides/parallel-coding-agents')}",
        f"- {strings['nav_guide_native']}: {ORIGIN + route(lang, 'guides/native-terminal-and-multi-machine')}",
        f"- {strings['nav_guide_worktree_gc']}: {ORIGIN + route(lang, 'guides/scheduled-worktree-cleanup')}",
        f"- {strings['nav_changelog']}: {ORIGIN + route(lang, 'changelog')}",
        f"- GitHub: {REPOSITORY}",
    ]
    return "\n".join(lines) + "\n"


def screenshot_markdown(shot):
    if not shot:
        return []
    return [f"![{shot['alt']}]({ORIGIN + shot['src']})", "", f"*{shot['caption']}*", ""]


def render_install_markdown(lang, labels):
    lines = [
        f"# {labels['install_title']}",
        "",
        f"> {labels['install_intro']}",
        "",
        *screenshot_markdown(labels.get("install_screenshot")),
        f"## {labels['requirements']}",
        "",
        labels["requirements_text"],
        "",
        f"## {labels['desktop']}",
        "",
        labels["desktop_text"],
        "",
        "```sh",
        DESKTOP,
        "```",
        "",
        f"- **{labels['macos']}**: {labels['macos_text']}",
        f"- **{labels['linux']}**: {labels['linux_text']}",
        "",
        f"### {labels['linux_deps']}",
        "",
        "```sh",
        "# Ubuntu / Debian",
        "sudo apt install libgtk-4-1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0",
        "# Arch Linux",
        "sudo pacman -S gtk4 gstreamer gst-plugins-base",
        "# Fedora",
        "sudo dnf install gtk4 gstreamer1 gstreamer1-plugins-base",
        "```",
        "",
        f"## {labels['headless']}",
        "",
        labels["headless_text"],
        "",
        "```sh",
        HEADLESS,
        "```",
        "",
        f"## {labels['mobile']}",
        "",
        f"### {labels['android']}",
        "",
        labels["android_text"],
        "",
        f"- {labels['android_download']}: {ANDROID_APK_URL}",
        f"- {labels['android_obtainium']}: {OBTAINIUM_URL}",
        f"- {labels['android_releases_link']}: {REPOSITORY}/releases/latest",
        "",
        f"## {labels['updates']}",
        "",
        labels["updates_text"],
        "",
        f"## {labels['source']}",
        "",
        f"{labels['source_text']} ({REPOSITORY}#build-from-source)",
        "",
        f"## {labels['faq']}",
        "",
    ]
    for question, answer in labels["faq_items"]:
        lines.extend([f"### {question}", "", answer, ""])
    return "\n".join(lines)


def render_changelog_markdown(lang, labels, releases):
    lines = [
        f"# {labels['changelog']} — Canban",
        "",
        f"> {labels['release_intro']} {labels['release_note']}",
        "",
    ]
    for release in releases:
        title = release["name"] or release["tag_name"]
        badge = f" [{labels['prerelease']}]" if release["prerelease"] else ""
        lines.extend([
            f"## {title}{badge} ({release['published_at'][:10]})",
            "",
            f"- GitHub Release: {release['html_url']}",
            "",
            (release["body"] or labels["no_notes"]).strip(),
            "",
        ])
    return "\n".join(lines)


def render_doc_markdown_mirror(lang, spec, labels):
    lines = [f"# {spec['title']}", "", f"> {spec['intro']}", ""]
    lines.extend(screenshot_markdown(spec.get("screenshot")))
    if spec.get("cards"):
        for card in spec["cards"]:
            lines.append(f"- [{card['title']}]({ORIGIN + card['href']}): {card['summary']}")
        lines.append("")
    for sec in spec["sections"]:
        lines.extend([f"## {sec['heading']}", "", sec["markdown"].strip(), ""])
        lines.extend(screenshot_markdown(sec.get("screenshot")))
        if sec.get("details"):
            lines.extend([f"### {sec['details']['summary']}", "", sec["details"]["markdown"].strip(), ""])
    if spec.get("faq_items"):
        lines.extend([f"## {labels['faq']}", ""])
        for question, answer in spec["faq_items"]:
            lines.extend([f"### {question}", "", answer, ""])
    return "\n".join(lines)


def strip_frontmatter(text):
    if text.startswith("---\n"):
        parts = text.split("\n---\n", 1)
        if len(parts) == 2:
            return parts[1].lstrip()
    return text


def render_llms_full(locales, pages, docs, releases):
    skill_text = strip_frontmatter((ROOT / "skills/canban/SKILL.md").read_text()).strip()
    parts = [
        "# Canban (`canban.dev`) — Complete Reference for LLMs",
        "",
        "> Canban (`canban.dev`, GitHub: `jjyr/canban`, Rust + NSH + `libghostty-vt` + `herdr-core`) — Organize with Kanban. Work with native agent CLIs. A lightweight, open-source native macOS and Linux app for AI coding agents. Formula: `Canban = Kanban + Workflow + Herdr`.",
        "",
        "**Disambiguation**: Canban (`https://canban.dev/`) is a Rust desktop application and daemon (`canband`), distinct from generic Kanban boards or SaaS issue trackers.",
        "",
        "---",
        "",
        render_home_markdown("en", locales["en"]).strip(),
        "",
        "---",
        "",
        render_install_markdown("en", pages["en"]).strip(),
        "",
    ]
    for slug in ARTICLE_PAGES:
        parts.extend(["---", "", render_doc_markdown_mirror("en", docs["en"][slug], pages["en"]).strip(), ""])

    parts.extend(["---", "", "## Agent CLI Skill Reference (`skills/canban/SKILL.md`)", "", skill_text, ""])

    if releases:
        parts.extend(["---", "", "## Recent Published Releases", ""])
        for release in releases[:5]:
            parts.append(f"- **{release['tag_name']}** ({release['published_at'][:10]}): {release['html_url']}")
        parts.append("")
    return "\n".join(parts)


def build(output, releases):
    output.mkdir(parents=True, exist_ok=True)
    for source in SOURCE.iterdir():
        if source.suffix in (".css", ".js", ".png", ".svg", ".webmanifest") or source.name in ("CNAME", "robots.txt", "llms.txt"):
            shutil.copyfile(source, output / source.name)
    screenshots_src = SOURCE / "screenshots"
    if screenshots_src.is_dir():
        screenshots_dst = output / "screenshots"
        if screenshots_dst.exists():
            shutil.rmtree(screenshots_dst)
        screenshots_dst.mkdir(parents=True, exist_ok=True)
        for shot in sorted(screenshots_src.iterdir()):
            if shot.is_file() and shot.suffix in (".webp", ".png"):
                shutil.copyfile(shot, screenshots_dst / shot.name)
    copy_provider_icons(output)
    shutil.copyfile(ROOT / "scripts/install.sh", output / "install.sh")
    locales = load("locales.json")
    pages = load("pages.json")
    docs = {lang: load_docs(lang) for lang in LANGUAGE_CODES}
    latest_release = releases[0] if releases else None

    for lang in LANGUAGE_CODES:
        for page in PAGES:
            docs_spec = docs[lang][page] if page in ARTICLE_PAGES else None
            if not page:
                body = localize_home(lang, locales[lang])
                md_text = render_home_markdown(lang, locales[lang])
            elif page == "install":
                body = install_body(lang, pages[lang])
                md_text = render_install_markdown(lang, pages[lang])
            elif page == "changelog":
                body = changelog_body(lang, pages[lang], releases)
                md_text = render_changelog_markdown(lang, pages[lang], releases)
            else:
                body = doc_body(lang, page, docs_spec, pages[lang])
                md_text = render_doc_markdown_mirror(lang, docs_spec, pages[lang])

            destination = output / route(lang, page).lstrip("/") / "index.html"
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(document(lang, page, body, locales[lang], pages[lang], docs_spec, latest_release))

            md_destination = output / md_route(lang, page).lstrip("/")
            md_destination.parent.mkdir(parents=True, exist_ok=True)
            md_destination.write_text(md_text)

    (output / "llms-full.txt").write_text(render_llms_full(locales, pages, docs, releases))

    urls = []
    for page in PAGES:
        if page == "changelog":
            stamp = page_updated("en", page, locales, pages, docs, latest_release)
            lastmod = f"<lastmod>{stamp}</lastmod>" if stamp else ""
            urls.append(f'<url><loc>{ORIGIN + route("en", page)}</loc>{lastmod}</url>')
            continue
        alternates = "".join([
            *(f'<xhtml:link rel="alternate" hreflang="{info["hreflang"]}" href="{ORIGIN + route(info["code"], page)}" />' for info in LANGUAGES),
            f'<xhtml:link rel="alternate" hreflang="x-default" href="{ORIGIN + route("en", page)}" />',
        ])
        for lang in LANGUAGE_CODES:
            stamp = page_updated(lang, page, locales, pages, docs, latest_release)
            lastmod = f"<lastmod>{stamp}</lastmod>" if stamp else ""
            urls.append(f'<url><loc>{ORIGIN + route(lang, page)}</loc>{lastmod}{alternates}</url>')
    (output / "sitemap.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n'
        + "\n".join(urls)
        + "\n</urlset>\n"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / ".local/website")
    parser.add_argument("--refresh-releases", action="store_true", help="Fetch published GitHub Releases; fail rather than publish stale history on API errors")
    args = parser.parse_args()
    releases = fetch_releases() if args.refresh_releases else published_releases(load("releases.json"))
    build(args.output, releases)
    total_pages = len(PAGES) * len(LANGUAGE_CODES)
    print(f"Built {total_pages} HTML pages, {total_pages} Markdown mirrors, and {len(releases)} published releases in {args.output}")


if __name__ == "__main__":
    main()
