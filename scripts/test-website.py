#!/usr/bin/env python3
"""Check the generated website's crawlable content and release trust boundary."""
# Copyright 2026 JJy <jjyruby@gmail.com>
# SPDX-License-Identifier: Apache-2.0

import json
import tempfile
import unittest
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

import website


class Page(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.elements = []
        self.visible_text = []
        self.scripts = []
        self.in_script = False
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        self.elements.append((tag, dict(attrs)))
        if tag == "script":
            self.in_script = True
            self.scripts.append((dict(attrs), ""))

    def handle_endtag(self, tag):
        if tag == "script":
            self.in_script = False

    def handle_data(self, data):
        if self.in_script:
            attrs, text = self.scripts[-1]
            self.scripts[-1] = (attrs, text + data)
        else:
            self.visible_text.append(data)

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
        for lang in ("en", "zh"):
            for section in ("", "install", "changelog"):
                with self.subTest(lang=lang, section=section):
                    route = website.route(lang, section)
                    text = (self.output / route.lstrip("/") / "index.html").read_text()
                    page = Page(text)
                    self.assertEqual(len(page.attrs("h1")), 1)
                    self.assertEqual(page.attrs("html")[0]["lang"], "zh-CN" if lang == "zh" else "en")
                    self.assertEqual(page.attrs("link", rel="canonical"), [{"rel": "canonical", "href": website.ORIGIN + route}])
                    for code, target in (("en", "en"), ("zh-CN", "zh"), ("x-default", "en")):
                        self.assertEqual(page.attrs("link", rel="alternate", hreflang=code)[0]["href"], website.ORIGIN + website.route(target, section))
                    self.assertEqual(page.attrs("meta", property="og:url")[0]["content"], website.ORIGIN + route)
                    self.assertEqual(page.attrs("meta", property="og:locale")[0]["content"], "zh_CN" if lang == "zh" else "en_US")
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
                                    (q, a) for q, a in website.load("pages.json")[lang]["faq_items"]
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
                    if section == "install":
                        visible = " ".join(page.visible_text)
                        for question, answer in website.load("pages.json")[lang]["faq_items"]:
                            self.assertIn(question, visible)
                            self.assertIn(answer, visible)
                        self.assertIn("Herdr", visible)
                        self.assertIn(website.HEADLESS, visible)
                    if not section:
                        self.assertIn(website.load("locales.json")[lang]["hero_subtitle"], " ".join(page.visible_text))
                        self.assertEqual(len(page.attrs("title")), 1)
                        self.assertIn("Kanban + Workflow + Herdr", text)

    def test_links_assets_and_fragments_resolve(self):
        for source in self.output.rglob("*.html"):
            page = Page(source.read_text())
            base = "/" + str(source.parent.relative_to(self.output))
            for tag, attrs in page.elements:
                url = attrs.get("href") or attrs.get("src")
                if not url or urlsplit(url).scheme or url.startswith("//"):
                    continue
                parts = urlsplit(url)
                path = unquote(parts.path)
                destination = self.output / path.lstrip("/") if path.startswith("/") else self.output / base.lstrip("/") / path
                if destination.is_dir():
                    destination /= "index.html"
                self.assertTrue(destination.is_file(), (source, url))
                if parts.fragment and destination.suffix == ".html":
                    ids = [attrs.get("id") for _, attrs in Page(destination.read_text()).elements]
                    self.assertIn(parts.fragment, ids, (source, url))

    def test_sitemap_has_all_canonical_pages_and_reciprocal_alternates(self):
        ns = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9", "x": "http://www.w3.org/1999/xhtml"}
        tree = ET.parse(self.output / "sitemap.xml")
        entries = tree.findall("s:url", ns)
        expected = {website.ORIGIN + website.route(lang, page) for lang in ("en", "zh") for page in ("", "install", "changelog")}
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


if __name__ == "__main__":
    unittest.main()
