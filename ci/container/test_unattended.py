import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
import uuid

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('unattended', ROOT / 'container/unattended.py')
u = importlib.util.module_from_spec(spec)
spec.loader.exec_module(u)


class UnattendedTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source.json'
        self.config = {'mode': 'container', 'session_id': str(uuid.uuid4()), 'credential_file': '/run/secrets/bot-token'}
        self.source.write_text(json.dumps(self.config))
        self.source.chmod(0o600)
        self.env = {'HYPERMESH_CONTAINER_UNATTENDED': '1', 'HYPERMESH_BOT_CONFIG': str(self.source)}

    def test_default_never_reads_configuration_or_grants_consent(self):
        self.assertIsNone(u.prepare({'HYPERMESH_BOT_CONFIG': '/nonexistent'}, self.root))
        for flag in ('true', '', 'yes'):
            with self.assertRaises(ValueError): u.prepare({'HYPERMESH_CONTAINER_UNATTENDED': flag}, self.root)

    def test_private_copy_selects_no_session_and_preserves_read_only_source(self):
        target = u.prepare(self.env, self.root)
        self.assertEqual(json.loads(target.read_text())['session_id'], str(uuid.UUID(int=0)))
        self.assertEqual(json.loads(self.source.read_text()), self.config)
        self.assertEqual(target.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.env['HYPERMESH_BOT_CONFIG'], str(target))

    def test_refuses_interactive_mode_shared_permissions_and_symlinks(self):
        self.source.chmod(0o644)
        with self.assertRaises(ValueError): u.prepare(self.env, self.root)
        self.source.chmod(0o600)
        link = self.root / 'link'
        link.symlink_to(self.source)
        with self.assertRaises(OSError): u.private_config(link)
        self.source.write_text(json.dumps(dict(self.config, mode='interactive')))
        with self.assertRaises(ValueError): u.prepare(self.env, self.root)

    def test_startup_consent_is_local_once_and_binds_new_session(self):
        target = u.prepare(self.env, self.root)
        session = str(uuid.uuid4())
        calls = []
        def request(companion, method, path, body=None):
            calls.append((companion, method, path, body))
            return {'id': session} if path == '/session' else {}
        self.assertEqual(u.activate(target, request), session)
        self.assertEqual(json.loads(target.read_text())['session_id'], session)
        self.assertEqual([(c[0], c[1], c[2]) for c in calls], [
            (False, 'POST', '/session'), (True, 'GET', '/companion'),
            (True, 'POST', '/companion/automation')])
        self.assertEqual(calls[-1][3], {'action': 'start'})
        self.assertNotIn('credential', json.dumps(calls))
        self.assertNotIn('resume', json.dumps(calls))

    def test_failed_local_start_is_not_retried(self):
        target = u.prepare(self.env, self.root)
        calls = []
        def request(companion, method, path, body=None):
            calls.append(path)
            if path == '/companion/automation': raise RuntimeError('local policy refused')
            return {'id': str(uuid.uuid4())}
        with self.assertRaises(RuntimeError): u.activate(target, request)
        self.assertEqual(calls.count('/companion/automation'), 1)
