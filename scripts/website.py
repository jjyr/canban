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


def load(name):
    return json.loads((SOURCE / name).read_text())


def route(lang, page=""):
    return ("/zh/" if lang == "zh" else "/") + (page + "/" if page else "")


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


def inline_markdown(text):
    """Render the subset used by release notes; never trust embedded HTML or URLs."""
    pattern = r"`([^`]+)`|\[([^\]]+)\]\(([^\s)]+)\)|\*\*([^*]+)\*\*"
    output, cursor = [], 0
    for match in re.finditer(pattern, text):
        output.append(html.escape(text[cursor:match.start()]))
        code, label, url, strong = match.groups()
        if code is not None:
            output.append("<code>" + html.escape(code) + "</code>")
        elif label is not None and urlsplit(url).scheme in ("https", "http"):
            output.append(f'<a href="{html.escape(url, quote=True)}">{html.escape(label)}</a>')
        elif strong is not None:
            output.append("<strong>" + html.escape(strong) + "</strong>")
        else:
            output.append(html.escape(match.group()))
        cursor = match.end()
    output.append(html.escape(text[cursor:]))
    return "".join(output)


def release_markdown(text):
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


def localize_home(lang, strings):
    body = (SOURCE / "templates/home.html").read_text()
    pattern = r'(<(?P<tag>[\w-]+)\b[^>]*\bdata-i18n="(?P<key>[^"]+)"[^>]*>)(.*?)(</(?P=tag)>)'

    def replace(match):
        key = match["key"]
        value = strings[key]
        # This sole trusted source translation deliberately contains an inline code tag.
        return match[1] + (value if key == "tl_mile" else html.escape(value)) + match[5]

    body = re.sub(pattern, replace, body, flags=re.DOTALL)
    for key, value in {"en_url": route("en"), "zh_url": route("zh"),
                       "install_url": route(lang, "install"),
                       "changelog_url": route(lang, "changelog")}.items():
        body = body.replace("{{" + key + "}}", value)
    body = body.replace(f'data-language="{lang}"', f'data-language="{lang}" aria-current="page"')
    return body


def header(lang, page, labels):
    links = "".join(
        f'<a href="{route(lang, key)}"' + (' aria-current="page"' if page == key else "")
        + f'>{html.escape(labels[name])}</a>'
        for key, name in (("", "home"), ("install", "install"), ("changelog", "changelog"))
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


def install_body(lang, labels):
    t = lambda key: html.escape(labels[key])
    faq = "".join(f'<article class="faq-item"><h3>{html.escape(q)}</h3><p>{html.escape(a)}</p></article>'
                  for q, a in labels["faq_items"])
    dependencies = "".join(f'<h3>{name}</h3><pre><code>{html.escape(cmd)}</code></pre>' for name, cmd in (
        ("Ubuntu / Debian", "sudo apt install libgtk-4-1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0"),
        ("Arch Linux", "sudo pacman -S gtk4 gstreamer gst-plugins-base"),
        ("Fedora", "sudo dnf install gtk4 gstreamer1 gstreamer1-plugins-base"),
    ))
    return f'''<div class="doc-shell">{header(lang, 'install', labels)}
    <main id="main" class="doc-main"><div class="doc-intro"><p class="eyebrow">CANBAN // INSTALL</p>
      <h1>{t('install_title')}</h1><p>{t('install_intro')}</p></div>
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
    <section id="faq"><h2>{t('faq')}</h2>{faq}</section></main>{footer(lang, labels)}</div>'''


def footer(lang, labels):
    return f'''<footer class="doc-footer"><span>Apache-2.0 · © 2026 JJy</span><nav class="page-links">
      <a href="{route(lang, 'install')}">{labels['install']}</a>
      <a href="{route(lang, 'changelog')}">{labels['changelog']}</a>
      <a href="{REPOSITORY}">GitHub</a></nav></footer>'''


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
      </main>{footer(lang, labels)}</div>'''


def structured_data(lang, page, title, description, strings, labels, latest_release=None):
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
            "itemListElement": [
                {"@type": "ListItem", "position": 1, "name": labels["home"], "item": home_url},
                {"@type": "ListItem", "position": 2, "name": labels[page], "item": page_url},
            ],
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


def document(lang, page, body, strings, labels, latest_release=None):
    path = route(lang, page)
    title = strings["meta_title"] if not page else labels["install_title"] if page == "install" else labels["changelog"] + " — Canban"
    description = strings["meta_desc"] if not page else labels["install_desc"] if page == "install" else labels["release_intro"]
    locale, language = ("zh_CN", "zh-CN") if lang == "zh" else ("en_US", "en")
    schema = structured_data(lang, page, title, description, strings, labels, latest_release)
    alternates = "\n".join(f'<link rel="alternate" hreflang="{code}" href="{ORIGIN + route(value, page)}">'
                            for code, value in (("en", "en"), ("zh-CN", "zh"), ("x-default", "en")))
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


def build(output, releases):
    output.mkdir(parents=True, exist_ok=True)
    for source in SOURCE.iterdir():
        if source.suffix in (".css", ".js", ".png", ".svg", ".webmanifest") or source.name in ("CNAME", "robots.txt", "llms.txt"):
            shutil.copyfile(source, output / source.name)
    shutil.copyfile(ROOT / "scripts/install.sh", output / "install.sh")
    locales, pages = load("locales.json"), load("pages.json")
    latest_release = releases[0] if releases else None
    lastmod = f'<lastmod>{latest_release["published_at"][:10]}</lastmod>' if latest_release else ""
    for lang in ("en", "zh"):
        for page in ("", "install", "changelog"):
            body = localize_home(lang, locales[lang]) if not page else install_body(lang, pages[lang]) if page == "install" else changelog_body(lang, pages[lang], releases)
            destination = output / route(lang, page).lstrip("/") / "index.html"
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(document(lang, page, body, locales[lang], pages[lang], latest_release))
    urls = []
    for page in ("", "install", "changelog"):
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
    print(f"Built 6 pages and {len(releases)} published releases in {args.output}")


if __name__ == "__main__":
    main()
