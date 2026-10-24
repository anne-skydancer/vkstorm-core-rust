"""Regenerates the offer and give fixtures of the inventory rung (recorder R4b).

usage: python tools/inventory-fixtures/offer/regenerate.py <test executable> <viewer checkout> [viewer commit]

<test executable> is INTEGRATION_TEST_llinventory_offer_oracle from the
viewer's test build (LL_TESTS ON); <viewer checkout> is the viewer worktree it
was built from. The script

  1. runs the executable's dump mode (VKS_R4B_DUMP) twice, each into its own
     fresh temporary directory. A dump always exits non-zero by design; it is
     accepted only when it wrote its DUMP_COMPLETE marker;
  2. requires the two dumps to be byte-identical;
  3. replaces the generated files in both
       <viewer>/indra/newview/tests/fixtures/inventory/{offer,give}/
       crates/vkstorm-inventory/tests/fixtures/{offer,give}/
     so the two copies stay byte-identical (README.md and .gitattributes are
     not generated; the crate's are copied from the viewer's);
  4. runs the executable once more in replay mode against the new viewer
     fixtures and requires it to pass.
"""
import filecmp
import os
import shutil
import subprocess
import sys
import tempfile

AREAS = ("offer", "give")
KEEP = ("README.md", ".gitattributes")


def env_for(exe):
    env = dict(os.environ)
    for k in ("VKS_R4B_DUMP", "VKS_R4B_ONLY", "VKS_R4B_FULL", "VKS_R4B_DEVELOPER"):
        env.pop(k, None)
    exe_dir = os.path.dirname(exe)
    if os.name != "nt":
        build = exe_dir
        while build != os.path.dirname(build) and not os.path.isdir(os.path.join(build, "sharedlibs")):
            build = os.path.dirname(build)
        extra = [exe_dir, os.path.join(build, "sharedlibs"), os.path.join(build, "sharedlibs", "lib")]
        env["LD_LIBRARY_PATH"] = os.pathsep.join(extra + [env.get("LD_LIBRARY_PATH", "")])
    return env


def dump(exe, commit, into):
    env = env_for(exe)
    env["VKS_R4B_DUMP"] = into
    env["VKS_R4B_COMMIT"] = commit
    subprocess.run([exe], cwd=os.path.dirname(exe), env=env)
    marker = os.path.join(into, "DUMP_COMPLETE")
    if not os.path.exists(marker):
        raise SystemExit("dump did not complete: " + into)
    os.remove(marker)


def same_tree(a, b):
    for area in AREAS:
        cmp = filecmp.dircmp(os.path.join(a, area), os.path.join(b, area))
        if cmp.left_only or cmp.right_only or cmp.diff_files:
            return False
        for name in cmp.common_files:
            if not filecmp.cmp(os.path.join(a, area, name), os.path.join(b, area, name), shallow=False):
                return False
    return True


def main():
    if len(sys.argv) not in (3, 4):
        print(__doc__)
        return 2
    exe = os.path.abspath(sys.argv[1])
    viewer = os.path.abspath(sys.argv[2])
    commit = sys.argv[3] if len(sys.argv) == 4 else subprocess.run(
        ["git", "rev-parse", "--short=10", "HEAD"], cwd=viewer, capture_output=True, text=True, check=True).stdout.strip()
    here = os.path.dirname(os.path.abspath(__file__))
    crate = os.path.normpath(os.path.join(here, "..", "..", "..", "crates", "vkstorm-inventory", "tests", "fixtures"))
    viewer_fixtures = os.path.join(viewer, "indra", "newview", "tests", "fixtures", "inventory")
    with tempfile.TemporaryDirectory() as t1, tempfile.TemporaryDirectory() as t2:
        dump(exe, commit, t1)
        dump(exe, commit, t2)
        if not same_tree(t1, t2):
            raise SystemExit("the two dumps differ: the oracle is not deterministic")
        for area in AREAS:
            src = os.path.join(t1, area)
            for dest in (os.path.join(viewer_fixtures, area), os.path.join(crate, area)):
                os.makedirs(dest, exist_ok=True)
                for name in os.listdir(dest):
                    if name not in KEEP:
                        os.remove(os.path.join(dest, name))
                for name in sorted(os.listdir(src)):
                    shutil.copyfile(os.path.join(src, name), os.path.join(dest, name))
            for name in KEEP:
                v = os.path.join(viewer_fixtures, area, name)
                if os.path.exists(v):
                    shutil.copyfile(v, os.path.join(crate, area, name))
    rc = subprocess.run([exe], cwd=os.path.dirname(exe), env=env_for(exe)).returncode
    if rc != 0:
        raise SystemExit("the replay of the new fixtures failed (rc %d)" % rc)
    print("fixtures written to", viewer_fixtures, "and", crate, "and replayed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
