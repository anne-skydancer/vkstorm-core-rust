# inventory-fixtures/task

Regenerates `crates/vkstorm-inventory/tests/fixtures/task/` (and, with
`--viewer`, the viewer's `indra/newview/tests/fixtures/inventory/task/`) from
the viewer's C++ oracle for object contents (task inventory), recorder R4a of
the inventory rung. What the fixtures hold, how the oracle is built and every
recorded quirk and runtime edge are described in the fixture directory's
`README.md`.

The oracle lives in the viewer repository
(`indra/newview/tests/llinventory_task_oracle*.{h,cpp}`, registered by
`indra/newview/tests/inventory/r4-task.cmake`) because it needs the viewer's
llinventory, llmessage (message template, xfer manager) and llfilesystem.

```sh
# In the viewer checkout, configured with -DLL_TESTS:BOOL=ON:
cmake --build <build> --config RelWithDebInfo --target INTEGRATION_TEST_llinventory_task_oracle
# Then, from this repository (Windows):
python tools/inventory-fixtures/task/regenerate.py <build>/sharedlibs/RelWithDebInfo/INTEGRATION_TEST_llinventory_task_oracle.exe --viewer <viewer checkout>
# Linux: add --libpath <build>/sharedlibs (and its lib/) if the libraries are not found.
```

The driver runs the executable with `VKS_TASK_DUMP=<scratch>` (the scratch
directory is created under this repository's ignored `target/`, not the
system temporary directory) and replaces both fixture trees, keeping their
`README.md`. Building the test target also replays the committed fixtures.
