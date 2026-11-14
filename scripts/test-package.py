#!/usr/bin/env python3
"""macOS packaging regressions using real Mach-O executables and codesign."""
from pathlib import Path
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


class Packaging(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.compiler_directory = tempfile.TemporaryDirectory(prefix="cb-package-binary-")
        cls.addClassCleanup(cls.compiler_directory.cleanup)
        cls.executable = Path(cls.compiler_directory.name) / "fixture"
        subprocess.run(
            ["/usr/bin/xcrun", "clang", "-x", "c", "-o", str(cls.executable), "-"],
            input="int main(void) { return 0; }\n", text=True, check=True,
        )

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
        # The regression concerns bundle assembly, independent of Rust optimization.
        for profile in ("debug", "release"):
            target = self.root / "target" / profile
            (target / "examples").mkdir(parents=True)
            for binary in ("canban-ui", "canban", "canban-updater", "examples/gallery"):
                shutil.copy2(self.executable, target / binary)

    def package(self, *args):
        if '--dev' in args:
            directory = self.root / '.local/dev/test'
            descriptor = self.root / 'dev-instance.json'
            import json
            descriptor.write_text(json.dumps(dict(checkout=str(self.root), branch='fixture', key='test',
                directory=str(directory), home=str(directory/'home'), app_id='com.canban.dev.test')))
            args = (*args, '--dev-instance', str(descriptor))
        env = {k: v for k, v in os.environ.items()
               if k not in ("CANBAN_RELEASE_VERSION", "GITHUB_RUN_NUMBER")}
        subprocess.run([sys.executable, str(self.root / "scripts/package.py"), *args],
                       env=env, check=True, capture_output=True, text=True)

    def verify(self, name="Canban"):
        bundle = self.root / ('.local/dev/test/CanbanDev.app' if name == 'CanbanDev' else '.local/' + name + '.app')
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(bundle)],
                       check=True, capture_output=True, text=True)
        return bundle

    def test_clean_local_bundles_have_valid_resource_signatures(self):
        for args, name in (((), "Canban"), (("--dev",), "CanbanDev"),
                           (("--gallery",), "CanbanGallery")):
            with self.subTest(name=name):
                self.package(*args)
                bundle = self.verify(name)
                self.assertTrue((bundle / "Contents/_CodeSignature/CodeResources").is_file())

    def test_release_then_debug_replaces_the_old_resource_signature(self):
        self.package("--release")
        bundle = self.verify()
        # Simulate existing release-only resources, then overwrite the executable
        # with the original linker-signed Debug build as a local rebuild does.
        retained = bundle / "Contents/Resources/release-fixture.txt"
        retained.write_text("resource from a previous release\n")
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-",
                        "--timestamp=none", str(bundle)], check=True, capture_output=True)
        self.package()
        self.verify()
        self.assertEqual(retained.read_text(), "resource from a previous release\n")

    def test_repackaging_repairs_a_stale_resource_seal_after_relocation(self):
        self.package()
        bundle = self.root / ".local/Canban.app"
        icon = bundle / "Contents/Resources/AppIcon.icns"
        icon.write_bytes(icon.read_bytes() + b"changed after signing")
        invalid = subprocess.run(["/usr/bin/codesign", "--verify", "--strict", str(bundle)],
                                 capture_output=True)
        self.assertNotEqual(invalid.returncode, 0)
        self.package()
        self.verify()
        relocated = self.root / "Applications/Canban.app"
        relocated.parent.mkdir()
        shutil.copytree(bundle, relocated)
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(relocated)],
                       check=True, capture_output=True)
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


if __name__ == "__main__":
    unittest.main()
