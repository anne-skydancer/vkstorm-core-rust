"""Regenerates the task (object contents) fixtures from the viewer's C++ oracle.

usage: python tools/inventory-fixtures/task/regenerate.py <INTEGRATION_TEST_llinventory_task_oracle executable>
           [--viewer <viewer checkout>] [--libpath <dir>]...

The executable is built by the viewer's test build (target
INTEGRATION_TEST_llinventory_task_oracle, indra/newview/tests/inventory/r4-task.cmake);
see README.md. The driver:

1. runs it twice with VKS_TASK_DUMP=<scratch>/a and <scratch>/b. A dump run
   always exits non-zero (a dump is never a passing test); the driver checks
   that each run wrote all four sets and nothing else failed;
2. compares the two trees byte for byte and stops if they differ (a
   non-deterministic recording is never installed);
3. replaces crates/vkstorm-inventory/tests/fixtures/task/ in this repository
   and, with --viewer, indra/newview/tests/fixtures/inventory/task/ in the
   viewer checkout (README.md and .gitattributes are kept in both);
4. with --viewer, runs the executable once more without VKS_TASK_DUMP: the
   replay against the new viewer fixtures (the path compiled into the
   executable) must pass.

--libpath adds a directory to PATH (Windows) or LD_LIBRARY_PATH (Linux) for
the runs, as the viewer's test runner does. The scratch directory is created
under this repository's ignored target/, not the system temporary directory.
"""
import argparse
import filecmp
import os
import re
import shutil
import subprocess
import sys
import tempfile

KEPT = ("README.md", ".gitattributes")
SETS = 4


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


def tree_files(root):
    out = set()
    for d, _, files in os.walk(root):
        for f in files:
            out.add(os.path.relpath(os.path.join(d, f), root).replace(os.sep, "/"))
    return out


def dump(exe, env, out):
    env = dict(env)
    env["VKS_TASK_DUMP"] = out
    p = subprocess.run([exe], env=env, cwd=os.path.dirname(exe), capture_output=True, text=True, errors="replace")
    log = p.stdout + p.stderr
    wrote = re.findall(r"VKS_TASK_DUMP: wrote ", log)
    failed = re.findall(r"\[llinventory_task_oracle, (\d+): [^\]]*\] fail: '([^\n]*)", log)
    dump_failures = [f for f in failed if f[1].startswith("VKS_TASK_DUMP: fixtures written")]
    other = [f for f in failed if not f[1].startswith("VKS_TASK_DUMP: fixtures written")]
    if p.returncode == 0 or len(wrote) != SETS or len(dump_failures) != SETS or other:
        sys.stderr.write(log)
        raise SystemExit("dump run into %s did not write the %d sets cleanly (rc %d, %d written, other failures: %s)"
                         % (out, SETS, p.returncode, len(wrote), other))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("executable")
    parser.add_argument("--viewer", help="viewer checkout whose fixtures are replaced too, then replayed")
    parser.add_argument("--libpath", action="append", default=[], help="directory added to the library search path")
    args = parser.parse_args()

    exe = os.path.abspath(args.executable)
    here = os.path.dirname(os.path.abspath(__file__))
    crate = os.path.normpath(os.path.join(here, "..", "..", "..", "crates", "vkstorm-inventory", "tests", "fixtures", "task"))
    env = dict(os.environ)
    env.pop("VKS_TASK_ONLY", None)
    if args.libpath:
        var = "PATH" if os.name == "nt" else "LD_LIBRARY_PATH"
        env[var] = os.pathsep.join(args.libpath + ([env[var]] if env.get(var) else []))
    scratch_root = os.path.normpath(os.path.join(here, "..", "..", "..", "target"))
    os.makedirs(scratch_root, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=scratch_root) as tmp:
        a = os.path.join(tmp, "a")
        b = os.path.join(tmp, "b")
        dump(exe, env, a)
        dump(exe, env, b)
        fa, fb = tree_files(a), tree_files(b)
        differing = sorted((fa ^ fb) | {f for f in fa & fb
                                       if not filecmp.cmp(os.path.join(a, f), os.path.join(b, f), shallow=False)})
        if differing:
            raise SystemExit("two dumps differ (%d files, e.g. %s); nothing installed" % (len(differing), differing[:5]))
        print("two dumps identical: %d files" % len(fa))
        replace_tree(a, crate)
        print("fixtures written to", crate)
        if args.viewer:
            viewer = os.path.join(os.path.abspath(args.viewer), "indra", "newview", "tests", "fixtures", "inventory", "task")
            replace_tree(a, viewer)
            print("fixtures written to", viewer)
            p = subprocess.run([exe], env=env, cwd=os.path.dirname(exe), capture_output=True, text=True, errors="replace")
            if p.returncode != 0:
                sys.stderr.write(p.stdout + p.stderr)
                raise SystemExit("replay against the new viewer fixtures failed (rc %d); is the executable built from %s?"
                                 % (p.returncode, args.viewer))
            print("replay against the new viewer fixtures passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
