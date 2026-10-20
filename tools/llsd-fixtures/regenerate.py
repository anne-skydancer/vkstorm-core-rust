"""Regenerates crates/vkstorm-llsd/tests/fixtures from the viewer's C++ oracle.

usage: python tools/llsd-fixtures/regenerate.py <path to llsdserialize_tool executable>

The executable is built by the viewer's test build (target llsdserialize_tool);
see README.md.
"""
import os
import shutil
import subprocess
import sys
import tempfile


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    tool = sys.argv[1]
    here = os.path.dirname(os.path.abspath(__file__))
    dest = os.path.normpath(os.path.join(here, "..", "..", "crates", "vkstorm-llsd", "tests", "fixtures"))
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run([tool, "dump", tmp], check=True)
        if os.path.isdir(dest):
            shutil.rmtree(dest)
        shutil.copytree(tmp, dest)
    print("fixtures written to", dest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
