"""Regenerates the cache and msgs fixtures of vkstorm-inventory (recorder R2)
from the viewer's C++ oracle, in the crate and in the viewer.

usage:
  python tools/inventory-fixtures/cache/regenerate.py <test executable> <viewer repository root>
         [--commit <viewer commit>] [--libpath <dir>]...

<test executable> is INTEGRATION_TEST_llinventorymodel_parity from the
viewer's test build (LL_TESTS ON; target INTEGRATION_TEST_llinventorymodel_parity,
indra/newview/tests/inventory/r2.cmake). It is run with VKS_INVMODEL_DUMP set
to a temporary directory and writes cache/ and msgs/ there; this script then
replaces the generated files in

  <viewer>/indra/newview/tests/fixtures/inventory/{cache,msgs}
  crates/vkstorm-inventory/tests/fixtures/{cache,msgs}

keeping README.md, .gitattributes and the other standard library's own
records: its <area>/<library>/ tree and its c4_validation_qa.<library>.txt.
Since upstream 28fee038e0 the model's maps are std::unordered_map, so every
record that follows their order is kept per C++ standard library, in
cache/msvc/ and msgs/msvc/ (recorded on Windows) and cache/libstdcxx/ and
msgs/libstdcxx/ (recorded on Linux); the C2 inputs in cache/c2/ are shared.
Run the script on both platforms to refresh both.

A record run never passes (the test fails on purpose after writing
DUMP_COMPLETE), so the exit status is not used: the script records twice
into fresh directories, requires DUMP_COMPLETE in both and the two trees to
be identical, and only then replaces the fixture trees. It finishes by
checking that the viewer and crate copies are byte-identical; replay the
viewer test afterwards to confirm.

The executable needs the viewer's shared libraries: the script adds the
executable's directory, its Release subdirectory, <build>/packages/lib/release
and <build>/sharedlibs/lib to PATH and LD_LIBRARY_PATH, plus any --libpath.
The test reads the Library cache from
<viewer>/indra/llinventory/tests/fixtures/library/ (committed by R1).
"""
import argparse
import filecmp
import os
import shutil
import subprocess
import sys
import tempfile

AREAS = ("cache", "msgs")
KEEP = {"README.md", ".gitattributes"}


def other_platform_files():
    """The other standard library's records (a file, or a directory in each area)."""
    other = "libstdcxx" if os.name == "nt" else "msvc"
    return {other, "c4_validation_qa.%s.txt" % other}


def replace_generated(dest, generated):
    os.makedirs(dest, exist_ok=True)
    keep = KEEP | other_platform_files()
    for name in os.listdir(dest):
        path = os.path.join(dest, name)
        if name in keep:
            continue
        if os.path.isdir(path):
            shutil.rmtree(path)
        else:
            os.remove(path)
    for name in os.listdir(generated):
        src = os.path.join(generated, name)
        dst = os.path.join(dest, name)
        if os.path.isdir(src):
            shutil.copytree(src, dst)
        else:
            shutil.copyfile(src, dst)


def same_tree(a, b):
    cmp = filecmp.dircmp(a, b)
    if cmp.left_only or cmp.right_only or cmp.funny_files:
        return False
    _, mismatch, errors = filecmp.cmpfiles(a, b, cmp.common_files, shallow=False)
    if mismatch or errors:
        return False
    return all(same_tree(os.path.join(a, d), os.path.join(b, d)) for d in cmp.common_dirs)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("exe")
    parser.add_argument("viewer")
    parser.add_argument("--commit")
    parser.add_argument("--libpath", action="append", default=[])
    args = parser.parse_args()

    here = os.path.dirname(os.path.abspath(__file__))
    crate = os.path.normpath(os.path.join(here, "..", "..", "..", "crates", "vkstorm-inventory", "tests", "fixtures"))
    viewer_fixtures = os.path.join(args.viewer, "indra", "newview", "tests", "fixtures", "inventory")
    commit = args.commit or subprocess.run(["git", "-C", args.viewer, "rev-parse", "--short=10", "HEAD"],
                                           check=True, capture_output=True, text=True).stdout.strip()

    exe = os.path.abspath(args.exe)
    exe_dir = os.path.dirname(exe)
    build = os.path.normpath(os.path.join(exe_dir, "..", ".."))
    libs = [exe_dir, os.path.join(exe_dir, "Release"), os.path.join(build, "packages", "lib", "release"),
            os.path.join(build, "sharedlibs", "lib")] + args.libpath
    env = dict(os.environ)
    for var in ("PATH", "LD_LIBRARY_PATH"):
        env[var] = os.pathsep.join(libs + ([env[var]] if env.get(var) else []))

    with tempfile.TemporaryDirectory() as tmp:
        runs = []
        for k in (1, 2):
            out = os.path.join(tmp, "run%d" % k)
            os.makedirs(out)
            env["VKS_INVMODEL_DUMP"] = out
            env["VKS_INVMODEL_COMMIT"] = commit
            subprocess.run([exe], env=env)  # a record run exits non-zero by design
            if not os.path.isfile(os.path.join(out, "DUMP_COMPLETE")):
                print("record run %d did not complete (no DUMP_COMPLETE)" % k)
                return 1
            runs.append(out)
        for area in AREAS:
            if not same_tree(os.path.join(runs[0], area), os.path.join(runs[1], area)):
                print("the two record runs differ in %s/: not replacing anything" % area)
                return 1
        for area in AREAS:
            generated = os.path.join(runs[0], area)
            replace_generated(os.path.join(viewer_fixtures, area), generated)
            replace_generated(os.path.join(crate, area), generated)
    for area in AREAS:
        if not same_tree(os.path.join(viewer_fixtures, area), os.path.join(crate, area)):
            print("the viewer and crate copies of %s/ differ" % area)
            return 1
    print("fixtures written to %s and %s (viewer commit %s)" % (viewer_fixtures, crate, commit))
    return 0


if __name__ == "__main__":
    sys.exit(main())
