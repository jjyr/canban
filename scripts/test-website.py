#!/usr/bin/env python3
"""Check the generated 5-language website's crawlable content, GEO mirrors, and release trust boundary."""
# Copyright 2026 JJy <jjyruby@gmail.com>
# SPDX-License-Identifier: Apache-2.0

import json
import re
import tempfile
import unittest
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

import website

BUILTIN_24_CLI_IDS = (
    "codex", "claude", "agy", "gemini", "opencode", "cursor", "pi", "omp",
    "copilot", "droid", "amp", "kimi", "kiro", "qwen", "devin", "cline",
    "grok", "hermes", "kilo", "qodercli", "letta", "mastracode", "maki", "muse",
)


class Page(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.elements = []
        self.visible_text = []
        self.prose_text = []
        self.scripts = []
        self.in_script = False
        self._code_depth = 0
        self.header_nav_links = []
        self._in_site_nav = False
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        attr_map = dict(attrs)
        self.elements.append((tag, attr_map))
        if tag in ("code", "pre"):
            self._code_depth += 1
        if tag == "nav" and "site-nav" in attr_map.get("class", "").split():
            self._in_site_nav = True
        if self._in_site_nav and tag == "a" and attr_map.get("href"):
            self.header_nav_links.append(attr_map["href"])
        if tag == "script":
            self.in_script = True
            self.scripts.append((attr_map, ""))

    def handle_endtag(self, tag):
        if tag in ("code", "pre") and self._code_depth > 0:
            self._code_depth -= 1
        if tag == "script":
            self.in_script = False
        if self._in_site_nav and tag == "nav":
            self._in_site_nav = False

    def handle_data(self, data):
        if self.in_script:
            attrs, text = self.scripts[-1]
            self.scripts[-1] = (attrs, text + data)
        else:
            self.visible_text.append(data)
            if self._code_depth == 0:
                self.prose_text.append(data)

    def attrs(self, tag, **attributes):
        return [attrs for name, attrs in self.elements if name == tag and
                all(attrs.get(key) == value for key, value in attributes.items())]


class WebsiteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.output = Path(cls.temp.name)
        cls.releases = website.published_releases(website.load("releases.json"))
        website.build(cls.output, cls.releases)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_localized_content_metadata_and_language_pairs_without_javascript(self):
        pages_json = website.load("pages.json")
        locales_json = website.load("locales.json")
        schema_json = website.load("schema.json")
        docs_by_lang = {lang: website.load_docs(lang) for lang in website.LANGUAGE_CODES}

        for info in website.LANGUAGES:
            lang = info["code"]
            for section in website.PAGES:
                with self.subTest(lang=lang, section=section):
                    route = website.route(lang, section)
                    text = (self.output / route.lstrip("/") / "index.html").read_text()
                    page = Page(text)
                    self.assertEqual(len(page.attrs("h1")), 1)
                    self.assertEqual(page.attrs("html")[0]["lang"], info["html_lang"])
                    self.assertEqual(page.attrs("link", rel="canonical"), [{"rel": "canonical", "href": website.ORIGIN + route}])

                    robots = page.attrs("meta", name="robots")[0]["content"]
                    if section == "changelog":
                        if lang == "en":
                            self.assertTrue(robots.startswith("index, follow"))
                        else:
                            self.assertEqual(robots, "noindex, follow")
                        self.assertEqual(page.attrs("link", rel="alternate", hreflang="en"), [])
                        self.assertEqual(page.attrs("link", rel="alternate", hreflang="x-default"), [])
                        self.assertEqual(page.attrs("meta", property="og:locale:alternate"), [])
                    else:
                        self.assertTrue(robots.startswith("index, follow"))
                        for target in website.LANGUAGES:
                            self.assertEqual(
                                page.attrs("link", rel="alternate", hreflang=target["hreflang"])[0]["href"],
                                website.ORIGIN + website.route(target["code"], section),
                            )
                        self.assertEqual(
                            page.attrs("link", rel="alternate", hreflang="x-default")[0]["href"],
                            website.ORIGIN + website.route("en", section),
                        )
                        og_alts = {m["content"] for m in page.attrs("meta", property="og:locale:alternate")}
                        expected_og_alts = {item["og_locale"] for item in website.LANGUAGES if item["code"] != lang}
                        self.assertEqual(og_alts, expected_og_alts)

                    md_alts = page.attrs("link", rel="alternate", type="text/markdown")
                    self.assertEqual(len(md_alts), 1)
                    self.assertEqual(md_alts[0]["href"], website.ORIGIN + website.md_route(lang, section))
                    self.assertEqual(page.attrs("meta", property="og:url")[0]["content"], website.ORIGIN + route)
                    self.assertEqual(page.attrs("meta", property="og:locale")[0]["content"], info["og_locale"])
                    self.assertIsNone(re.search(r"\{\{(?:lang_switcher|(?:en|zh|install|docs|workflows|cli|guide_parallel|guide_native|guide_worktree_gc|changelog|android_apk|obtainium)_url)\}\}", text))
                    if section not in ("docs/workflows", "guides/scheduled-worktree-cleanup"):
                        self.assertNotIn("{{", text)
                    self.assertNotIn("localStorage", text)
                    self.assertNotIn("schema v11", text.lower())
                    self.assertNotIn("schema: 10", text.lower())
                    self.assertNotIn("--allow-insecure-bind", text)
                    self.assertNotIn("enc:v1:", text)
                    self.assertNotIn("remove-legacy-token", text)
                    self.assertNotIn("host.probe", text.lower())

                    lang_links = [
                        attrs for tag, attrs in page.elements
                        if tag == "a" and "lang-btn" in attrs.get("class", "").split()
                    ]
                    self.assertEqual(len(lang_links), len(website.LANGUAGES))
                    self.assertEqual({a["href"] for a in lang_links}, {website.route(c, section) for c in website.LANGUAGE_CODES})

                    localized_schema = schema_json["localized"][lang]
                    for attrs, script in page.scripts:
                        if attrs.get("type") == "application/ld+json":
                            graph = json.loads(script)["@graph"]
                            by_type = {item["@type"]: item for item in graph}
                            ws = by_type["WebSite"]
                            self.assertEqual(ws["inLanguage"], info["hreflang"])
                            self.assertEqual(ws["description"], localized_schema["website_desc"])

                            software = by_type["SoftwareApplication"]
                            self.assertEqual(software["inLanguage"], info["hreflang"])
                            self.assertEqual(software["description"], localized_schema["software_desc"])
                            self.assertEqual(software["applicationSubCategory"], localized_schema["application_sub_category"])
                            self.assertEqual(software["softwareRequirements"], localized_schema["software_requirements"])
                            self.assertEqual(software["keywords"], localized_schema["keywords"])
                            self.assertEqual(software["softwareVersion"], self.releases[0]["tag_name"])
                            self.assertEqual(software["dateModified"], self.releases[0]["published_at"])
                            self.assertEqual(software["installUrl"], website.ORIGIN + website.route(lang, "install"))
                            self.assertEqual(software["releaseNotes"], website.ORIGIN + website.route(lang, "changelog"))
                            screenshots = software["screenshot"]
                            self.assertIsInstance(screenshots, list)
                            self.assertEqual(len(screenshots), 8)
                            for shot_url in screenshots:
                                self.assertTrue(shot_url.startswith(website.ORIGIN + "/screenshots/"), shot_url)
                                shot_path = self.output / urlsplit(shot_url).path.lstrip("/")
                                self.assertTrue(shot_path.is_file(), shot_path)

                            if section in website.ARTICLE_PAGES:
                                spec = docs_by_lang[lang][section]
                                article = by_type["TechArticle"]
                                self.assertEqual(article["url"], website.ORIGIN + route)
                                self.assertEqual(article["mainEntityOfPage"], {"@type": "WebPage", "@id": website.ORIGIN + route})
                                self.assertEqual(article["inLanguage"], info["hreflang"])
                                self.assertEqual(article["datePublished"], spec["date_published"])
                                self.assertEqual(article["dateModified"], spec["updated"])
                                self.assertEqual(article["publisher"]["name"], "Canban")
                                self.assertTrue(article["image"].startswith(website.ORIGIN + "/"), article["image"])
                                self.assertIn("BreadcrumbList", by_type)
                                expected_crumbs = 3 if ("/" in section) else 2
                                self.assertEqual(len(by_type["BreadcrumbList"]["itemListElement"]), expected_crumbs)
                                if spec.get("faq_items"):
                                    faq = by_type["FAQPage"]
                                    expected_faqs = [(q, a) for q, a in spec["faq_items"]]
                                    actual_faqs = [(it["name"], it["acceptedAnswer"]["text"]) for it in faq["mainEntity"]]
                                    self.assertEqual(actual_faqs, expected_faqs)
                                else:
                                    self.assertNotIn("FAQPage", by_type)
                            else:
                                webpage = by_type["CollectionPage" if section == "changelog" else "WebPage"]
                                self.assertEqual(webpage["url"], website.ORIGIN + route)
                                if not section:
                                    self.assertEqual(software["featureList"], localized_schema["feature_list"])
                                    self.assertNotIn("FAQPage", by_type)
                                    self.assertNotIn("BreadcrumbList", by_type)
                                else:
                                    self.assertNotIn("featureList", software)
                                    self.assertIn("BreadcrumbList", by_type)
                                if section == "install":
                                    faq = by_type["FAQPage"]
                                    expected_faqs = [(q, a) for q, a in pages_json[lang]["faq_items"]]
                                    actual_faqs = [(it["name"], it["acceptedAnswer"]["text"]) for it in faq["mainEntity"]]
                                    self.assertEqual(actual_faqs, expected_faqs)
                                elif section == "changelog":
                                    self.assertNotIn("FAQPage", by_type)
                        if attrs.get("type") == "application/json":
                            self.assertIn("copied_label", json.loads(script))

                    visible = " ".join(page.visible_text)
                    prose = " ".join(page.prose_text)
                    self.assertIsNone(re.search(r"`[^`]+`", prose), (lang, section))
                    if section == "install":
                        for question, answer in pages_json[lang]["faq_items"]:
                            self.assertIn(question, visible)
                            self.assertIn(answer, visible)
                        self.assertIn("Herdr", visible)
                        self.assertIn(website.HEADLESS, visible)
                        self.assertTrue(page.attrs("section", id="mobile"), (lang, section))
                        hrefs = {attrs.get("href") for tag, attrs in page.elements if tag == "a"}
                        self.assertIn(website.ANDROID_APK_URL, hrefs, (lang, section))
                        self.assertIn(website.OBTAINIUM_URL, hrefs, (lang, section))
                        self.assertIn(website.IOS_ISSUE_URL, hrefs, (lang, section))
                    if section in website.ARTICLE_PAGES and docs_by_lang[lang][section].get("faq_items"):
                        for question, answer in docs_by_lang[lang][section]["faq_items"]:
                            self.assertIn(question, visible)
                            self.assertIn(answer, visible)
                    if section == "docs/agents":
                        title_text = docs_by_lang[lang][section]["meta_title"]
                        self.assertNotIn("24", title_text)
                    if not section:
                        self.assertIn(locales_json[lang]["hero_title"], visible)
                        self.assertEqual(len(page.attrs("title")), 1)
                        self.assertIn("Kanban + Workflow + Herdr", text)
                        expected_nav = {
                            website.route(lang, hub)
                            for hub in ("install", "docs", "docs/workflows", "docs/cli", "changelog")
                        }
                        self.assertEqual(set(page.header_nav_links), expected_nav)
                        hrefs = {attrs.get("href") for tag, attrs in page.elements if tag == "a"}
                        self.assertIn(f"{website.route(lang, 'install')}#mobile", hrefs, lang)
                        self.assertIn(website.ANDROID_APK_URL, hrefs, lang)
                        self.assertIn(website.OBTAINIUM_URL, hrefs, lang)
                        self.assertIn(website.IOS_ISSUE_URL, hrefs, lang)
                        install_tabs = [
                            attrs.get("data-install-tab")
                            for tag, attrs in page.elements
                            if tag == "button" and attrs.get("data-install-tab")
                        ]
                        self.assertEqual(install_tabs, ["desktop", "headless", "android"], lang)
                        self.assertNotIn("mobile-pill", text, lang)
                        android_panel = page.attrs("div", id="closing-install-android")
                        self.assertEqual(len(android_panel), 1, lang)
                        self.assertIn("hidden", android_panel[0], lang)
                        home_shots = [img.get("src") for img in page.attrs("img") if img.get("src", "").startswith("/screenshots/")]
                        self.assertEqual(
                            home_shots,
                            [
                                "/screenshots/daily-briefing.webp",
                                "/screenshots/board.webp",
                                "/screenshots/terminals.webp",
                                "/screenshots/git-review.webp",
                                "/screenshots/workflows.webp",
                                "/screenshots/mobile-inbox.webp",
                                "/screenshots/mobile-board.webp",
                                "/screenshots/mobile-terminal.webp",
                            ],
                            lang,
                        )
                        showcase_tabs = [
                            attrs.get("data-showcase-tab")
                            for tag, attrs in page.elements
                            if tag == "button" and attrs.get("data-showcase-tab")
                        ]
                        self.assertEqual(
                            showcase_tabs,
                            ["daily-briefing", "board", "terminals", "git-review", "workflows"],
                            lang,
                        )
                        provider_icons = {
                            img.get("src")
                            for img in page.attrs("img")
                            if img.get("src", "").startswith("/providers/")
                        }
                        self.assertEqual(
                            provider_icons,
                            {f"/providers/{cli_id}.svg" for cli_id in BUILTIN_24_CLI_IDS},
                            lang,
                        )

                    expected_screenshots = {
                        "install": ("/screenshots/daily-briefing.webp", False),
                        "docs": ("/screenshots/git-review.webp", True),
                        "docs/workflows": ("/screenshots/workflows.webp", True),
                        "docs/agents": ("/screenshots/terminals.webp", True),
                        "guides/native-terminal-and-multi-machine": ("/screenshots/workflows.webp", True),
                    }
                    if section in expected_screenshots:
                        expected_src, expect_lazy = expected_screenshots[section]
                        shot_imgs = [img for img in page.attrs("img") if img.get("src") == expected_src]
                        self.assertEqual(len(shot_imgs), 1, (lang, section))
                        self.assertEqual(shot_imgs[0].get("width"), "2880")
                        self.assertEqual(shot_imgs[0].get("height"), "1800")
                        self.assertTrue(shot_imgs[0].get("alt"), (lang, section))
                        if expect_lazy:
                            self.assertEqual(shot_imgs[0].get("loading"), "lazy", (lang, section))
                        else:
                            self.assertNotIn("loading", shot_imgs[0], (lang, section))

    def test_removed_routes_are_not_generated(self):
        self.assertEqual(len(website.PAGES), 11)
        self.assertEqual(len(website.LANGUAGES), 5)
        sitemap = (self.output / "sitemap.xml").read_text()
        for info in website.LANGUAGES:
            prefix = f"{info['prefix']}/" if info["prefix"] else ""
            for slug in ("workflows", "compare"):
                removed = f"{prefix}{slug}"
                self.assertFalse((self.output / removed / "index.html").exists(), removed)
                self.assertFalse((self.output / f"{removed}.md").exists(), removed)
                self.assertNotIn(f"{website.ORIGIN}/{removed}/", sitemap)

    def test_markdown_mirrors_and_geo_files(self):
        for lang in website.LANGUAGE_CODES:
            for section in website.PAGES:
                with self.subTest(lang=lang, section=section):
                    md_path = self.output / website.md_route(lang, section).lstrip("/")
                    self.assertTrue(md_path.is_file(), md_path)
                    content = md_path.read_text()
                    self.assertTrue(content.startswith("# "), md_path)
                    self.assertNotIn("<script", content)
                    self.assertNotIn("schema v11", content.lower())
                    self.assertNotIn("schema: 10", content.lower())
                    self.assertNotIn("--allow-insecure-bind", content)
                    self.assertNotIn("enc:v1:", content)
                    self.assertNotIn("remove-legacy-token", content)
                    self.assertNotIn("host.probe", content.lower())

        agents_md = (self.output / "docs/agents.md").read_text()
        for cli_id in BUILTIN_24_CLI_IDS:
            self.assertIn(f"`{cli_id}`", agents_md)
        self.assertIn("canban card adopt", agents_md)
        self.assertIn("libghostty-vt", agents_md)
        for term in ("viewer.presence", "Reclaiming soon", "Orchestrator"):
            self.assertIn(term, agents_md)

        workflows_doc_md = (self.output / "docs/workflows.md").read_text()
        for term in (
            "schema: 13", "development.yaml", "default_agent", "admit", "script", "deliver",
            "condition", "approval", "card.move", "notify", "complete",
            "external_pr", "on_changes_requested", "on_failure", "_merge_queue",
            "cardless", "exclude_self", "secrets.", "vars.", "XChaCha20-Poly1305",
        ):
            self.assertIn(term, workflows_doc_md)

        cli_doc_md = (self.output / "docs/cli.md").read_text()
        for term in (
            "snapshot", "status", "call", "card show", "card adopt", "resolve", "reopen", "report",
            "reply", "bind-session", "review draft", "publish", "workflow sources", "schedule",
            "worktree gc", "batch report", "serve", "pair", "service", "device", "skill",
            "build-info", "--fields", "--full", "--client-pubkey", "authorize", "Noise_XX", "request_key", "CANBAN_CLI",
            "CANBAN_RUN_DIR", "canban skill",
            "npx skills add ~/.canban/skills --skill canban -g",
            "canban --human snapshot",
            "canban --human --fields number,title,stage card show",
            "canban --human call project.list '{}'",
        ):
            self.assertIn(term, cli_doc_md)
        for lang in ("zh", "zh-tw", "ja", "ko"):
            localized_cli_md = (self.output / website.md_route(lang, "docs/cli").lstrip("/")).read_text()
            for term in (
                "canban skill",
                "npx skills add ~/.canban/skills --skill canban -g",
                "canban --human snapshot",
                "canban --human --fields number,title,stage card show",
                "canban --human call project.list '{}'",
            ):
                self.assertIn(term, localized_cli_md, (lang, term))

        remote_md = (self.output / "docs/remote-hosts.md").read_text()
        for term in (
            "canband", "Tailscale", "SSH", "--headless", "yamux/1",
            "canban device", "pair_", "canban-sync", "Noise_XX", "authorized_devices.json",
            "trust_tailscale_self", "host.discover_canban", "WhoIs", "Tailscale Admin",
        ):
            self.assertIn(term, remote_md)
        for lang in website.LANGUAGE_CODES:
            docs_html = (self.output / website.route(lang, "docs").lstrip("/") / "index.html").read_text()
            self.assertIn('id="privacy-security"', docs_html, lang)
            localized_remote_md = (self.output / website.md_route(lang, "docs/remote-hosts").lstrip("/")).read_text()
            for term in ("Noise_XX", "trust_tailscale_self", "host.discover_canban", "WhoIs"):
                self.assertIn(term, localized_remote_md, (lang, term))

        parallel_md = (self.output / "guides/parallel-coding-agents.md").read_text()
        for term in ("Attention Inbox", "FIFO Queue Merge", "canban card adopt", "Antigravity", "Pi", "Insights"):
            self.assertIn(term, parallel_md)

        native_md = (self.output / "guides/native-terminal-and-multi-machine.md").read_text()
        for term in ("libghostty-vt", "herdr-core", "tmux", "Noise_XX", "Tailscale", "canband", "yamux/1", "canban-sync"):
            self.assertIn(term, native_md)

        gc_md = (self.output / "guides/scheduled-worktree-cleanup.md").read_text()
        for term in (
            "schema: 13", "scope: global", "worktree gc", "merge-base",
            "_merge_queue", "DRY_RUN", "project.list", "card.create",
            "card resolve", "create_worktree: false",
            "workflow sources --global", "--revision", "Settings → Environment & Secrets",
            "Describe with agent", "uses: agent",
        ):
            self.assertIn(term, gc_md)
        gc_html = (self.output / "guides/scheduled-worktree-cleanup/index.html").read_text()
        self.assertIn('class="code-block code-block-prompt"', gc_html)
        self.assertIn('<details class="doc-details">', gc_html)
        self.assertIn('<span class="tok-key">schema</span>', gc_html)
        self.assertIn('<span class="tok-num">13</span>', gc_html)
        for lang in website.LANGUAGE_CODES:
            localized_gc_md = (self.output / website.md_route(lang, "guides/scheduled-worktree-cleanup").lstrip("/")).read_text()
            self.assertNotIn("canban snapshot", localized_gc_md, lang)

        for lang in website.LANGUAGE_CODES:
            install_md = (self.output / website.md_route(lang, "install").lstrip("/")).read_text()
            self.assertIn(website.ANDROID_APK_URL, install_md, lang)
            self.assertIn(website.OBTAINIUM_URL, install_md, lang)

        llms_txt = (self.output / "llms.txt").read_text()
        self.assertIn("https://canban.dev/llms-full.txt", llms_txt)
        self.assertIn(website.ANDROID_APK_URL, llms_txt)
        self.assertIn(website.OBTAINIUM_URL, llms_txt)
        self.assertIn("Canban = Kanban + Workflow + Herdr", llms_txt)
        for slug in (
            "install",
            "docs",
            "docs/workflows",
            "docs/cli",
            "docs/agents",
            "docs/remote-hosts",
            "guides/parallel-coding-agents",
            "guides/native-terminal-and-multi-machine",
            "guides/scheduled-worktree-cleanup",
            "changelog",
        ):
            self.assertIn(f"https://canban.dev/{slug}.md", llms_txt)
        for prefix in ("zh", "zh-tw", "ja", "ko"):
            self.assertIn(f"https://canban.dev/{prefix}/", llms_txt)
        self.assertNotIn("https://canban.dev/workflows.md", llms_txt)
        self.assertNotIn("https://canban.dev/compare.md", llms_txt)

        llms_full = (self.output / "llms-full.txt").read_text()
        self.assertIn("Canban = Kanban + Workflow + Herdr", llms_full)
        self.assertIn(website.ANDROID_APK_URL, llms_full)
        self.assertIn(website.OBTAINIUM_URL, llms_full)
        self.assertIn("canban --human snapshot", llms_full)
        self.assertIn("canban card adopt", llms_full)
        self.assertIn("batch report", llms_full)
        self.assertIn("review draft", llms_full)
        self.assertIn("worktree gc", llms_full)
        self.assertIn("Run Multiple AI Coding Agents in Parallel with Kanban & Git Worktrees", llms_full)
        self.assertIn("Native Terminal Workbench vs. tmux & Chat Wrappers", llms_full)
        self.assertIn("Safely Reclaim Old Git Worktrees with a Pure-Script Schedule", llms_full)
        for cli_id in BUILTIN_24_CLI_IDS:
            self.assertIn(f"`{cli_id}`", llms_full)

    def test_links_assets_and_fragments_resolve(self):
        expected_screenshots = {
            "daily-briefing.webp",
            "board.webp",
            "terminals.webp",
            "git-review.webp",
            "workflows.webp",
            "mobile-inbox.webp",
            "mobile-board.webp",
            "mobile-terminal.webp",
        }
        source_shots = {
            path.name
            for path in (website.SOURCE / "screenshots").iterdir()
            if path.is_file() and path.suffix in (".webp", ".png")
        }
        self.assertEqual(source_shots, expected_screenshots)
        for name in expected_screenshots:
            built_shot = self.output / "screenshots" / name
            self.assertTrue(built_shot.is_file(), built_shot)
            self.assertEqual(built_shot.read_bytes(), (website.SOURCE / "screenshots" / name).read_bytes())

        referenced_screenshots = set()
        for source in self.output.rglob("*.html"):
            page = Page(source.read_text())
            base = "/" + str(source.parent.relative_to(self.output))
            for tag, attrs in page.elements:
                url = attrs.get("href") or attrs.get("src")
                if not url or urlsplit(url).scheme or url.startswith("//"):
                    continue
                if tag == "img" and url.startswith("/screenshots/"):
                    referenced_screenshots.add(url)
                parts = urlsplit(url)
                path = unquote(parts.path)
                destination = self.output / path.lstrip("/") if path.startswith("/") else self.output / base.lstrip("/") / path
                if destination.is_dir():
                    destination /= "index.html"
                self.assertTrue(destination.is_file(), (source, url))
                if parts.fragment and destination.suffix == ".html":
                    ids = [attrs.get("id") for _, attrs in Page(destination.read_text()).elements]
                    self.assertIn(parts.fragment, ids, (source, url))
        self.assertEqual(referenced_screenshots, {f"/screenshots/{name}" for name in expected_screenshots})

    def test_sitemap_has_indexable_pages_per_page_lastmod_and_reciprocal_alternates(self):
        ns = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9", "x": "http://www.w3.org/1999/xhtml"}
        tree = ET.parse(self.output / "sitemap.xml")
        entries = tree.findall("s:url", ns)
        locales_json = website.load("locales.json")
        pages_json = website.load("pages.json")
        docs_by_lang = {lang: website.load_docs(lang) for lang in website.LANGUAGE_CODES}

        expected = {
            website.ORIGIN + website.route(lang, page)
            for lang in website.LANGUAGE_CODES
            for page in website.PAGES
            if website.is_indexable(lang, page)
        }
        self.assertEqual(len(expected), 51)
        self.assertEqual({entry.find("s:loc", ns).text for entry in entries}, expected)

        for lang in ("zh", "zh-tw", "ja", "ko"):
            self.assertNotIn(website.ORIGIN + website.route(lang, "changelog"), expected)

        expected_lastmod = {
            website.ORIGIN + website.route(lang, page): website.page_updated(
                lang, page, locales_json, pages_json, docs_by_lang, self.releases[0]
            )
            for lang in website.LANGUAGE_CODES
            for page in website.PAGES
        }
        for entry in entries:
            loc = entry.find("s:loc", ns).text
            lastmod = entry.find("s:lastmod", ns).text
            if loc == website.ORIGIN + website.route("en", "changelog"):
                self.assertEqual(lastmod, self.releases[0]["published_at"][:10])
                self.assertEqual(len(entry.findall("x:link", ns)), 0)
            else:
                self.assertEqual(len(entry.findall("x:link", ns)), len(website.LANGUAGES) + 1)
                self.assertTrue(all(link.attrib["href"] in expected for link in entry.findall("x:link", ns)))
                # Page-scoped lastmod must reflect content metadata, not blindly mirror latest release date.
                self.assertNotEqual(lastmod, self.releases[0]["published_at"][:10], loc)
                self.assertEqual(lastmod, expected_lastmod[loc], loc)
        self.assertEqual(pages_json["en"]["install_updated"], "2026-11-27")
        self.assertEqual(locales_json["en"]["updated"], "2026-11-27")
        self.assertEqual(docs_by_lang["en"]["docs/workflows"]["updated"], "2026-11-27")
        self.assertEqual(docs_by_lang["en"]["guides/parallel-coding-agents"]["updated"], "2026-11-24")
        self.assertEqual(docs_by_lang["en"]["guides/scheduled-worktree-cleanup"]["updated"], "2026-11-27")

    def test_unpublished_release_exclusion_and_safe_markdown(self):
        release = {"tag_name": "20261118-1", "published_at": "2026-11-18T01:00:00Z", "html_url": website.REPOSITORY + "/releases/tag/20261118-1", "draft": False}
        self.assertEqual(website.published_releases([dict(release, draft=True), dict(release, published_at=None), release]), [dict((key, release.get(key)) for key in ("tag_name", "name", "html_url", "published_at", "body", "prerelease"))])
        body = website.release_markdown('## Changes\n- **Fix** `worktrees`\n- [Compare](https://github.com/jjyr/canban/compare/a...b)\n![untrusted](https://evil.example/tracker.png)\n<script>alert(1)</script>\n[bad](javascript:alert)\n```html\n<img src=x onerror=alert(1)>\n```')
        self.assertIn('<a href="https://github.com/jjyr/canban/compare/a...b">', body)
        self.assertIn("<strong>Fix</strong>", body)
        self.assertNotIn("<script>", body)
        self.assertNotIn("<img", body)
        self.assertNotIn('href="javascript:', body)
        with_images = website.release_markdown(
            '![Daily Briefing Inbox](https://raw.githubusercontent.com/jjyr/canban/main/docs/assets/daily-briefing.webp)\n'
            '![Mobile Companion](https://canban.dev/screenshots/mobile-inbox.webp)\n'
            '![Traversal](https://canban.dev/screenshots/../secret.png)'
        )
        self.assertIn(
            '<figure class="release-shot"><img src="https://raw.githubusercontent.com/jjyr/canban/main/docs/assets/daily-briefing.webp" alt="Daily Briefing Inbox" loading="lazy" /></figure>',
            with_images,
        )
        self.assertIn(
            '<figure class="release-shot"><img src="https://canban.dev/screenshots/mobile-inbox.webp" alt="Mobile Companion" loading="lazy" /></figure>',
            with_images,
        )
        self.assertNotIn('secret.png"', with_images)
        with self.assertRaises(ValueError):
            website.published_releases([dict(release, html_url="https://example.com/")])

    def test_doc_markdown_escapes_html_and_rejects_unsafe_links(self):
        rendered = website.doc_markdown(
            '<script>alert(1)</script>\n\n**`source_trust == "external_pr"` gate**\n\n[ok](/docs/)\n[bad](javascript:alert(1))\n\n```yaml\nschema: 12\n# <script>bad</script>\nenv:\n  REPORT: ${{ vars.GC_REPORT_PROJECT_ID }}\n```\n\n```sh\ncanban workflow sources --global # <script>bad</script>\n```\n\n```prompt\nGenerate pure script\n<script>bad</script>\n```',
            website.load("pages.json")["en"],
        )
        self.assertNotIn("<script>", rendered)
        self.assertIn("<strong><code>source_trust == &quot;external_pr&quot;</code> gate</strong>", rendered)
        self.assertIn('<a href="/docs/">ok</a>', rendered)
        self.assertNotIn('href="javascript:', rendered)
        self.assertIn('class="code-block"', rendered)
        self.assertIn('class="code-block code-block-prompt"', rendered)
        self.assertIn('<span class="tok-key">schema</span><span class="tok-op">:</span> <span class="tok-num">12</span>', rendered)
        self.assertIn('<span class="tok-expr">${{ vars.GC_REPORT_PROJECT_ID }}</span>', rendered)
        self.assertIn("Generate pure script", rendered)

    def test_installer_is_canonical_and_history_is_rendered(self):
        self.assertEqual((self.output / "install.sh").read_bytes(), (website.ROOT / "scripts/install.sh").read_bytes())
        for lang in website.LANGUAGE_CODES:
            page = Page((self.output / website.route(lang, "changelog").lstrip("/") / "index.html").read_text())
            ids = {attrs.get("id") for _, attrs in page.elements}
            self.assertTrue(all(release["tag_name"] in ids for release in self.releases))

    def test_versions_use_numeric_daily_sequence_even_if_publication_is_delayed(self):
        base = {"published_at": "2026-11-18T01:00:00Z", "html_url": website.REPOSITORY + "/releases/tag/test"}
        values = [dict(base, tag_name="20261118-2", published_at="2026-11-19T01:00:00Z"),
                  dict(base, tag_name="20261118-10"), dict(base, tag_name="20261117-99")]
        self.assertEqual([item["tag_name"] for item in website.published_releases(values)],
                         ["20261118-10", "20261118-2", "20261117-99"])

    def test_catalog_24_matches_rust_source(self):
        catalog_src = (website.ROOT / "crates/profiles/src/catalog.rs").read_text()
        found = re.findall(r'CatalogEntry\s*\{\s*id:\s*"([^"]+)"', catalog_src)
        self.assertEqual(tuple(found), BUILTIN_24_CLI_IDS)


if __name__ == "__main__":
    unittest.main()
