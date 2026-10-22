"""Regenerates the inventory fetch wire corpus (recorder R3b) in the viewer and in the crate.

usage: python tools/inventory-fixtures/ais-fetch/regenerate.py <viewer checkout> <test executable> [<library dir>...]

  <viewer checkout>  the Vulkanstorm viewer working tree the test was built from
  <test executable>  INTEGRATION_TEST_llinvfetch_parity from that tree's test
                     build (LL_TESTS ON; see the README)
  <library dir>      directories prepended to PATH / LD_LIBRARY_PATH so the
                     executable finds its shared libraries (on Windows the
                     build's sharedlibs/<config> directory and
                     sharedlibs/<config>/Release, packages/lib/release)

The executable always runs under the viewer's
indra/newview/tests/vksinventory_mock_server.py (a loopback AIS and legacy
capability server on a port the operating system assigns, its request log in
a temporary file of its own), so two runs never share anything.

Steps, all in a fresh temporary tree first:
 1. Dump: VKS_INVFETCH_DUMP=<tmp>/fetch. Dump mode always exits non-zero;
    the script requires that, one "recorded" line per scenario, and
    "(0 scenarios with failures)". The README and .gitattributes come from
    the viewer's fixture directory.
 2. Replay the new tree (VKS_INVFETCH_FIXTURES=<tmp>/fetch, no dump): the
    copy check, every scenario, the response bodies, index.txt,
    manifest.txt and the exact file set must pass.
 3. Only then the tree replaces indra/newview/tests/fixtures/inventory/ais/fetch
    in the viewer and crates/vkstorm-inventory/tests/fixtures/ais/fetch in
    the crate, byte for byte; both are compared with the temporary tree
    afterwards.

The headers name the viewer commit (git rev-parse --short=10 HEAD).
"""
import filecmp
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CORE = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
CRATE_DIR = os.path.join(CORE, "crates", "vkstorm-inventory", "tests", "fixtures", "ais", "fetch")
VIEWER_REL = os.path.join("indra", "newview", "tests", "fixtures", "inventory", "ais", "fetch")
MOCK_REL = os.path.join("indra", "newview", "tests", "vksinventory_mock_server.py")


def git(viewer, *args):
    return subprocess.run(["git", "-C", viewer] + list(args), capture_output=True, text=True,
                          check=True).stdout.strip()


def replace_tree(src, dest):
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    shutil.copytree(src, dest)


def same_tree(a, b):
    cmp = filecmp.dircmp(a, b)
    if cmp.left_only or cmp.right_only or cmp.funny_files:
        return False
    _, mismatch, errors = filecmp.cmpfiles(a, b, cmp.common_files, shallow=False)
    if mismatch or errors:
        return False
    return all(same_tree(os.path.join(a, d), os.path.join(b, d)) for d in cmp.common_dirs)


def run(viewer, exe, env):
    cmd = [sys.executable, "-I", os.path.join(viewer, MOCK_REL), "--", exe]
    r = subprocess.run(cmd, env=env, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    viewer = os.path.abspath(sys.argv[1])
    exe = os.path.abspath(sys.argv[2])
    libs = [os.path.abspath(x) for x in sys.argv[3:]]
    vdir = os.path.join(viewer, VIEWER_REL)

    env = dict(os.environ)
    for var in ("VKS_INVFETCH_DUMP", "VKS_INVFETCH_FIXTURES", "VKS_INVFETCH_ONLY", "VKS_INVFETCH_DEV",
                "VKS_INVFETCH_ACTUAL", "VKS_HTTP_MOCK_LOG", "LL_TEST_PORT"):
        env.pop(var, None)
    if libs:
        for var in ("PATH", "LD_LIBRARY_PATH"):
            env[var] = os.pathsep.join(libs + ([env[var]] if env.get(var) else []))
    env["VKS_INVFETCH_COMMIT"] = git(viewer, "rev-parse", "--short=10", "HEAD")

    with tempfile.TemporaryDirectory() as tmp:
        tree = os.path.join(tmp, "fetch")
        os.makedirs(tree)
        for name in ("README.md", ".gitattributes"):
            shutil.copy2(os.path.join(vdir, name), os.path.join(tree, name))

        rc, out = run(viewer, exe, dict(env, VKS_INVFETCH_DUMP=tree))
        sys.stdout.write(out[-4000:])
        recorded = re.findall(r"\[r3b\] recorded (\S+)", out)
        index = [l for l in open(os.path.join(tree, "index.txt"), encoding="utf-8").read().splitlines()
                 if l and not l.startswith("#")] if os.path.exists(os.path.join(tree, "index.txt")) else []
        if rc == 0 or "(0 scenarios with failures)" not in out or not recorded or recorded != index:
            print("dump did not run as expected (it must record every scenario of index.txt, report no failures and exit non-zero)")
            return 1

        rc, out = run(viewer, exe, dict(env, VKS_INVFETCH_FIXTURES=tree))
        sys.stdout.write(out[-4000:])
        if rc != 0:
            print("the new tree does not replay: nothing replaced")
            return 1

        replace_tree(tree, vdir)
        replace_tree(tree, CRATE_DIR)
        if not (same_tree(tree, vdir) and same_tree(tree, CRATE_DIR)):
            print("the copied trees differ from the replayed one")
            return 1
    print("fixtures written to", vdir, "and", CRATE_DIR)
    return 0


if __name__ == "__main__":
    sys.exit(main())
