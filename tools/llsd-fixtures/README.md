# llsd-fixtures

Regenerates `crates/vkstorm-llsd/tests/fixtures/` from the viewer's C++
oracle: the LLSD parsers and formatters as they were before vkstorm-rust rung
two replaced them with `vkstorm-llsd`.

The oracle and the corpus live in the viewer repository
(`indra/llcommon/tests/llsdserialize_oracle.cpp`,
`indra/llcommon/tests/llsdserialize_corpus.cpp`), because they need the
viewer's `LLSD`, `LLDate`, `LLUUID`, APR and expat. The viewer's test build
produces the generator executable `llsdserialize_tool`; this directory holds
the driver that runs it and copies the result here.

```sh
# In the viewer checkout, after `cmake -S indra -B build-vc170-64 -DLL_TESTS:BOOL=ON`:
cmake --build build-vc170-64 --config RelWithDebInfo --target llsdserialize_tool -- -m:6 -nologo
# Then, from this repository:
python tools/llsd-fixtures/regenerate.py <viewer>/build-vc170-64/llcommon/RelWithDebInfo/llsdserialize_tool.exe
cargo test -p vkstorm-llsd --test fixtures
```

`manifest.txt` lists one case per line:

- `parse <name> <format> <max_bytes> <max_depth> <lines> <count>`: `<name>.in`
  is the input; `<count>` is what `LLSDParser::parse` (or `parseLines` when
  `lines` is 1) returned; for a positive count `<name>.out` is the parsed
  value in the C++ binary serialization.
- `format <name> <value> <format> <options> <boolalpha> <real_format> <count>`:
  `<value>.val` is the value in the binary serialization, `<options>` the
  `EFormatterOptions` bits, `<real_format>` the `realFormat` or `-`, and
  `<name>.expect` the bytes the C++ formatter wrote.

Formats are 0 notation, 1 binary, 2 XML. The fixtures were generated on
Windows (MSVC runtime); the formatter cases involving NaN depend on the C
runtime's spelling and are expected to hold on Windows only.
