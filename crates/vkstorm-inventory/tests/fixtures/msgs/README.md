# msgs fixtures

The agent-inventory UDP messages the viewer handles (`UpdateCreateInventoryItem`,
`BulkUpdateInventory` and the others), decoded into model updates and callbacks.

Filled by recorder R2 of the inventory rung (vkstorm-rust rung seven), through
the real inventory model, since the handlers live in `llinventorymodel.cpp`.
The fixtures were first recorded from the viewer C++ (`codex/rust-inventory`
at a6d6a04641), before any port commit, on Windows and Linux, and re-recorded
on Windows at 8d18dd15dd after the inventory bug fixes of PR #181 (bug 24
changed `update_create_inventory_item` and `bulk_update_udp`; see
`../cache/README.md`). They are never edited by hand. To change them, change the recorder and record them again.

## Files

`set8_handlers.txt` (survey 4 section 7, set 8), made by
`INTEGRATION_TEST_llinventorymodel_parity` (`tests/llinventorymodel_parity_msgs.h`);
how it is run, recorded and compared, the line format and the determinism
rules are in `../cache/README.md`.

Each case starts from the standard inventory (`llinventorymodel_parity_test.cpp`,
`standard_world()`), callback ids restarting at 1 and the window's busy count
at 5. Every step builds one message with the real template builder through
`gMessageSystem` (`in <name> <hex>`, the builder's bytes, not zero-coded),
reads it with the real template reader, which calls the handler
`LLInventoryModel::registerCallbacks()` registered, and records every journal
line (notification batches with the transaction id, callbacks fired, fetch
requests, marketplace and appearance calls, messages sent), checks the
step's expectations in code where it has them (a non-empty batch or none,
the number of callbacks, lines that must or must not occur), then
`callbacks_registered=[...]` (the ids registered with `gInventoryCallbacks`
that never fired) and `busy=` (the busy count after `decBusyCount()`), then the
model lines that changed.

| case | steps |
|---|---|
| `update_create_inventory_item` | new item with a callback, existing item moved (±1 accounting), same parent, two blocks (each block's callback fires and its item is re-fetched, bug 24), parent of unknown version, a localisable name, missing parent (Lost And Found and a server move), callback id 0, foreign agent |
| `bulk_update_udp` | new folder and item, existing folder same parent (version reset to unknown, survey 2 Q3), existing folder of unknown version (`fetch()`), moved folder, new folder under an unknown version (parent `fetch()`) and under a missing parent, three item blocks with three callback ids (each fires its own, bug 24; survey 4 Q17 recorded block 0's id for all before the fix), existing items, an item into a folder of unknown version whose fetch timer has expired (the parent `fetch()` is observed), a null item block (its callback stays registered; the next item fires its own), marketplace depths 0, 1, 2 and 3, the RLVa `#RLV/~` rename guard on and off, buy-and-wear across two packets of one transaction and a third transaction, foreign agent, an empty packet |
| `bulk_update_generic` | the dispatcher registered (as the first `requestPost` does); GenericMessage with a folder record, with parameters of 253, 254 (the limit) and 255 bytes, and with an item record (cut: nothing applied); LargeGenericMessage over UDP with parameters of 1,199, 1,200 (the limit) and 1,201 bytes, one item with integer flags (applied, its callback fires), two items (cut), raw binary flags (cut at the first NUL); LargeGenericMessage through the event queue (`LLMessageSystem::dispatch`, the LLSD reader): items before a folder they live in (folders applied first; per-item callback ids, both fire), raw and b16 binary flags, XML, an existing folder (version reset), an item into an unknown-version folder (no `fetch()`, unlike the UDP form), a folder keyed `folder_id` (not read: `category_id` is), a foreign agent in the second item (nothing applied, no callback), the wear rule without a transaction id (wears the shirt), six items (six callbacks), an unparsable parameter; an unknown method |
| `remove_messages` | RemoveInventoryItem (known, known, unknown), foreign agent, RemoveInventoryFolder with children, RemoveInventoryObjects (folders, then items) |
| `save_asset_and_move` | SaveAssetIntoInventory known, unknown, with RLVa, foreign agent; MoveInventoryItem with a new name, three blocks with an unknown item, only an unknown item, foreign agent |
| `udp_fetch_replies` | InventoryDescendents (folder blocks reset their versions, items already present are skipped, the parent's version and count set from the message), for an unknown parent, FetchInventoryReply (no accounting) |

The GenericMessage and LargeGenericMessage handlers of
`llviewergenericmessage.cpp` are frozen copies in
`tests/llinventorymodel_parity_generic.cpp` (checked by
`tests/check_r2_copies.py`).

Recorded on Windows (MSVC) and Linux (GCC 15 and 14): byte-identical apart
from the header lines.
