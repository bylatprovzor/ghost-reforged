import ctypes
import importlib.util
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

source = Path(__file__).resolve().parents[1] / 'maintain-runtime.py'
spec = importlib.util.spec_from_file_location('maintenance', source)
m = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = m
spec.loader.exec_module(m)


class Maintenance(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.root = Path(self.folder.name)
        self.now = time.time()

    def tearDown(self):
        self.folder.cleanup()

    def file(self, name, days, size=10):
        p = self.root / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(b'x'*size)
        os.utime(p, (self.now-days*m.DAY, self.now-days*m.DAY))
        return p

    def policy(self, **kwargs):
        return m.Policy('fixture', self.root / 'Logs', r'.*\.log', **kwargs)

    def test_age_budget_newest_and_fresh_files(self):
        self.file('Logs/old.log', 20)
        self.file('Logs/recent.log', 4)
        self.file('Logs/new.log', 3)
        fresh = self.file('Logs/active.log', 0, 100)
        r = m.clean(self.policy(limit_bytes=30), self.now, True)
        self.assertEqual(r['removed_files'], 2)
        self.assertTrue(fresh.exists())
        self.assertTrue((self.root / 'Logs/new.log').exists())
        self.assertGreater(r['remaining_bytes'], 30)  # The limit never overrides protection.

    def test_preview_does_not_delete_and_apply_is_idempotent(self):
        p = self.file('Logs/old.log', 20)
        policy = self.policy(keep_newest=0)
        self.assertEqual(m.clean(policy, self.now)['eligible_bytes'], 10)
        self.assertTrue(p.exists())
        self.assertEqual(m.clean(policy, self.now, True)['removed_files'], 1)
        self.assertEqual(m.clean(policy, self.now, True)['removed_files'], 0)

    def test_maps_credentials_databases_and_unrelated_dumps_survive(self):
        paths = [self.file(name, 30) for name in [
            'Logs/replay.w3g', 'Logs/map.w3x', 'Logs/outbox.sqlite', 'Logs/credentials.json',
            'Maps/map.w3x', 'Replays/game.w3g', 'AppData/Local/CrashDumps/other.exe.123.dmp']]
        for policy in m.policies(self.root, self.root / 'ProgramData'):
            m.clean(policy, self.now, True)
        m.clean(self.policy(keep_newest=0), self.now, True)
        self.assertTrue(all(p.exists() for p in paths))

    def test_locked_and_replaced_files_survive(self):
        p = self.file('Logs/old.log', 30)
        info = p.stat()
        p.write_bytes(b'new live contents')
        self.assertFalse(m.delete_exclusive(p, info))
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32,
                                     ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
        kernel.CreateFileW.restype = ctypes.c_void_p
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.CreateFileW(str(p), 0x80000000, 1, None, 3, 0, None)
        self.assertNotEqual(handle, ctypes.c_void_p(-1).value)
        try:
            self.assertFalse(m.delete_exclusive(p, p.stat()))
        finally:
            kernel.CloseHandle(handle)
        self.assertTrue(p.exists())

    def test_linked_files_and_junctions_are_not_followed(self):
        outside = self.file('outside/important.log', 30)
        logs = self.root / 'Logs'
        logs.mkdir()
        os.link(outside, logs / 'alias.log')
        self.assertEqual(m.clean(self.policy(keep_newest=0), self.now, True)['files'], 0)
        import subprocess
        junction = logs / 'redirect'
        subprocess.run(['cmd.exe', '/c', 'mklink', '/J', str(junction), str(outside.parent)],
                       check=True, capture_output=True)
        try:
            self.assertFalse(m.plain_path(junction))
            self.assertEqual(m.clean(self.policy(keep_newest=0), self.now, True)['files'], 0)
        finally:
            os.rmdir(junction)
        self.assertTrue(outside.exists())


if __name__ == '__main__':
    unittest.main()
