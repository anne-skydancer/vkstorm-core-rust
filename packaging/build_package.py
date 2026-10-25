"""Build the vkstorm-ffi autobuild package's staging tree.

Run by `autobuild build` (autobuild.xml) from the staging directory, or by hand:

    VKS_FFI_FEATURES="j2c" python packaging/build_package.py <stage-dir>

VKS_FFI_FEATURES names the rungs to build (Cargo features of vkstorm-ffi,
space or comma separated, default "full"); each viewer line links the archive
built with the rungs it has adopted. The stage receives:

    lib/release/vkstorm_ffi.lib | libvkstorm_ffi.a   the static library
    lib/release/vkstorm_ffi.native-libs.txt          system libraries it needs at link time
    include/vkstorm_ffi.h                            the C ABI
    LICENSES/vkstorm-core-rust.txt                   this repository's licence
    LICENSES/third-party.txt                         every crate built in, with its licence
    VERSION.txt                                      <version>.<commit>.<rungs>

The library targets the viewer's CPU baseline (USE_AVX2_OPTIMIZATION:
x86-64-v3) with the toolchain pinned in rust-toolchain.toml.
"""
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
CRATE = "vkstorm-ffi"
PROFILE = "ffi-package"
RUSTFLAGS = "-Ctarget-cpu=x86-64-v3"


def run(*args, capture=False, merge=True):
    """Run in the repository; capture stdout (and stderr when merge, where rustc notes go)."""
    print("+", " ".join(args), flush=True)
    env = dict(os.environ, RUSTFLAGS=RUSTFLAGS)
    proc = subprocess.run(args, cwd=ROOT, env=env, check=True,
                          stdout=subprocess.PIPE if capture else None,
                          stderr=subprocess.STDOUT if capture and merge else None,
                          universal_newlines=True)
    return proc.stdout


def features():
    raw = os.environ.get("VKS_FFI_FEATURES", "full")
    names = sorted({f for f in re.split(r"[\s,]+", raw) if f})
    if not names:
        sys.exit("VKS_FFI_FEATURES names no rung")
    return names


def main():
    stage = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else os.getcwd()).resolve()
    rungs = features()
    feature_arg = ",".join(rungs)
    common = ["-p", CRATE, "--profile", PROFILE, "--locked", "--no-default-features", "--features", feature_arg]

    # One build, printing the system libraries a static library leaves to the final link.
    out = run("cargo", "rustc", *common, "--", "--print", "native-static-libs", capture=True)
    print(out)
    native = re.findall(r"native-static-libs: (.*)", out)
    if not native:
        sys.exit("cargo did not report the native static libraries")

    target = ROOT / "target" / PROFILE
    library = "vkstorm_ffi.lib" if os.name == "nt" else "libvkstorm_ffi.a"
    for sub in ("lib/release", "include", "LICENSES"):
        (stage / sub).mkdir(parents=True, exist_ok=True)
    shutil.copy2(target / library, stage / "lib/release" / library)
    (stage / "lib/release/vkstorm_ffi.native-libs.txt").write_text(native[-1].strip() + "\n", encoding="utf-8")
    shutil.copy2(ROOT / "crates" / CRATE / "include" / "vkstorm_ffi.h", stage / "include" / "vkstorm_ffi.h")
    shutil.copy2(ROOT / "LICENSE", stage / "LICENSES" / "vkstorm-core-rust.txt")

    # Every package compiled into the library (the resolved graph for these features).
    meta = json.loads(run("cargo", "metadata", "--format-version", "1", "--locked",
                          "--filter-platform", host_triple(), capture=True, merge=False))
    tree = run("cargo", "tree", "-p", CRATE, "-e", "normal", "--prefix", "none", "--locked",
               "--no-default-features", "--features", feature_arg, "--format", "{p}", capture=True, merge=False)
    built = {line.split()[0] + " " + line.split()[1] for line in tree.splitlines() if line.strip()}
    rows = []
    for package in meta["packages"]:
        key = package["name"] + " " + "v" + package["version"]
        if key in built:
            rows.append("%s %s  %s" % (package["name"], package["version"], package.get("license") or "see source"))
    (stage / "LICENSES/third-party.txt").write_text(
        "Crates compiled into vkstorm-ffi (" + feature_arg + "):\n\n" + "\n".join(sorted(rows)) + "\n", encoding="utf-8")

    version = meta_version(meta)
    commit = subprocess.run(["git", "rev-parse", "--short=10", "HEAD"], cwd=ROOT, check=True,
                            stdout=subprocess.PIPE, universal_newlines=True).stdout.strip()
    label = "full" if rungs == ["full"] else "-".join(rungs)
    (stage / "VERSION.txt").write_text("%s.%s.%s\n" % (version, commit, label), encoding="utf-8")
    print("staged vkstorm-ffi %s.%s.%s into %s" % (version, commit, label, stage))


def host_triple():
    out = subprocess.run(["rustc", "-vV"], cwd=ROOT, check=True, stdout=subprocess.PIPE,
                         universal_newlines=True).stdout
    return re.search(r"host: (\S+)", out).group(1)


def meta_version(meta):
    for package in meta["packages"]:
        if package["name"] == CRATE:
            return package["version"]
    sys.exit("vkstorm-ffi not in the workspace")


if __name__ == "__main__":
    main()
