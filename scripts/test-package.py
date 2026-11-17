#!/usr/bin/env python3
"""macOS and Linux desktop packaging and install-app regressions."""
from pathlib import Path
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


def build_fixture_executable(directory: Path) -> Path:
    executable = directory / "fixture"
    if sys.platform == "darwin":
        subprocess.run(
            ["/usr/bin/xcrun", "clang", "-x", "c", "-o", str(executable), "-"],
            input="int main(void) { return 0; }\n",
            text=True,
            check=True,
        )
        return executable
    for compiler in ("cc", "gcc", "clang"):
        found = shutil.which(compiler)
        if found:
            res = subprocess.run(
                [found, "-x", "c", "-o", str(executable), "-"],
                input="int main(void) { return 0; }\n",
                text=True,
                capture_output=True,
            )
            if res.returncode == 0:
                return executable
    executable.write_text("#!/bin/sh\nexit 0\n")
    executable.chmod(0o755)
    return executable


@unittest.skipUnless(sys.platform == "darwin", "macOS Mach-O / codesign tests require Darwin")
class MacOSPackaging(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.compiler_directory = tempfile.TemporaryDirectory(prefix="cb-package-binary-")
        cls.addClassCleanup(cls.compiler_directory.cleanup)
        cls.executable = build_fixture_executable(Path(cls.compiler_directory.name))

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="cb-package-regression-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        (self.root / "scripts").mkdir()
        shutil.copy2(REPO / "scripts/package.py", self.root / "scripts/package.py")
        shutil.copy2(REPO / "scripts/install-app.py", self.root / "scripts/install-app.py")
        (self.root / "crates/ui").mkdir(parents=True)
        (self.root / "crates/ui/assets").symlink_to(REPO / "crates/ui/assets")
        (self.root / "third_party").symlink_to(REPO / "third_party")
        (self.root / "THIRD_PARTY_NOTICES.md").symlink_to(REPO / "THIRD_PARTY_NOTICES.md")
        for profile in ("debug", "release", "profiling"):
            target = self.root / "target" / profile
            (target / "examples").mkdir(parents=True)
            for binary in ("canban-ui", "canban", "canban-updater", "examples/gallery"):
                shutil.copy2(self.executable, target / binary)

    def package(self, *args):
        if "--dev" in args:
            directory = self.root / ".local/dev/test"
            descriptor = self.root / "dev-instance.json"
            descriptor.write_text(
                json.dumps(
                    dict(
                        checkout=str(self.root),
                        branch="fixture",
                        key="test",
                        directory=str(directory),
                        home=str(directory / "home"),
                        app_id="com.canban.dev.test",
                    )
                )
            )
            args = (*args, "--dev-instance", str(descriptor))
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("CANBAN_RELEASE_VERSION", "GITHUB_RUN_NUMBER", "CANBAN_PACKAGE_PLATFORM")
        }
        subprocess.run(
            [sys.executable, str(self.root / "scripts/package.py"), *args],
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )

    def verify(self, name="Canban"):
        bundle = self.root / (
            ".local/dev/test/CanbanDev.app" if name == "CanbanDev" else ".local/" + name + ".app"
        )
        subprocess.run(
            ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(bundle)],
            check=True,
            capture_output=True,
            text=True,
        )
        return bundle

    def test_clean_local_bundles_have_valid_resource_signatures(self):
        for args, name in (((), "Canban"), (("--dev",), "CanbanDev"), (("--gallery",), "CanbanGallery"), (("--profile", "profiling"), "Canban")):
            with self.subTest(name=name):
                self.package(*args)
                bundle = self.verify(name)
                self.assertTrue((bundle / "Contents/_CodeSignature/CodeResources").is_file())

    def test_profiling_debug_entitlements_are_limited_to_profiling_packages(self):
        self.package("--profile", "profiling")
        bundle = self.verify()
        for name in ("canban-ui", "canban", "canban-updater"):
            result = subprocess.run(
                ["/usr/bin/codesign", "--display", "--entitlements", ":-", str(bundle / "Contents/MacOS" / name)],
                check=True, capture_output=True,
            )
            self.assertTrue(plistlib.loads(result.stdout)["com.apple.security.get-task-allow"])
        self.package()
        self.verify()
        for name in ("canban-ui", "canban", "canban-updater"):
            result = subprocess.run(
                ["/usr/bin/codesign", "--display", "--entitlements", ":-", str(bundle / "Contents/MacOS" / name)],
                check=True, capture_output=True,
            )
            self.assertNotIn(b"get-task-allow", result.stdout)

    def test_release_then_debug_replaces_the_old_resource_signature(self):
        self.package("--release")
        bundle = self.verify()
        retained = bundle / "Contents/Resources/release-fixture.txt"
        retained.write_text("resource from a previous release\n")
        subprocess.run(
            ["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(bundle)],
            check=True,
            capture_output=True,
        )
        self.package()
        self.verify()
        self.assertEqual(retained.read_text(), "resource from a previous release\n")

    def test_repackaging_repairs_a_stale_resource_seal_after_relocation(self):
        self.package()
        bundle = self.root / ".local/Canban.app"
        icon = bundle / "Contents/Resources/AppIcon.icns"
        icon.write_bytes(icon.read_bytes() + b"changed after signing")
        invalid = subprocess.run(["/usr/bin/codesign", "--verify", "--strict", str(bundle)], capture_output=True)
        self.assertNotEqual(invalid.returncode, 0)
        self.package()
        self.verify()
        relocated = self.root / "Applications/Canban.app"
        relocated.parent.mkdir()
        shutil.copytree(bundle, relocated)
        subprocess.run(
            ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(relocated)],
            check=True,
            capture_output=True,
        )
        info = plistlib.loads((relocated / "Contents/Info.plist").read_bytes())
        subprocess.run([str(relocated / "Contents/MacOS" / info["CFBundleExecutable"])], check=True)

    def test_install_app_atomically_installs_and_replaces_existing_bundle(self):
        self.package("--release")
        dest = self.root / "Applications/Canban.app"
        subprocess.run(
            [sys.executable, str(self.root / "scripts/install-app.py"), "--dest", str(dest)],
            check=True,
            capture_output=True,
            text=True,
        )
        subprocess.run(
            ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(dest)],
            check=True,
            capture_output=True,
        )
        marker = dest / "Contents/Resources/old-marker.txt"
        marker.write_text("old\n")
        stale = 1_700_000_000
        os.utime(self.root / ".local/Canban.app", (stale, stale))
        os.utime(dest, (stale, stale))
        self.package("--release")
        self.assertGreater((self.root / ".local/Canban.app").stat().st_mtime, stale)
        subprocess.run(
            [sys.executable, str(self.root / "scripts/install-app.py"), "--dest", str(dest)],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertFalse(marker.exists())
        self.assertGreater(dest.stat().st_mtime, stale)
        subprocess.run(
            ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(dest)],
            check=True,
            capture_output=True,
        )

    def test_install_app_refuses_bound_development_bundle(self):
        self.package("--dev")
        dev_bundle = self.root / ".local/dev/test/CanbanDev.app"
        dest = self.root / "Applications/Canban.app"
        res = subprocess.run(
            [
                sys.executable,
                str(self.root / "scripts/install-app.py"),
                "--source",
                str(dev_bundle),
                "--dest",
                str(dest),
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(res.returncode, 0)
        self.assertFalse(dest.exists())


class LinuxPackaging(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.compiler_directory = tempfile.TemporaryDirectory(prefix="cb-linux-package-bin-")
        cls.addClassCleanup(cls.compiler_directory.cleanup)
        cls.executable = build_fixture_executable(Path(cls.compiler_directory.name))

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="cb-linux-package-reg-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        (self.root / "scripts").mkdir()
        shutil.copy2(REPO / "scripts/package.py", self.root / "scripts/package.py")
        shutil.copy2(REPO / "scripts/install-app.py", self.root / "scripts/install-app.py")
        (self.root / "crates/ui").mkdir(parents=True)
        (self.root / "crates/ui/assets").symlink_to(REPO / "crates/ui/assets")
        (self.root / "third_party").symlink_to(REPO / "third_party")
        (self.root / "THIRD_PARTY_NOTICES.md").symlink_to(REPO / "THIRD_PARTY_NOTICES.md")
        for profile in ("debug", "release", "profiling"):
            target = self.root / "target" / profile
            (target / "examples").mkdir(parents=True)
            for binary in ("canban-ui", "canban", "canban-updater", "examples/gallery"):
                shutil.copy2(self.executable, target / binary)

    def linux_env(self, extra=None):
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("CANBAN_RELEASE_VERSION", "CANBAN_RELEASE_COMMIT", "GITHUB_RUN_NUMBER",
                         "CANBAN_INSTALL_DEST", "CANBAN_INSTALL_DIR", "CANBAN_BIN_DIR", "CANBAN_XDG_DATA_HOME")
        }
        env["CANBAN_PACKAGE_PLATFORM"] = "linux"
        if extra:
            env.update(extra)
        return env

    def package(self, *args, extra_env=None):
        if "--dev" in args:
            directory = self.root / ".local/dev/test"
            descriptor = self.root / "dev-instance.json"
            descriptor.write_text(
                json.dumps(
                    dict(
                        checkout=str(self.root),
                        branch="feature/linux",
                        key="test",
                        directory=str(directory),
                        home=str(directory / "home"),
                        app_id="com.canban.dev.test",
                    )
                )
            )
            args = (*args, "--dev-instance", str(descriptor))
        res = subprocess.run(
            [sys.executable, str(self.root / "scripts/package.py"), *args],
            env=self.linux_env(extra_env),
            check=True,
            capture_output=True,
            text=True,
        )
        return Path(res.stdout.strip())

    def test_linux_desktop_dev_and_gallery_bundles_match_contract(self):
        desktop = self.package("--release", extra_env={"CANBAN_RELEASE_VERSION": "20261116-1", "CANBAN_RELEASE_COMMIT": "a" * 40})
        self.assertEqual(desktop, self.root / ".local/canban")
        for b in ("canban", "canban-ui", "canban-updater"):
            p = desktop / "bin" / b
            self.assertTrue(p.is_file())
            self.assertEqual(p.stat().st_mode & 0o111, 0o111)
        meta = json.loads((desktop / "bundle.json").read_text())
        self.assertEqual(meta["app_id"], "com.canban.desktop")
        self.assertEqual(meta["executable"], "canban-ui")
        self.assertEqual(meta["version"], "20261116-1")
        self.assertEqual(meta["commit"], "a" * 40)
        self.assertFalse((desktop / "share/dev-instance.json").exists())
        desktop_file = (desktop / "share/applications/com.canban.desktop.desktop").read_text()
        self.assertIn("Exec=canban-ui\n", desktop_file)
        self.assertIn("Icon=com.canban.desktop\n", desktop_file)
        self.assertIn("StartupWMClass=com.canban.desktop\n", desktop_file)
        self.assertTrue((desktop / "share/icons/hicolor/scalable/apps/com.canban.desktop.svg").is_file())
        self.assertTrue((desktop / "share/icons/hicolor/256x256/apps/com.canban.desktop.png").is_file())
        self.assertTrue((desktop / "share/icons/hicolor/512x512/apps/com.canban.desktop.png").is_file())
        self.assertTrue((desktop / "share/licenses/THIRD_PARTY_NOTICES.md").is_file())

        dev_bundle = self.package("--dev")
        self.assertEqual(dev_bundle, self.root / ".local/dev/test/CanbanDev")
        dev_meta = json.loads((dev_bundle / "bundle.json").read_text())
        self.assertEqual(dev_meta["app_id"], "com.canban.dev.test")
        dev_inst = json.loads((dev_bundle / "share/dev-instance.json").read_text())
        self.assertEqual(dev_inst["branch"], "feature/linux")

        gallery = self.package("--gallery")
        self.assertEqual(gallery, self.root / ".local/canban-gallery")
        self.assertTrue((gallery / "bin/canban-gallery").is_file())
        subprocess.run([str(gallery / "bin/canban-gallery")], check=True)

    def test_linux_install_app_installs_opt_bundle_symlinks_and_xdg_entries_isolated(self):
        self.package("--release")
        prefix = self.root / "isolated-local"
        dest = prefix / "opt/canban"
        subprocess.run(
            [sys.executable, str(self.root / "scripts/install-app.py"), "--dest", str(dest)],
            env=self.linux_env(),
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertTrue((dest / "bundle.json").is_file())
        self.assertTrue((prefix / "bin/canban").is_symlink())
        self.assertTrue((prefix / "bin/canban-ui").is_symlink())
        self.assertEqual((prefix / "bin/canban").resolve(), (dest / "bin/canban").resolve())
        self.assertEqual((prefix / "bin/canban-ui").resolve(), (dest / "bin/canban-ui").resolve())

        xdg_desktop = prefix / "share/applications/com.canban.desktop.desktop"
        self.assertTrue(xdg_desktop.is_file())
        self.assertIn(f"Exec={dest / 'bin/canban-ui'}\n", xdg_desktop.read_text())
        self.assertTrue((prefix / "share/icons/hicolor/scalable/apps/com.canban.desktop.svg").is_file())
        self.assertTrue((prefix / "share/icons/hicolor/256x256/apps/com.canban.desktop.png").is_file())
        self.assertTrue((prefix / "share/icons/hicolor/512x512/apps/com.canban.desktop.png").is_file())

        # Atomic replacement cleans old marker and preserves working symlinks
        marker = dest / "share/stale-marker.txt"
        marker.write_text("stale\n")
        subprocess.run(
            [sys.executable, str(self.root / "scripts/install-app.py"), "--dest", str(dest)],
            env=self.linux_env(),
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertFalse(marker.exists())
        subprocess.run([str(prefix / "bin/canban")], check=True)

        # Refuses bound CanbanDev bundle
        dev_bundle = self.package("--dev")
        bad_dest = self.root / "bad-dest/opt/canban"
        bad = subprocess.run(
            [
                sys.executable,
                str(self.root / "scripts/install-app.py"),
                "--source",
                str(dev_bundle),
                "--dest",
                str(bad_dest),
            ],
            env=self.linux_env(),
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(bad.returncode, 0)
        self.assertFalse(bad_dest.exists())


if __name__ == "__main__":
    unittest.main()
