#!/usr/bin/env python3
"""Build the bilingual, JavaScript-optional product website using the stdlib."""
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

DOC_PAGES = ("docs", "docs/workflows", "docs/agents", "docs/cli", "docs/remote-hosts")
PAGES = ("", "install", *DOC_PAGES, "changelog")
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


def route(lang, page=""):
    return ("/zh/" if lang == "zh" else "/") + (page + "/" if page else "")


def md_route(lang, page=""):
    prefix = "/zh/" if lang == "zh" else "/"
    return prefix + (f"{page}.md" if page else "index.md")


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
            if lang_hint in ("yaml", "yml", "sh", "bash"):
                label = html.escape(lang_hint or "code")
                blocks.append(
                    f'<div class="code-block"><div class="code-block-head"><span>{label}</span>'
                    f'<button class="copy-btn" type="button" data-copy-btn data-copy-value="{html.escape(code_text, quote=True)}">'
                    f'<span class="copy-text">{html.escape(labels["copy"])}</span></button></div>'
                    f'<pre><code>{html.escape(code_text)}</code></pre></div>'
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


def localize_home(lang, strings):
    body = (SOURCE / "templates/home.html").read_text()
    pattern = r'(<(?P<tag>[\w-]+)\b[^>]*\bdata-i18n="(?P<key>[^"]+)"[^>]*>)(.*?)(</(?P=tag)>)'

    def replace(match):
        key = match["key"]
        value = strings[key]
        # This sole trusted source translation deliberately contains an inline code tag.
        return match[1] + (value if key == "tl_mile" else html.escape(value)) + match[5]

    body = re.sub(pattern, replace, body, flags=re.DOTALL)
    for key, value in {
        "en_url": route("en"),
        "zh_url": route("zh"),
        "install_url": route(lang, "install"),
        "docs_url": route(lang, "docs"),
        "workflows_url": route(lang, "docs/workflows"),
        "cli_url": route(lang, "docs/cli"),
        "changelog_url": route(lang, "changelog"),
    }.items():
        body = body.replace("{{" + key + "}}", value)
    body = body.replace(f'data-language="{lang}"', f'data-language="{lang}" aria-current="page"')
    return body


def nav_active(page, key):
    if page == key:
        return True
    return key == "docs" and page.startswith("docs/") and not any(
        nav_key == page for nav_key, _ in NAV_PAGES
    )


def header(lang, page, labels):
    links = "".join(
        f'<a href="{route(lang, key)}"' + (' aria-current="page"' if nav_active(page, key) else "")
        + f'>{html.escape(labels[name])}</a>'
        for key, name in NAV_PAGES
    )
    languages = "".join(
        f'<a class="lang-btn" href="{route(code, page)}" lang="{code}" hreflang="{code}"'
        + (' aria-current="page"' if code == lang else "") + f'>{name}</a>'
        for code, name in (("en", "EN"), ("zh", "中文"))
    )
    return f'''<header class="doc-header">
      <a class="crt-brand" href="{route(lang)}"><span class="power-led" aria-hidden="true"></span>
      <img src="/favicon.svg" width="20" height="20" alt="">CANBAN</a>
      <nav class="doc-nav" aria-label="{'主导航' if lang == 'zh' else 'Main navigation'}">{links}</nav>
      <nav class="lang-switch" aria-label="Language">{languages}</nav></header>'''


def command(text, labels):
    return f'''<div class="doc-command"><pre><code>{html.escape(text)}</code></pre>
      <button class="copy-btn" type="button" data-copy-btn data-copy-value="{html.escape(text, quote=True)}">
      <span class="copy-text">{labels['copy']}</span></button></div>'''


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
    return f'''<footer class="doc-footer"><span>Apache-2.0 · © 2026 JJy</span><nav class="page-links">
      <a href="{route(lang, 'install')}">{html.escape(labels['install'])}</a>
      <a href="{route(lang, 'docs')}">{html.escape(labels['docs'])}</a>
      <a href="{route(lang, 'docs/workflows')}">{html.escape(labels['workflows'])}</a>
      <a href="{route(lang, 'docs/cli')}">{html.escape(labels['cli'])}</a>
      <a href="{route(lang, 'changelog')}">{html.escape(labels['changelog'])}</a>
      <a href="{md_route(lang, page)}">{html.escape(labels['markdown_mirror'])}</a>
      <a href="/llms-full.txt">{html.escape(labels['llms_full'])}</a>
      <a href="{REPOSITORY}">GitHub</a></nav></footer>'''


def install_body(lang, labels):
    t = lambda key: html.escape(labels[key])
    faq = "".join(f'<article class="faq-item"><h3>{html.escape(q)}</h3><p>{html.escape(a)}</p></article>'
                  for q, a in labels["faq_items"])
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
    <section id="updates"><h2>{t('updates')}</h2><p>{t('updates_text')}</p>
      <a class="text-link" href="{route(lang, 'changelog')}">{t('releases_link')} →</a></section>
    <section><h2>{t('source')}</h2><p>{t('source_text')}</p>
      <a class="text-link" href="{REPOSITORY}#build-from-source">GitHub README →</a></section>
    <section id="faq"><h2>{t('faq')}</h2>{faq}</section></main>{footer(lang, 'install', labels)}</div>'''


def changelog_body(lang, labels, releases):
    entries = []
    for release in releases:
        tag = html.escape(release["tag_name"])
        url = html.escape(release["html_url"], quote=True)
        stamp = release["published_at"]
        date = stamp[:10]
        badge = f'<span class="release-badge">{labels["prerelease"]}</span>' if release["prerelease"] else ""
        title = html.escape(release["name"] or release["tag_name"])
        notes = release_markdown(release["body"] or labels["no_notes"])
        entries.append(f'''<article class="release" id="{tag}"><div class="release-heading">
          <h2><a href="{url}">{title}</a></h2>{badge}<time datetime="{stamp}" title="{stamp}">{date}</time></div>
          <div class="release-notes">{notes}</div><a class="text-link" href="{url}">{labels['github_release']} →</a></article>''')
    return f'''<div class="doc-shell">{header(lang, 'changelog', labels)}
      <main id="main" class="doc-main"><div class="doc-intro"><p class="eyebrow">CANBAN // CHANGELOG</p>
      <h1>{labels['changelog']}</h1><p>{labels['release_intro']}</p><p class="muted">{labels['release_note']}</p></div>
      {''.join(entries)}<a class="text-link" href="{REPOSITORY}/releases">{labels['all_releases']} →</a>
      </main>{footer(lang, 'changelog', labels)}</div>'''


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
        f'{doc_markdown(sec["markdown"], labels)}{render_screenshot(sec.get("screenshot"))}</section>'
        for sec in spec["sections"]
    )
    return f'''<div class="doc-shell">{header(lang, page, labels)}
      <main id="main" class="doc-main"><div class="doc-intro">{breadcrumb_nav}<p class="eyebrow">{html.escape(spec['eyebrow'])}</p>
      <h1>{html.escape(spec['title'])}</h1><p>{inline_markdown(spec['intro'], allow_relative=True)}</p>{intro_shot}</div>
      {cards_html}{sections_html}</main>{footer(lang, page, labels)}</div>'''


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


def structured_data(lang, page, title, description, strings, labels, docs_spec=None, latest_release=None):
    """Build page-scoped Schema.org JSON-LD without invisible FAQ claims."""
    language = "zh-CN" if lang == "zh" else "en"
    home_url = ORIGIN + route(lang)
    page_url = ORIGIN + route(lang, page)
    schema = copy.deepcopy(load("schema.json"))
    graph = schema["@graph"]
    for item in graph:
        item["@id"] = home_url + ("#website" if item["@type"] == "WebSite" else "#software")
        item["url"] = home_url
        item["inLanguage"] = language
        item["description"] = strings["meta_desc"]
        if item["@type"] == "SoftwareApplication":
            item["installUrl"] = ORIGIN + route(lang, "install")
            item["releaseNotes"] = ORIGIN + route(lang, "changelog")
            if page:
                item.pop("featureList", None)
            if latest_release:
                item["softwareVersion"] = latest_release["tag_name"]
                item["dateModified"] = latest_release["published_at"]

    if page in DOC_PAGES:
        article = {
            "@type": "TechArticle",
            "@id": page_url + "#webpage",
            "url": page_url,
            "headline": title,
            "name": title,
            "description": description,
            "inLanguage": language,
            "author": {"@type": "Person", "name": "JJy", "url": "https://github.com/jjyr"},
            "isPartOf": {"@id": home_url + "#website"},
            "about": {"@id": home_url + "#software"},
        }
        if latest_release:
            article["dateModified"] = latest_release["published_at"]
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
    if page == "install":
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
                for question, answer in labels["faq_items"]
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


def document(lang, page, body, strings, labels, docs_spec=None, latest_release=None):
    path = route(lang, page)
    title, description = page_meta(page, strings, labels, docs_spec)
    locale, language = ("zh_CN", "zh-CN") if lang == "zh" else ("en_US", "en")
    schema = structured_data(lang, page, title, description, strings, labels, docs_spec, latest_release)
    alternates = "\n".join(f'<link rel="alternate" hreflang="{code}" href="{ORIGIN + route(value, page)}">'
                            for code, value in (("en", "en"), ("zh-CN", "zh"), ("x-default", "en")))
    md_alternate = f'<link rel="alternate" type="text/markdown" href="{ORIGIN + md_route(lang, page)}">'
    # A tiny early redirect preserves legacy bookmarks, other parameters and channel hashes.
    legacy = '''<script>(function(){var u=new URL(location.href),l=u.searchParams.get("lang");
      if(l!=="en"&&l!=="zh"&&l!=="zh-CN")return;u.searchParams.delete("lang");
      var p=u.pathname.replace(/^\\/zh(?=\\/)/,"");
      location.replace((l==="en"?p:"/zh"+p)+u.search+u.hash);})();</script>'''
    return f'''<!DOCTYPE html>
<html lang="{language}" data-lang="{lang}" class="{'home-root' if not page else 'document-root'}"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title><meta name="description" content="{html.escape(description, quote=True)}">
<meta name="application-name" content="Canban"><meta name="apple-mobile-web-app-title" content="Canban">
<meta name="author" content="JJy"><meta name="robots" content="index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1">
<meta name="theme-color" content="#141511"><link rel="canonical" href="{ORIGIN + path}">{alternates}
{md_alternate}
<meta property="og:site_name" content="Canban"><meta property="og:type" content="website">
<meta property="og:title" content="{html.escape(title, quote=True)}"><meta property="og:description" content="{html.escape(description, quote=True)}">
<meta property="og:url" content="{ORIGIN + path}"><meta property="og:locale" content="{locale}">
<meta property="og:locale:alternate" content="{'en_US' if lang == 'zh' else 'zh_CN'}">
<meta property="og:image" content="{ORIGIN}/og-image.png"><meta property="og:image:type" content="image/png">
<meta property="og:image:width" content="1200"><meta property="og:image:height" content="630">
<meta property="og:image:alt" content="{html.escape(strings['hero_subtitle'], quote=True)}">
<meta name="twitter:card" content="summary_large_image"><meta name="twitter:title" content="{html.escape(title, quote=True)}">
<meta name="twitter:description" content="{html.escape(description, quote=True)}"><meta name="twitter:image" content="{ORIGIN}/og-image.png">
<meta name="twitter:image:alt" content="{html.escape(strings['hero_subtitle'], quote=True)}">
<link rel="icon" type="image/svg+xml" href="/favicon.svg"><link rel="icon" type="image/png" sizes="48x48" href="/favicon-48x48.png">
<link rel="icon" type="image/png" sizes="192x192" href="/icon-192.png">
<link rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon.png"><link rel="manifest" href="/site.webmanifest">
<script type="application/ld+json">{script_json(schema)}</script>{legacy}
<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&amp;family=JetBrains+Mono:wght@400;500;600;700&amp;display=swap" rel="stylesheet">
<link rel="stylesheet" href="/styles.css"><link rel="stylesheet" href="/pages.css">
<noscript><style>.home-root{{height:auto;overflow:auto}}.home-page{{height:auto;overflow:visible}}.home-page .crt-shell{{height:auto;display:block}}
.home-page .crt-glass{{overflow:visible}}.home-page .crt-screen{{position:relative;visibility:visible;opacity:1;pointer-events:auto}}
.home-page .crt-topbar,.home-page .crt-bottombar{{min-height:48px}}</style></noscript>
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
        strings["hero_subtitle"],
        "",
        strings["hero_description"],
        "",
        "## Highlights",
        "",
        f"- **Kanban**: {strings['tab_1_desc']} — {strings['al_1']}",
        f"- **Workflow**: {strings['tab_2_desc']} — {strings['al_3']}",
        f"- **Herdr**: {strings['tab_3_desc']} — {strings['al_2']}",
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
        "## Navigation",
        "",
        f"- {strings['nav_install']}: {ORIGIN + route(lang, 'install')}",
        f"- {strings['nav_docs']}: {ORIGIN + route(lang, 'docs')}",
        f"- {strings['nav_workflows']}: {ORIGIN + route(lang, 'docs/workflows')}",
        f"- {strings['nav_cli']}: {ORIGIN + route(lang, 'docs/cli')}",
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


def render_doc_markdown_mirror(lang, spec):
    lines = [f"# {spec['title']}", "", f"> {spec['intro']}", ""]
    lines.extend(screenshot_markdown(spec.get("screenshot")))
    if spec.get("cards"):
        for card in spec["cards"]:
            lines.append(f"- [{card['title']}]({ORIGIN + card['href']}): {card['summary']}")
        lines.append("")
    for sec in spec["sections"]:
        lines.extend([f"## {sec['heading']}", "", sec["markdown"].strip(), ""])
        lines.extend(screenshot_markdown(sec.get("screenshot")))
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
        "> Canban (`canban.dev`, GitHub: `jjyr/canban`, Rust + NSH + `libghostty-vt` + `herdr-core`) is an open-source native macOS and Linux Kanban workbench for AI coding agents. Formula: `Canban = Kanban + Workflow + Herdr`.",
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
    for slug in DOC_PAGES:
        parts.extend(["---", "", render_doc_markdown_mirror("en", docs["en"][slug]).strip(), ""])

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
        screenshots_dst.mkdir(parents=True, exist_ok=True)
        for shot in sorted(screenshots_src.glob("*.png")):
            shutil.copyfile(shot, screenshots_dst / shot.name)
    shutil.copyfile(ROOT / "scripts/install.sh", output / "install.sh")
    locales, pages, docs = load("locales.json"), load("pages.json"), load("docs.json")
    latest_release = releases[0] if releases else None
    lastmod = f'<lastmod>{latest_release["published_at"][:10]}</lastmod>' if latest_release else ""

    for lang in ("en", "zh"):
        for page in PAGES:
            docs_spec = docs[lang][page] if page in DOC_PAGES else None
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
                md_text = render_doc_markdown_mirror(lang, docs_spec)

            destination = output / route(lang, page).lstrip("/") / "index.html"
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(document(lang, page, body, locales[lang], pages[lang], docs_spec, latest_release))

            md_destination = output / md_route(lang, page).lstrip("/")
            md_destination.parent.mkdir(parents=True, exist_ok=True)
            md_destination.write_text(md_text)

    (output / "llms-full.txt").write_text(render_llms_full(locales, pages, docs, releases))

    urls = []
    for page in PAGES:
        alternates = "".join(f'<xhtml:link rel="alternate" hreflang="{code}" href="{ORIGIN + route(lang, page)}" />'
                             for code, lang in (("en", "en"), ("zh-CN", "zh"), ("x-default", "en")))
        for lang in ("en", "zh"):
            urls.append(f'<url><loc>{ORIGIN + route(lang, page)}</loc>{lastmod}{alternates}</url>')
    (output / "sitemap.xml").write_text('<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n' + "\n".join(urls) + "\n</urlset>\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / ".local/website")
    parser.add_argument("--refresh-releases", action="store_true", help="Fetch published GitHub Releases; fail rather than publish stale history on API errors")
    args = parser.parse_args()
    releases = fetch_releases() if args.refresh_releases else published_releases(load("releases.json"))
    build(args.output, releases)
    print(f"Built {len(PAGES) * 2} HTML pages, {len(PAGES) * 2} Markdown mirrors, and {len(releases)} published releases in {args.output}")


if __name__ == "__main__":
    main()
