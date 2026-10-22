# inventory-fixtures/offer

Regenerates `crates/vkstorm-inventory/tests/fixtures/offer/` and `give/` (and
the viewer's identical copies) from recorder R4b's oracle:
`INTEGRATION_TEST_llinventory_offer_oracle` in the viewer
(`indra/newview/tests/llinventory_offer_oracle*.cpp`, registered by
`indra/newview/tests/inventory/r4-offer.cmake`), which runs verbatim copies of
the viewer's offer and give decision code against fakes and the real message
system.

```sh
# In the viewer worktree, configured with LL_TESTS ON (RelWithDebInfo, Release feature set):
cmake --build build-vc170-64 --config RelWithDebInfo --target INTEGRATION_TEST_llinventory_offer_oracle -- -m:4
# Then, from this repository:
python tools/inventory-fixtures/offer/regenerate.py \
    <viewer>/build-vc170-64/sharedlibs/RelWithDebInfo/INTEGRATION_TEST_llinventory_offer_oracle.exe <viewer>
```

On Linux the executable is under `build-linux-64/newview/` (the script sets
`LD_LIBRARY_PATH`). The dump takes about 25 s; the 589,824-case policy table
dominates. Format, digests, samples, the policy projection and the runtime
edges are described in the fixtures' `README.md` files.

For a Rust test: every non-header line is `<case>\t<key>\t<value>`; skip lines
whose key starts with `note.`. A digest file's `sha256` is over every
non-note line of the full stream followed by `\n`; the `.sample.txt` file holds
the cases with FNV-1a-32(case id) mod `sample_modulus` == 0.
