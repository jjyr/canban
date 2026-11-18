#!/usr/bin/env python3
"""Check the generated website's crawlable content, GEO mirrors, and release trust boundary."""
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
        self.ch1_links = []
        self._in_ch0 = False
        self._ch0_depth = 0
        self._in_ch1_nav = False
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        attr_map = dict(attrs)
        self.elements.append((tag, attr_map))
        if tag in ("code", "pre"):
            self._code_depth += 1
        if tag == "section" and attr_map.get("id") == "ch-0":
            self._in_ch0 = True
            self._ch0_depth = 1
        elif self._in_ch0:
            self._ch0_depth += 1
        if self._in_ch0 and tag == "nav" and "ch1-links" in attr_map.get("class", "").split():
            self._in_ch1_nav = True
        if self._in_ch1_nav and tag == "a" and attr_map.get("href"):
            self.ch1_links.append(attr_map["href"])
        if tag == "script":
            self.in_script = True
            self.scripts.append((attr_map, ""))

    def handle_endtag(self, tag):
        if tag in ("code", "pre") and self._code_depth > 0:
            self._code_depth -= 1
        if tag == "script":
            self.in_script = False
        if self._in_ch1_nav and tag == "nav":
            self._in_ch1_nav = False
        if self._in_ch0:
            self._ch0_depth -= 1
            if self._ch0_depth <= 0:
                self._in_ch0 = False

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
        for lang in ("en", "zh"):
            for section in website.PAGES:
                with self.subTest(lang=lang, section=section):
                    route = website.route(lang, section)
                    text = (self.output / route.lstrip("/") / "index.html").read_text()
                    page = Page(text)
                    self.assertEqual(len(page.attrs("h1")), 1)
                    self.assertEqual(page.attrs("html")[0]["lang"], "zh-CN" if lang == "zh" else "en")
                    self.assertEqual(page.attrs("link", rel="canonical"), [{"rel": "canonical", "href": website.ORIGIN + route}])
                    for code, target in (("en", "en"), ("zh-CN", "zh"), ("x-default", "en")):
                        self.assertEqual(page.attrs("link", rel="alternate", hreflang=code)[0]["href"], website.ORIGIN + website.route(target, section))
                    md_alts = page.attrs("link", rel="alternate", type="text/markdown")
                    self.assertEqual(len(md_alts), 1)
                    self.assertEqual(md_alts[0]["href"], website.ORIGIN + website.md_route(lang, section))
                    self.assertEqual(page.attrs("meta", property="og:url")[0]["content"], website.ORIGIN + route)
                    self.assertEqual(page.attrs("meta", property="og:locale")[0]["content"], "zh_CN" if lang == "zh" else "en_US")
                    self.assertIsNone(re.search(r"\{\{(?:en|zh|install|docs|workflows|compare|changelog)_url\}\}", text))
                    if section != "workflows":
                        self.assertNotIn("{{", text)
                    self.assertNotIn("localStorage", text)

                    for attrs, script in page.scripts:
                        if attrs.get("type") == "application/ld+json":
                            graph = json.loads(script)["@graph"]
                            by_type = {item["@type"]: item for item in graph}
                            software = by_type["SoftwareApplication"]
                            self.assertEqual(software["softwareVersion"], self.releases[0]["tag_name"])
                            self.assertEqual(software["dateModified"], self.releases[0]["published_at"])
                            self.assertEqual(software["releaseNotes"], website.ORIGIN + website.route(lang, "changelog"))
                            screenshots = software["screenshot"]
                            self.assertIsInstance(screenshots, list)
                            self.assertEqual(len(screenshots), 3)
                            for shot_url in screenshots:
                                self.assertTrue(shot_url.startswith(website.ORIGIN + "/screenshots/"), shot_url)
                                shot_path = self.output / urlsplit(shot_url).path.lstrip("/")
                                self.assertTrue(shot_path.is_file(), shot_path)

                            if section in website.DOC_PAGES:
                                article = by_type["TechArticle"]
                                self.assertEqual(article["url"], website.ORIGIN + route)
                                self.assertEqual(article["inLanguage"], "zh-CN" if lang == "zh" else "en")
                                self.assertEqual(article["dateModified"], self.releases[0]["published_at"])
                                self.assertIn("BreadcrumbList", by_type)
                                expected_crumbs = 3 if section.startswith("docs/") else 2
                                self.assertEqual(len(by_type["BreadcrumbList"]["itemListElement"]), expected_crumbs)
                                self.assertNotIn("FAQPage", by_type)
                            else:
                                webpage = by_type["CollectionPage" if section == "changelog" else "WebPage"]
                                self.assertEqual(webpage["url"], website.ORIGIN + route)
                                if not section:
                                    self.assertIn("featureList", software)
                                    self.assertNotIn("FAQPage", by_type)
                                    self.assertNotIn("BreadcrumbList", by_type)
                                else:
                                    self.assertIn("BreadcrumbList", by_type)
                                if section == "install":
                                    faq = by_type["FAQPage"]
                                    expected_faqs = [
                                        (q, a) for q, a in pages_json[lang]["faq_items"]
                                    ]
                                    actual_faqs = [
                                        (item["name"], item["acceptedAnswer"]["text"])
                                        for item in faq["mainEntity"]
                                    ]
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
                    if not section:
                        self.assertIn(locales_json[lang]["hero_subtitle"], visible)
                        self.assertEqual(len(page.attrs("title")), 1)
                        self.assertIn("Kanban + Workflow + Herdr", text)
                        expected_ch1 = {
                            website.route(lang, hub)
                            for hub in ("install", "docs", "workflows", "compare", "changelog")
                        }
                        self.assertEqual(set(page.ch1_links), expected_ch1)
                        self.assertFalse([img for img in page.attrs("img") if img.get("src", "").startswith("/screenshots/")])

                    expected_screenshots = {
                        "install": ("/screenshots/board-terminal.png", False),
                        "docs": ("/screenshots/review-diff.png", True),
                        "docs/workflows": ("/screenshots/workflow-studio.png", True),
                        "docs/agents": ("/screenshots/board-terminal.png", True),
                        "workflows": ("/screenshots/workflow-studio.png", True),
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

    def test_markdown_mirrors_and_geo_files(self):
        for lang in ("en", "zh"):
            for section in website.PAGES:
                with self.subTest(lang=lang, section=section):
                    md_path = self.output / website.md_route(lang, section).lstrip("/")
                    self.assertTrue(md_path.is_file(), md_path)
                    content = md_path.read_text()
                    self.assertTrue(content.startswith("# "), md_path)
                    self.assertNotIn("<script", content)

        agents_md = (self.output / "docs/agents.md").read_text()
        for cli_id in BUILTIN_24_CLI_IDS:
            self.assertIn(f"`{cli_id}`", agents_md)
        self.assertIn("canban card adopt", agents_md)
        self.assertIn("libghostty-vt", agents_md)

        workflows_doc_md = (self.output / "docs/workflows.md").read_text()
        for term in ("development.yaml", "admit", "script", "deliver", "external_pr", "schema: 10"):
            self.assertIn(term, workflows_doc_md)

        remote_md = (self.output / "docs/remote-hosts.md").read_text()
        for term in ("canband", "Tailscale", "SSH", "--headless"):
            self.assertIn(term, remote_md)

        workflows_md = (self.output / "workflows.md").read_text()
        self.assertGreaterEqual(workflows_md.count("schema: 10"), 4)

        compare_md = (self.output / "compare.md").read_text()
        for competitor in ("Vibe Kanban", "Linear", "Cloud Coding Agents", "Terminal Tabs"):
            self.assertIn(competitor, compare_md)

        llms_txt = (self.output / "llms.txt").read_text()
        self.assertIn("https://canban.dev/llms-full.txt", llms_txt)
        self.assertIn("Canban = Kanban + Workflow + Herdr", llms_txt)
        for slug in ("install", "docs", "docs/workflows", "docs/agents", "docs/remote-hosts", "workflows", "compare", "changelog"):
            self.assertIn(f"https://canban.dev/{slug}.md", llms_txt)

        llms_full = (self.output / "llms-full.txt").read_text()
        self.assertIn("Canban = Kanban + Workflow + Herdr", llms_full)
        self.assertIn("canban card adopt", llms_full)
        self.assertIn("batch report", llms_full)
        self.assertIn("review draft", llms_full)
        self.assertIn("worktree gc", llms_full)
        for cli_id in BUILTIN_24_CLI_IDS:
            self.assertIn(f"`{cli_id}`", llms_full)

    def test_links_assets_and_fragments_resolve(self):
        expected_screenshots = {
            "board-terminal.png",
            "workflow-studio.png",
            "review-diff.png",
        }
        source_shots = {path.name for path in (website.SOURCE / "screenshots").glob("*.png")}
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

    def test_sitemap_has_all_canonical_pages_and_reciprocal_alternates(self):
        ns = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9", "x": "http://www.w3.org/1999/xhtml"}
        tree = ET.parse(self.output / "sitemap.xml")
        entries = tree.findall("s:url", ns)
        expected = {website.ORIGIN + website.route(lang, page) for lang in ("en", "zh") for page in website.PAGES}
        self.assertEqual({entry.find("s:loc", ns).text for entry in entries}, expected)
        for entry in entries:
            self.assertEqual(entry.find("s:lastmod", ns).text, self.releases[0]["published_at"][:10])
            self.assertEqual(len(entry.findall("x:link", ns)), 3)
            self.assertTrue(all(link.attrib["href"] in expected for link in entry.findall("x:link", ns)))

    def test_unpublished_release_exclusion_and_safe_markdown(self):
        release = {"tag_name": "20261118-1", "published_at": "2026-11-18T01:00:00Z", "html_url": website.REPOSITORY + "/releases/tag/20261118-1", "draft": False}
        self.assertEqual(website.published_releases([dict(release, draft=True), dict(release, published_at=None), release]), [dict((key, release.get(key)) for key in ("tag_name", "name", "html_url", "published_at", "body", "prerelease"))])
        body = website.release_markdown('## Changes\n- **Fix** `worktrees`\n- [Compare](https://github.com/jjyr/canban/compare/a...b)\n<script>alert(1)</script>\n[bad](javascript:alert)\n```html\n<img src=x onerror=alert(1)>\n```')
        self.assertIn('<a href="https://github.com/jjyr/canban/compare/a...b">', body)
        self.assertIn("<strong>Fix</strong>", body)
        self.assertNotIn("<script>", body)
        self.assertNotIn("<img", body)
        self.assertNotIn('href="javascript:', body)
        with self.assertRaises(ValueError):
            website.published_releases([dict(release, html_url="https://example.com/")])

    def test_doc_markdown_escapes_html_and_rejects_unsafe_links(self):
        rendered = website.doc_markdown(
            '<script>alert(1)</script>\n\n**`source_trust == "external_pr"` gate**\n\n[ok](/docs/)\n[bad](javascript:alert(1))\n\n```yaml\nschema: 10\n```',
            website.load("pages.json")["en"],
        )
        self.assertNotIn("<script>", rendered)
        self.assertIn("<strong><code>source_trust == &quot;external_pr&quot;</code> gate</strong>", rendered)
        self.assertIn('<a href="/docs/">ok</a>', rendered)
        self.assertNotIn('href="javascript:', rendered)
        self.assertIn('class="code-block"', rendered)
        self.assertIn('data-copy-value="schema: 10"', rendered)

    def test_installer_is_canonical_and_history_is_rendered(self):
        self.assertEqual((self.output / "install.sh").read_bytes(), (website.ROOT / "scripts/install.sh").read_bytes())
        for lang in ("en", "zh"):
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
