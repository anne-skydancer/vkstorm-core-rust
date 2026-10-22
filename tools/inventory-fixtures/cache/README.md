# inventory-fixtures/cache

Regenerates `crates/vkstorm-inventory/tests/fixtures/cache/` and
`crates/vkstorm-inventory/tests/fixtures/msgs/` (recorder R2: the inventory
model and its cache, and the inventory UDP message handlers) from the viewer's
C++ oracle, `INTEGRATION_TEST_llinventorymodel_parity`, which compiles the
unchanged `llinventorymodel.cpp`, `llviewerinventory.cpp` and
`llinventoryobserver.cpp` against stubs.

```
# viewer (RelWithDebInfo, LL_TESTS ON), then:
python tools/inventory-fixtures/cache/regenerate.py \
    <viewer build>/sharedlibs/RelWithDebInfo/INTEGRATION_TEST_llinventorymodel_parity.exe \
    <viewer repository root>
```

On Linux the executable is `<build>/sharedlibs/bin/INTEGRATION_TEST_llinventorymodel_parity`.
The script runs it with `VKS_INVMODEL_DUMP`, replaces the generated files in
the viewer's `indra/newview/tests/fixtures/inventory/{cache,msgs}` and in the
crate, keeps the READMEs, the `.gitattributes` and the other platform's
QA-mode validation record, and checks that both copies are identical. The
fixture format, the cases and the runtime edges are described in
`crates/vkstorm-inventory/tests/fixtures/cache/README.md`.

The fixtures are recorded from the viewer before any port commit and are
never edited by hand.
