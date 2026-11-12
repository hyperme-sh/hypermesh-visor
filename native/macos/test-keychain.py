"""Native credential-store contract, restricted to disposable hosted macOS CI."""
import contextlib
import os
from pathlib import Path
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest


def security(*args):
    return subprocess.check_output(['/usr/bin/security', *args], text=True, stderr=subprocess.PIPE, timeout=15).strip()


@contextlib.contextmanager
def isolated_keychain(path, command=security):
    original_default = shlex.split(command('default-keychain', '-d', 'user'))
    original_search = shlex.split(command('list-keychains', '-d', 'user'))
    if len(original_default) != 1 or not original_search:
        raise RuntimeError('Cannot save original keychain configuration')
    password = secrets.token_urlsafe(32)
    command('create-keychain', '-p', password, str(path))
    try:
        command('unlock-keychain', '-p', password, str(path))
        command('set-keychain-settings', '-lut', '600', str(path))
        command('list-keychains', '-d', 'user', '-s', str(path))
        command('default-keychain', '-d', 'user', '-s', str(path))
        if shlex.split(command('default-keychain', '-d', 'user')) != [str(path)]:
            raise RuntimeError('Temporary keychain is not the default')
        if shlex.split(command('list-keychains', '-d', 'user')) != [str(path)]:
            raise RuntimeError('Keychain search is not isolated')
        yield
    finally:
        # Attempt every restoration even when an earlier operation fails.
        errors = []
        for args in [('default-keychain', '-d', 'user', '-s', *original_default),
                     ('list-keychains', '-d', 'user', '-s', *original_search),
                     ('delete-keychain', str(path))]:
            try:
                command(*args)
            except Exception as error:
                errors.append(error)
        if errors:
            raise RuntimeError('Temporary keychain cleanup failed') from errors[0]


def main():
    if not (sys.platform == 'darwin' and os.environ.get('GITHUB_ACTIONS') == 'true'
            and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'):
        raise SystemExit('This contract runs only on a disposable GitHub-hosted macOS runner')
    repo = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix='visor-keychain-') as directory:
        root = Path(directory).resolve()
        source = root/'cli'
        shutil.copytree(repo/'third_party/hypermesh-cli', source)
        contract = source/'cmd/visor-keychain-contract'
        contract.mkdir()
        shutil.copyfile(Path(__file__).with_name('keychain-contract.go'), contract/'main.go')
        binary = root/'keychain-contract'
        subprocess.run(['go', 'build', '-mod=readonly', '-buildvcs=false', '-o', str(binary), './cmd/visor-keychain-contract'],
                       cwd=source, check=True, env=dict(os.environ, CGO_ENABLED='0', GOWORK='off'))
        with isolated_keychain(root/'contract.keychain-db'):
            subprocess.run([str(binary)], check=True, timeout=30,
                           env=dict(os.environ, VISOR_ISOLATED_KEYCHAIN_CONTRACT='1'))


class CleanupContracts(unittest.TestCase):
    def exercise(self, fail_operation=None, body_fails=False):
        calls = []
        default, search = ['/original keychain'], ['/original keychain', '/other']
        def command(*args):
            nonlocal default, search
            calls.append(args)
            if args[0] == fail_operation:
                raise RuntimeError('fixture failure')
            if args[0] in ('default-keychain', 'list-keychains'):
                if '-s' in args:
                    if args[0] == 'default-keychain': default = list(args[4:])
                    else: search = list(args[4:])
                    return ''
                return ' '.join(shlex.quote(p) for p in (default if args[0] == 'default-keychain' else search))
            return ''
        try:
            with isolated_keychain(Path('/temporary keychain'), command):
                self.assertEqual(default, ['/temporary keychain'])
                self.assertEqual(search, ['/temporary keychain'])
                if body_fails: raise ValueError('body failed')
        except (RuntimeError, ValueError):
            if not (fail_operation or body_fails): raise
        self.assertEqual(default, ['/original keychain'])
        self.assertEqual(search, ['/original keychain', '/other'])
        self.assertEqual(calls[-1], ('delete-keychain', '/temporary keychain'))

    def test_restores_after_success_and_contract_failure(self):
        self.exercise()
        self.exercise(body_fails=True)

    def test_restores_after_setup_failure(self):
        self.exercise(fail_operation='unlock-keychain')

    def test_cleanup_attempts_all_steps_when_default_restore_fails(self):
        calls = []
        current = '/original'
        def command(*args):
            nonlocal current
            calls.append(args)
            if args == ('default-keychain', '-d', 'user', '-s', '/original'):
                raise RuntimeError('restore failure')
            if args[0] in ('default-keychain', 'list-keychains'):
                if '-s' in args:
                    current = args[4]
                    return ''
                return shlex.quote(current)
            return ''
        with self.assertRaisesRegex(RuntimeError, 'cleanup failed'):
            with isolated_keychain(Path('/temporary'), command):
                pass
        self.assertEqual(calls[-2:], [('list-keychains', '-d', 'user', '-s', '/original'),
                                     ('delete-keychain', '/temporary')])


if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']:
        unittest.main(argv=[sys.argv[0]])
    else:
        main()
