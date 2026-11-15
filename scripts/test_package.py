import os
import subprocess
import sys
import plistlib
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from package import package


class Packages(unittest.TestCase):
    def test_all_platforms(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"test executable")
            for platform in ("ubuntu", "arch", "windows", "macos"):
                with self.subTest(platform=platform):
                    archive = package(platform, "x86_64", binary, root / "dist")
                    self.assertIn(f"-{platform}-x86_64.", archive.name)
                    if platform == "windows":
                        with zipfile.ZipFile(archive) as stream:
                            self.assertTrue(any(n.endswith('/docs/installation.md') for n in stream.namelist()))
                            self.assertTrue(any(n.endswith('/docs/updates.md') for n in stream.namelist()))
                            exe = next(n for n in stream.namelist() if n.endswith('.exe'))
                            self.assertEqual(stream.read(exe), binary.read_bytes())
                        continue
                    with tarfile.open(archive) as stream:
                        files = stream.getnames()
                        self.assertTrue(any(n.endswith('/docs/installation.md') for n in files))
                        self.assertTrue(any(n.endswith('/docs/updates.md') for n in files))
                        suffix = '/Contents/MacOS/ms-teams-agent' if platform == 'macos' else '/ms-teams-agent'
                        executable = next(n for n in files if n.endswith(suffix))
                        self.assertEqual(stream.extractfile(executable).read(), binary.read_bytes())
                        self.assertEqual(stream.getmember(executable).mode & 0o777, 0o755)
                        if platform == 'macos':
                            info = next(n for n in files if n.endswith('/Info.plist'))
                            self.assertEqual(plistlib.load(stream.extractfile(info))['CFBundleExecutable'], 'ms-teams-agent')
                            icon = next(n for n in files if n.endswith('/icon.icns'))
                            self.assertTrue(stream.extractfile(icon).read().startswith(b'icns'))
                        else:
                            self.assertTrue(any(n.endswith('/install.sh') for n in files))
                            self.assertTrue(any(n.endswith('/assets/linux/ms-teams-agent.desktop') for n in files))

    @unittest.skipUnless(sys.platform == 'linux', 'requires Linux install utilities')
    def test_linux_install_and_replace_preserves_user_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            home = root / 'home with spaces'
            home.mkdir()
            binary = root / 'binary'
            binary.write_bytes(b'first version')
            archive = package('ubuntu', 'x86_64', binary, root / 'dist')
            with tarfile.open(archive) as stream:
                stream.extractall(root / 'unpacked', filter='data')
            folder = next((root / 'unpacked').iterdir())
            env = dict(os.environ, HOME=str(home))
            data = home / '.local/share/ms-teams-agent/refresh_token'
            data.parent.mkdir(parents=True)
            data.write_text('existing session')
            for content in (b'first version', b'new version'):
                (folder / 'ms-teams-agent').write_bytes(content)
                subprocess.run(['sh', str(folder / 'install.sh')], env=env, check=True, capture_output=True)
                self.assertEqual((home / '.local/bin/ms-teams-agent').read_bytes(), content)
                self.assertEqual(data.read_text(), 'existing session')
            entry = (home / '.local/share/applications/ms-teams-agent.desktop').read_text()
            self.assertIn(f'Exec="{home}/.local/bin/ms-teams-agent"', entry)


if __name__ == '__main__':
    unittest.main()
