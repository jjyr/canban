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
        self.repo = patch.object(dev, 'REPO', Path('/private/tmp/cb-dev-repo'))
        self.root = patch.object(dev, 'ROOT', Path('/private/tmp/cb-dev-repo/.local/dev'))
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
        with patch.object(dev, 'REPO', Path('/private/tmp/other-checkout')), patch.object(dev, 'ROOT', Path('/private/tmp/other-checkout/.local/dev')):
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
            dev.select('main', str(Path.home()/'.canban'))
        with self.assertRaisesRegex(RuntimeError, 'too long'):
            dev.select('main', '/private/tmp/' + 'x'*120)

    def test_update_configuration_survives_but_agent_endpoint_bindings_do_not(self):
        with patch.dict(os.environ, {'CANBAN_ENDPOINT':'foreign.sock','CANBAN_REQUEST_ID':'agent',
                'CANBAN_UPDATE_REPOSITORY':'fixture/releases','CANBAN_UPDATES_DISABLED':'0'}):
            env = dev.environment(dev.select('main'))
        self.assertNotIn('CANBAN_ENDPOINT', env)
        self.assertNotIn('CANBAN_REQUEST_ID', env)
        self.assertEqual(env['CANBAN_UPDATE_REPOSITORY'], 'fixture/releases')
        self.assertEqual(env['CANBAN_UPDATES_DISABLED'], '1')


if __name__ == '__main__':
    unittest.main()
