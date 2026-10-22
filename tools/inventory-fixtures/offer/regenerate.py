"""Regenerates the offer and give fixtures of the inventory rung (recorder R4b).

usage: python tools/inventory-fixtures/offer/regenerate.py <test executable> <viewer checkout> [viewer commit]

<test executable> is INTEGRATION_TEST_llinventory_offer_oracle from the
viewer's test build (LL_TESTS ON); <viewer checkout> is the viewer worktree it
was built from. The script runs the executable in its dump mode
(VKS_R4B_DUMP), then copies the generated files into both

  <viewer>/indra/newview/tests/fixtures/inventory/{offer,give}/
  crates/vkstorm-inventory/tests/fixtures/{offer,give}/

so the two copies stay byte-identical. README.md and .gitattributes are not
generated: the crate's are copied from the viewer's. See
crates/vkstorm-inventory/tests/fixtures/offer/README.md.
"""
import os
import shutil
import subprocess
import sys
import tempfile

AREAS = ("offer", "give")
KEEP = ("README.md", ".gitattributes")


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
    env = dict(os.environ)
    exe_dir = os.path.dirname(exe)
    if os.name != "nt":
        build = exe_dir
        while build != os.path.dirname(build) and not os.path.isdir(os.path.join(build, "sharedlibs")):
            build = os.path.dirname(build)
        extra = [exe_dir, os.path.join(build, "sharedlibs"), os.path.join(build, "sharedlibs", "lib")]
        env["LD_LIBRARY_PATH"] = os.pathsep.join(extra + [env.get("LD_LIBRARY_PATH", "")])
    with tempfile.TemporaryDirectory() as tmp:
        env["VKS_R4B_DUMP"] = tmp
        env["VKS_R4B_COMMIT"] = commit
        subprocess.run([exe], cwd=exe_dir, env=env, check=True)
        for area in AREAS:
            src = os.path.join(tmp, area)
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
    print("fixtures written to", viewer_fixtures, "and", crate)
    return 0


if __name__ == "__main__":
    sys.exit(main())
