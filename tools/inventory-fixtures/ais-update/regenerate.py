"""Regenerates the AIS update corpus (recorder R3a) in the viewer and in the crate.

usage: python tools/inventory-fixtures/ais-update/regenerate.py <viewer checkout> <test executable> [<library dir>...]

  <viewer checkout>  the Vulkanstorm viewer working tree the test was built from
  <test executable>  INTEGRATION_TEST_llaisupdate_parity from that tree's test
                     build (LL_TESTS ON; see the README)
  <library dir>      directories prepended to PATH / LD_LIBRARY_PATH so the
                     executable finds its shared libraries (on Windows the
                     build's sharedlibs/<config> directory)

Steps:
 1. gen_cases.py writes the inputs into the viewer's
    indra/newview/tests/fixtures/inventory/ais/update/cases (the test reads
    them from there: its fixture path is compiled in).
 2. The test runs in dump mode (VKS_AISUPDATE_DUMP) and writes the expected
    outputs; they replace .../ais/update/expected.
 3. The whole viewer directory (cases, expected, README.md, .gitattributes)
    is copied to crates/vkstorm-inventory/tests/fixtures/ais/update, byte for
    byte.

The headers name the viewer commit (git rev-parse HEAD of the checkout) and
the blob of llaisapi.cpp. Run the viewer test afterwards without the dump
variable: it must report 0 differences.
"""
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CORE = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
CRATE_DIR = os.path.join(CORE, "crates", "vkstorm-inventory", "tests", "fixtures", "ais", "update")
VIEWER_REL = os.path.join("indra", "newview", "tests", "fixtures", "inventory", "ais", "update")


def git(viewer, *args):
    return subprocess.run(["git", "-C", viewer] + list(args), capture_output=True, text=True,
                          check=True).stdout.strip()


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    viewer = os.path.abspath(sys.argv[1])
    exe = os.path.abspath(sys.argv[2])
    libs = [os.path.abspath(x) for x in sys.argv[3:]]
    vdir = os.path.join(viewer, VIEWER_REL)

    subprocess.run([sys.executable, "-I", os.path.join(HERE, "gen_cases.py"), os.path.join(vdir, "cases")], check=True)

    env = dict(os.environ)
    if libs:
        for var in ("PATH", "LD_LIBRARY_PATH"):
            env[var] = os.pathsep.join(libs + ([env[var]] if env.get(var) else []))
    commit = git(viewer, "rev-parse", "HEAD")
    blob = git(viewer, "hash-object", os.path.join("indra", "newview", "llaisapi.cpp"))
    env["VKS_AISUPDATE_VIEWER_COMMIT"] = "%s (llaisapi.cpp blob %s)" % (commit, blob)
    with tempfile.TemporaryDirectory() as tmp:
        env["VKS_AISUPDATE_DUMP"] = tmp
        subprocess.run([exe], env=env, check=True)
        expected = os.path.join(vdir, "expected")
        if os.path.isdir(expected):
            shutil.rmtree(expected)
        shutil.copytree(tmp, expected)

    if os.path.isdir(CRATE_DIR):
        shutil.rmtree(CRATE_DIR)
    shutil.copytree(vdir, CRATE_DIR)
    print("fixtures written to", vdir, "and", CRATE_DIR)
    return 0


if __name__ == "__main__":
    sys.exit(main())
