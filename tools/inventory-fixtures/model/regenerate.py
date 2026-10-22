"""Regenerates the inventory data model fixtures (rung seven, recorder R1) in
both repositories from the viewer's llinventory_tool:

  <viewer>/indra/llinventory/tests/fixtures/
  crates/vkstorm-inventory/tests/fixtures/model/

usage: python tools/inventory-fixtures/model/regenerate.py <llinventory_tool> <viewer checkout> [--source <commit>]

The tool is built by the viewer's test build (target llinventory_tool); see
README.md. The values come from the frozen copies of the pre-port C++ in
llinventory_oracle.cpp, so the fixtures do not change when the live C++ is
ported. The hand-written README.md, the .gitattributes and the Library cache
in library/ are kept as they are in the viewer and copied to the crate
unchanged. --source names the recorded C++ in the file headers (default:
472ecf270d, the commit the frozen copies were made from).

Nothing is replaced until the new recording is verified: the tool dumps twice,
each run into its own new directory, each must end with the status of a
recording (3; a dump never exits 0, so it cannot pass for a test), the two
trees must be byte-identical and must hold exactly the expected files. After
the replacement the viewer and crate trees must be byte-identical.
"""
import os
import shutil
import subprocess
import sys
import tempfile

LIBRARY = os.path.join("library", "ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz")
KEEP = {"README.md", "library", ".gitattributes"}
DUMP_RECORDED = 3
DUMPED = {"c1_types.txt", "c2_normalisers.txt", "c3_permissions.txt", "c4_sale.txt", "c5_items.txt", "c5_matrix.txt",
          "c5_categories.txt", "c6_legacy.txt", "c7_item_llsd.txt", "c8_category_llsd.txt", "c9_messages.txt",
          "c3_random_sample.txt", "c4_random_sample.txt", "c5_random_sample.txt", "digests.txt", "c10_library.txt.gz"}


def tree(path):
    out = {}
    for base, _, files in os.walk(path):
        for f in files:
            full = os.path.join(base, f)
            with open(full, "rb") as fh:
                out[os.path.relpath(full, path).replace(os.sep, "/")] = fh.read()
    return out


def dump(tool, dest, source, library):
    proc = subprocess.run([tool, "dump", dest, "--source", source, "--library", library])
    if proc.returncode != DUMP_RECORDED:
        raise SystemExit("llinventory_tool dump exited %d, not %d (a recording)" % (proc.returncode, DUMP_RECORDED))
    got = set(tree(dest))
    if got != DUMPED:
        raise SystemExit("the dump wrote %s; expected %s" % (sorted(got), sorted(DUMPED)))


def main():
    args = sys.argv[1:]
    source = "472ecf270d"
    if "--source" in args:
        i = args.index("--source")
        source = args[i + 1]
        del args[i:i + 2]
    if len(args) != 2:
        print(__doc__)
        return 2
    tool, viewer = args
    here = os.path.dirname(os.path.abspath(__file__))
    crate = os.path.normpath(os.path.join(here, "..", "..", "..", "crates", "vkstorm-inventory", "tests", "fixtures", "model"))
    view = os.path.join(viewer, "indra", "llinventory", "tests", "fixtures")
    library = os.path.join(view, LIBRARY)
    for kept in ("README.md", ".gitattributes", LIBRARY):
        if not os.path.isfile(os.path.join(view, kept)):
            print("missing " + os.path.join(view, kept))
            return 1
    with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
        dump(tool, a, source, library)
        dump(tool, b, source, library)
        ta, tb = tree(a), tree(b)
        differ = sorted(k for k in ta if ta[k] != tb.get(k))
        if differ:
            print("two dumps differ (the recording is not deterministic): " + ", ".join(differ))
            return 1
        for dest in (view, crate):
            os.makedirs(dest, exist_ok=True)
            for name in os.listdir(dest):
                if name not in KEEP:
                    full = os.path.join(dest, name)
                    if os.path.isdir(full):
                        shutil.rmtree(full)
                    else:
                        os.remove(full)
            for name in ta:
                shutil.copyfile(os.path.join(a, name), os.path.join(dest, name))
    os.makedirs(os.path.join(crate, "library"), exist_ok=True)
    shutil.copyfile(library, os.path.join(crate, LIBRARY))
    for kept in ("README.md", ".gitattributes"):
        shutil.copyfile(os.path.join(view, kept), os.path.join(crate, kept))
    tv, tc = tree(view), tree(crate)
    if tv != tc:
        print("the viewer and crate copies differ: " + ", ".join(sorted(k for k in set(tv) | set(tc) if tv.get(k) != tc.get(k))))
        return 1
    print("verified (two identical dumps) and written to", view, "and", crate)
    return 0


if __name__ == "__main__":
    sys.exit(main())
