"""Regenerates the AIS update corpus (recorder R3a) in the viewer and in the crate.

usage: python tools/inventory-fixtures/ais-update/regenerate.py <viewer checkout> <test executable> [<library dir>...]

  <viewer checkout>  the Vulkanstorm viewer working tree the test was built from
  <test executable>  INTEGRATION_TEST_llaisupdate_parity from that tree's test
                     build (LL_TESTS ON; see the README)
  <library dir>      directories prepended to PATH / LD_LIBRARY_PATH so the
                     executable finds its shared libraries (on Windows the
                     build's sharedlibs/<config> directory)

Steps, all in a fresh temporary tree first:
 1. gen_cases.py writes the inputs into <tmp>/update/cases; the README and
    .gitattributes come from the viewer's fixture directory.
 2. The test runs in dump mode (VKS_AISUPDATE_DUMP=<tmp>/update/expected,
    VKS_AISUPDATE_FIXTURES=<tmp>/update). Dump mode always exits non-zero;
    the script requires that, and the "dumped N cases" line.
 3. The test replays the new tree (VKS_AISUPDATE_FIXTURES=<tmp>/update, no
    dump): every check, the drift guard included, must pass.
 4. Only then the tree replaces indra/newview/tests/fixtures/inventory/ais/update
    in the viewer and crates/vkstorm-inventory/tests/fixtures/ais/update in
    the crate, byte for byte.

The headers name the viewer commit (git rev-parse HEAD of the checkout) and
the blob of llaisapi.cpp.
"""
import os
import re
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


def replace_tree(src, dest):
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    shutil.copytree(src, dest)


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    viewer = os.path.abspath(sys.argv[1])
    exe = os.path.abspath(sys.argv[2])
    libs = [os.path.abspath(x) for x in sys.argv[3:]]
    vdir = os.path.join(viewer, VIEWER_REL)

    env = dict(os.environ)
    for var in ("VKS_AISUPDATE_DUMP", "VKS_AISUPDATE_FIXTURES", "VKS_AISUPDATE_FULL"):
        env.pop(var, None)
    if libs:
        for var in ("PATH", "LD_LIBRARY_PATH"):
            env[var] = os.pathsep.join(libs + ([env[var]] if env.get(var) else []))
    commit = git(viewer, "rev-parse", "HEAD")
    blob = git(viewer, "hash-object", os.path.join("indra", "newview", "llaisapi.cpp"))
    env["VKS_AISUPDATE_VIEWER_COMMIT"] = "%s (llaisapi.cpp blob %s)" % (commit, blob)

    with tempfile.TemporaryDirectory() as tmp:
        tree = os.path.join(tmp, "update")
        os.makedirs(os.path.join(tree, "cases"))
        for name in ("README.md", ".gitattributes"):
            shutil.copy2(os.path.join(vdir, name), os.path.join(tree, name))
        subprocess.run([sys.executable, "-I", os.path.join(HERE, "gen_cases.py"), os.path.join(tree, "cases")],
                       check=True)

        dump_env = dict(env, VKS_AISUPDATE_DUMP=os.path.join(tree, "expected"), VKS_AISUPDATE_FIXTURES=tree)
        r = subprocess.run([exe], env=dump_env, capture_output=True, text=True)
        sys.stdout.write(r.stdout[-3000:])
        if r.returncode == 0 or not re.search(r"\[aisupdate\] dumped \d+ cases", r.stdout):
            print("dump did not run as expected (it must report the cases and exit non-zero)")
            return 1

        replay_env = dict(env, VKS_AISUPDATE_FIXTURES=tree)
        r = subprocess.run([exe], env=replay_env, capture_output=True, text=True)
        sys.stdout.write(r.stdout[-3000:])
        if r.returncode != 0:
            print("the new tree does not replay: nothing replaced")
            return 1

        replace_tree(tree, vdir)
        replace_tree(tree, CRATE_DIR)
    print("fixtures written to", vdir, "and", CRATE_DIR)
    return 0


if __name__ == "__main__":
    sys.exit(main())
