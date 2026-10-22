"""Regenerates the inventory data model fixtures (rung seven, recorder R1) in
both repositories from the viewer's llinventory_tool:

  <viewer>/indra/llinventory/tests/fixtures/
  crates/vkstorm-inventory/tests/fixtures/model/

usage: python tools/inventory-fixtures/model/regenerate.py <llinventory_tool> <viewer checkout> [--source <commit>]

The tool is built by the viewer's test build (target llinventory_tool); see
README.md. The values come from the frozen copies of the pre-port C++ in
llinventory_oracle.cpp, so the fixtures do not change when the live C++ is
ported. The hand-written README.md and the Library cache in library/ are kept
as they are in the viewer and copied to the crate unchanged. --source names
the recorded C++ in the file headers (default: 472ecf270d, the commit the
frozen copies were made from).
"""
import filecmp
import os
import shutil
import subprocess
import sys
import tempfile

LIBRARY = os.path.join("library", "ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz")
KEEP = {"README.md", "library", ".gitattributes"}


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
    if not os.path.isfile(library):
        print("missing " + library)
        return 1
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run([tool, "dump", tmp, "--source", source, "--library", library], check=True)
        for dest in (view, crate):
            os.makedirs(dest, exist_ok=True)
            for name in os.listdir(dest):
                if name not in KEEP:
                    os.remove(os.path.join(dest, name))
            for name in os.listdir(tmp):
                shutil.copyfile(os.path.join(tmp, name), os.path.join(dest, name))
    os.makedirs(os.path.join(crate, "library"), exist_ok=True)
    shutil.copyfile(library, os.path.join(crate, LIBRARY))
    shutil.copyfile(os.path.join(view, "README.md"), os.path.join(crate, "README.md"))
    # Both copies must hold the same bytes.
    cmp = filecmp.dircmp(view, crate)
    if cmp.left_only or cmp.right_only or cmp.diff_files or filecmp.cmpfiles(view, crate, [LIBRARY], shallow=False)[1]:
        print("the viewer and crate copies differ:", cmp.left_only, cmp.right_only, cmp.diff_files)
        return 1
    print("fixtures written to", view, "and", crate)
    return 0


if __name__ == "__main__":
    sys.exit(main())
