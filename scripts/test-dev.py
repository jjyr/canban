#!/usr/bin/env python3
"""Selection and refusal regressions; never starts or stops user processes."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('dev', Path(__file__).with_name('dev.py'))
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)


class Selection(unittest.TestCase):
    def setUp(self):
        self.environment = patch.dict(os.environ, {}, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.repo = patch.object(dev, 'REPO', Path('/tmp/cb-dev-repo'))
        self.root = patch.object(dev, 'ROOT', Path('/tmp/cb-dev-repo/.local/dev'))
        self.repo.start()
        self.root.start()
        self.addCleanup(self.repo.stop)
        self.addCleanup(self.root.stop)

    def test_branch_names_checkout_and_detached_head_have_distinct_instances(self):
        a = dev.select('feature/editor')
        self.assertEqual(a, dev.select('feature/editor'))
        b = dev.select('feature-editor')
        self.assertNotEqual(a['home'], b['home'])
        self.assertNotEqual(a['app_id'], b['app_id'])
        with patch.object(dev, 'REPO', Path('/tmp/other-checkout')), patch.object(dev, 'ROOT', Path('/tmp/other-checkout/.local/dev')):
            self.assertNotEqual(a['app_id'], dev.select('feature/editor')['app_id'])
        with patch.object(dev, 'git', side_effect=['', '0123456789ab']):
            detached = dev.select()
        self.assertEqual(detached['branch'], 'detached-0123456789ab')
        self.assertNotIn('/', Path(a['directory']).name)

    def test_explicit_home_wins_over_branch_and_formal_home_is_rejected(self):
        with tempfile.TemporaryDirectory(prefix='cbd-home-') as temporary:
            with patch.dict(os.environ, {'CANBAN_HOME': temporary}):
                a, b = dev.select('main'), dev.select('other')
            self.assertEqual(a['key'], b['key'])
            self.assertEqual(a['home'], str(Path(temporary).resolve()))
        with self.assertRaisesRegex(RuntimeError, 'installed'):
            dev.select('main', str(Path.home() / '.canban'))
        with self.assertRaisesRegex(RuntimeError, 'too long'):
            dev.select('main', '/tmp/' + 'x' * 120)

    def test_update_configuration_survives_but_agent_endpoint_bindings_do_not(self):
        with patch.dict(os.environ, {'CANBAN_ENDPOINT': 'foreign.sock', 'CANBAN_REQUEST_ID': 'agent',
                'CANBAN_UPDATE_REPOSITORY': 'fixture/releases', 'CANBAN_UPDATES_DISABLED': '0'}):
            env = dev.environment(dev.select('main'))
        self.assertNotIn('CANBAN_ENDPOINT', env)
        self.assertNotIn('CANBAN_REQUEST_ID', env)
        self.assertEqual(env['CANBAN_UPDATE_REPOSITORY'], 'fixture/releases')
        self.assertEqual(env['CANBAN_UPDATES_DISABLED'], '1')

    def test_bundle_bin_and_socket_limits_adapt_across_macos_and_linux(self):
        with patch.object(dev.sys, 'platform', 'darwin'):
            self.assertEqual(dev.bundle_bin('/tmp/inst'), Path('/tmp/inst/CanbanDev.app/Contents/MacOS'))
            # /tmp/<90 chars>/app.sock => 5 + 90 + 9 = 104 bytes -> rejected on macOS (limit 104)
            with patch.object(Path, 'resolve', lambda p: p):
                with self.assertRaisesRegex(RuntimeError, 'too long'):
                    dev.select('main', '/' + 'a' * 94)
        with patch.object(dev.sys, 'platform', 'linux'):
            self.assertEqual(dev.bundle_bin('/tmp/inst'), Path('/tmp/inst/CanbanDev/bin'))
            with patch.object(Path, 'resolve', lambda p: p):
                # 1 + 94 + 9 = 104 bytes < 108 -> allowed on Linux!
                ok = dev.select('main', '/' + 'a' * 94)
                self.assertEqual(ok['home'], '/' + 'a' * 94)
                # 1 + 98 + 9 = 108 bytes -> rejected on Linux (limit 108)
                with self.assertRaisesRegex(RuntimeError, 'too long'):
                    dev.select('main', '/' + 'a' * 98)

    def test_linux_identity_uses_proc_exe_instead_of_truncated_ps_comm(self):
        def fake_ps(argv, **kwargs):
            field = argv[-1]
            values = {
                'stat=': 'Ssl\n',
                'lstart=': 'Mon Nov 16 08:00:00 2026\n',
                'command=': '/tmp/inst/CanbanDev/bin/canban-ui\n',
                'comm=': 'canban-ui\n',  # Linux procps truncates to 15-char basename
            }
            return type('Res', (), {'returncode': 0, 'stdout': values[field]})()

        with patch.object(dev.sys, 'platform', 'linux'), \
             patch.object(dev.subprocess, 'run', side_effect=fake_ps), \
             patch.object(dev.os, 'readlink', return_value='/tmp/inst/CanbanDev/bin/canban-ui (deleted)'):
            ident = dev.identity(4242)
            self.assertEqual(ident, {
                'start': 'Mon Nov 16 08:00:00 2026',
                'command': '/tmp/inst/CanbanDev/bin/canban-ui',
                'executable': '/tmp/inst/CanbanDev/bin/canban-ui',
            })
            record = {'pid': 4242, 'identity': ident}
            self.assertTrue(dev.live(record, Path('/tmp/inst/CanbanDev/bin/canban-ui')))
            self.assertFalse(dev.live(record, Path('/tmp/other/CanbanDev/bin/canban-ui')))


if __name__ == '__main__':
    unittest.main()
