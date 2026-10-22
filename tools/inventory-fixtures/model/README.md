# inventory-fixtures/model

Regenerates `crates/vkstorm-inventory/tests/fixtures/model/` and the viewer's
`indra/llinventory/tests/fixtures/` (the same bytes) from the viewer's oracle
of the inventory data model (rung seven, recorder R1). The fixtures and their
format are described in `crates/vkstorm-inventory/tests/fixtures/model/README.md`.

The oracle is in the viewer repository: the frozen copies of the pre-port C++
in `indra/llinventory/tests/llinventory_oracle.{h,cpp}` (checked against the
source by `check_r1_copies.py`), and the corpora, dump and bench in
`indra/llinventory/tests/llinventory_tool.cpp`. It needs the viewer's `LLSD`,
`LLUUID`, `LLStringUtil` and message system.

```sh
# In the viewer checkout, configured with -DLL_TESTS:BOOL=ON (Windows example):
cmake --build build-vc170-64 --config RelWithDebInfo --target llinventory_tool -- -m:4
# Then, from this repository:
python tools/inventory-fixtures/model/regenerate.py <viewer>/build-vc170-64/sharedlibs/RelWithDebInfo/llinventory_tool.exe <viewer>
```

The script keeps the hand-written README and the Library cache
(`library/ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz`, committed
unchanged) and replaces everything else, then checks that the two copies have
the same bytes.

The same executable times the C++ baseline that the port must match or beat:
`llinventory_tool bench 3 --library <cache.gz> [--cache <private cache.gz>]`.
A private cache is used locally only. The tool prints its counts, its byte
identity and its timings, never its contents.
