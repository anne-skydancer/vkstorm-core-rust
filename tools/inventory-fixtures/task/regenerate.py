"""Regenerates the task (object contents) fixtures from the viewer's C++ oracle.

usage: python tools/inventory-fixtures/task/regenerate.py <INTEGRATION_TEST_llinventory_task_oracle executable>
           [--viewer <viewer checkout>] [--libpath <dir>]...

The executable is built by the viewer's test build (target
INTEGRATION_TEST_llinventory_task_oracle, indra/newview/tests/inventory/r4-task.cmake);
see README.md. It is run with VKS_TASK_DUMP=<temporary directory>; the output
replaces crates/vkstorm-inventory/tests/fixtures/task/ in this repository and,
with --viewer, indra/newview/tests/fixtures/inventory/task/ in the viewer
checkout, so both hold identical bytes. README.md and .gitattributes are kept
in both places.
--libpath adds a directory to PATH (Windows) or LD_LIBRARY_PATH (Linux) for
the run, as the viewer's test runner does.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile


KEPT = ("README.md", ".gitattributes")


def replace_tree(src, dest):
    kept = {}
    for name in KEPT:
        path = os.path.join(dest, name)
        if os.path.isfile(path):
            with open(path, "rb") as f:
                kept[name] = f.read()
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    shutil.copytree(src, dest)
    for name, data in kept.items():
        with open(os.path.join(dest, name), "wb") as f:
            f.write(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("executable")
    parser.add_argument("--viewer", help="viewer checkout whose fixtures are replaced too")
    parser.add_argument("--libpath", action="append", default=[], help="directory added to the library search path")
    args = parser.parse_args()

    exe = os.path.abspath(args.executable)
    here = os.path.dirname(os.path.abspath(__file__))
    crate = os.path.normpath(os.path.join(here, "..", "..", "..", "crates", "vkstorm-inventory", "tests", "fixtures", "task"))
    env = dict(os.environ)
    if args.libpath:
        var = "PATH" if os.name == "nt" else "LD_LIBRARY_PATH"
        env[var] = os.pathsep.join(args.libpath + ([env[var]] if env.get(var) else []))
    # the scratch directory lives in the repository's (ignored) target/, not
    # in the system temporary directory
    scratch_root = os.path.normpath(os.path.join(here, "..", "..", "..", "target"))
    os.makedirs(scratch_root, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=scratch_root) as tmp:
        out = os.path.join(tmp, "task")
        env["VKS_TASK_DUMP"] = out
        subprocess.run([exe], check=True, env=env, cwd=os.path.dirname(exe))
        replace_tree(out, crate)
        print("fixtures written to", crate)
        if args.viewer:
            viewer = os.path.join(os.path.abspath(args.viewer), "indra", "newview", "tests", "fixtures", "inventory", "task")
            replace_tree(out, viewer)
            print("fixtures written to", viewer)
    return 0


if __name__ == "__main__":
    sys.exit(main())
