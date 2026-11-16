import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import uuid

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('managed', ROOT / 'container/managed.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

class ManagedTests(unittest.TestCase):
    def test_default_and_partial_configuration(self):
        self.assertIsNone(m.scope({}))
        with self.assertRaises(RuntimeError):
            m.scope({m.KEYS[0]: str(uuid.uuid4())})
        with self.assertRaises(RuntimeError):
            m.scope(dict.fromkeys(m.KEYS, str(uuid.UUID(int=0))))

    def test_expiry_wrong_generation_missing_and_oversized_control(self):
        expected = m.scope({key: str(uuid.uuid4()) for key in m.KEYS})
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'lease.json'
            self.assertFalse(m.alive(expected, path, 1000))
            lease = dict(expected, version=1, run=None, expires_ms=2000)
            path.write_text(json.dumps(lease))
            self.assertTrue(m.alive(expected, path, 1000))
            self.assertFalse(m.alive(expected, path, 2000))
            self.assertFalse(m.alive(expected, path, 999))
            path.write_text(json.dumps(dict(lease, generation=str(uuid.uuid4()))))
            self.assertFalse(m.alive(expected, path, 1000))
            path.write_text(' ' * 2049)
            self.assertFalse(m.alive(expected, path, 1000))

    def test_symlink_is_not_a_control_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'lease.json'
            path.symlink_to('/dev/zero')
            self.assertFalse(m.alive({}, path, 1000))

class ManagedBridgeTests(unittest.TestCase):
    def test_resume_uses_current_epoch_and_exact_session(self):
        import contextlib
        import http.client
        import io
        import re
        from unittest.mock import patch
        source = (ROOT/'src/companion_docker.rs').read_text()
        bridge = re.search(r'const GUEST_BRIDGE: &str = r#"(.*?)"#;', source, re.S).group(1)
        session = str(uuid.uuid4())
        calls = []
        class Response:
            status = 200
            def __init__(self, body): self.body = body
            def read(self, limit): return json.dumps(self.body).encode()
            def getheader(self, *args): return 'application/json'
        class Connection:
            def __init__(self, *args, **kwargs): pass
            def request(self, method, path, body, headers): calls.append((method,path,json.loads(body) if body else None))
            def getresponse(self): return Response({'version':1,'session':session,'epoch':37} if len(calls)==1 else {})
            def close(self): pass
        with patch.object(http.client, 'HTTPConnection', Connection), patch('sys.argv', ['bridge','resume',session]), contextlib.redirect_stdout(io.StringIO()):
            exec(compile(bridge, 'managed-bridge', 'exec'), {})
        self.assertEqual(calls, [('GET','/companion/managed',None), ('POST','/companion/automation',{'action':'resume','expected_session':session,'expected_epoch':37})])
