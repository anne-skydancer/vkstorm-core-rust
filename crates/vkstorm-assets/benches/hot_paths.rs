//! The crate-side timing harness for the hot paths the viewer's integration
//! tests also time: unpacking a mesh LOD, assembling a texture from range
//! replies through the fetch state machine, a cache index lookup and write.
//! Run with `cargo bench --bench hot_paths`; filled in with the fixtures.

fn main() {
    println!("hot_paths: see tests/ for the fixture-driven timings");
}
