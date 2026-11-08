# SPDX-License-Identifier: GPL-3.0-or-later
"""The Rust core is GPL-3.0-or-later: licence text, Cargo licences and the packaged notices."""
from pathlib import Path
import hashlib
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
# sha256 of https://www.gnu.org/licenses/gpl-3.0.txt
GPL3_SHA256 = "3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986"
# Vendored ports that keep their own licence beside the GPL.
OWN_LICENCE = {
    "vkstorm-openjp2": "BSD-2-Clause",
    "vkstorm-audio": "GPL-3.0-or-later AND Zlib",
    "vkstorm-vorbis": "BSD-3-Clause AND GPL-3.0-or-later",
}


class LicenceTest(unittest.TestCase):
    def test_licence_is_the_gpl3_text(self):
        self.assertEqual(hashlib.sha256((ROOT / "LICENSE").read_bytes()).hexdigest(), GPL3_SHA256)
        self.assertFalse((ROOT / "LICENSE-GPL-3.0").exists())

    def test_lgpl_text_is_kept_for_the_ported_headers(self):
        self.assertIn(b"Version 2.1, February 1999", (ROOT / "LICENSE-LGPL-2.1").read_bytes())

    def test_workspace_licence(self):
        cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        self.assertRegex(cargo, r'(?m)^license = "GPL-3\.0-or-later"$')

    def test_every_crate_licence(self):
        for manifest in sorted(ROOT.glob("crates/*/Cargo.toml")) + sorted(ROOT.glob("tools/*/Cargo.toml")):
            text = manifest.read_text(encoding="utf-8")
            name = manifest.parent.name
            match = re.search(r'(?m)^license(?:\.workspace)? = (.*)$', text)
            with self.subTest(crate=name):
                self.assertIsNotNone(match)
                if name in OWN_LICENCE:
                    self.assertEqual(match.group(1), '"%s"' % OWN_LICENCE[name])
                else:
                    self.assertEqual(match.group(1), "true")
                self.assertNotIn("LGPL", match.group(1))

    def test_no_lgpl_option_in_spdx_headers(self):
        for path in list((ROOT / "crates").rglob("*.rs")) + list((ROOT / "tools").rglob("*.rs")):
            head = path.read_bytes()[:400].decode("utf-8", "replace")
            if "SPDX-License-Identifier:" in head:
                with self.subTest(path=str(path.relative_to(ROOT))):
                    line = [l for l in head.splitlines() if "SPDX-License-Identifier:" in l][0]
                    self.assertEqual(line.split("SPDX-License-Identifier:", 1)[1].strip(), "GPL-3.0-or-later")
                    self.assertNotIn("LGPL", line)

    def test_packaging_ships_the_gpl3_and_vendored_notices(self):
        script = (ROOT / "packaging" / "build_package.py").read_text(encoding="utf-8")
        self.assertIn('("NOTICE.md", "LICENSE", "LICENSE-LGPL-2.1")', script)
        self.assertNotIn("LICENSE-GPL-3.0", script)
        for notice in ('"vkstorm-openjp2": ("LICENSE", "NOTICE.md")', '"vkstorm-vorbis": ("LICENSE-xiph",)',
                       '"vkstorm-audio": ("LICENSE-soloud",)'):
            self.assertIn(notice, script)
        for path in ("crates/vkstorm-openjp2/LICENSE", "crates/vkstorm-openjp2/NOTICE.md",
                     "crates/vkstorm-vorbis/LICENSE-xiph", "crates/vkstorm-audio/LICENSE-soloud"):
            self.assertTrue((ROOT / path).is_file(), path)


if __name__ == "__main__":
    unittest.main()
