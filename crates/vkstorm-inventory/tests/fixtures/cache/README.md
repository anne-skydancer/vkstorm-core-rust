# cache fixtures

The inventory model and its cache file (`LLInventoryModel`): cache files
written and read, cache load and reconciliation at login, mutation sequences
with every notification batch and outgoing message, and model validation.

Filled by recorder R2 of the inventory rung (vkstorm-rust rung seven). The
fixtures are recorded from the viewer C++ (`codex/rust-inventory` at
a6d6a04641, which includes the approved fixes LL #5358, FIRE-33455, the cache
save check and the `gunzip_file` fix), before any port commit, on Windows and
Linux, and are never edited by hand. To change them, change the recorder and
record them again.

## How they are made

`INTEGRATION_TEST_llinventorymodel_parity` (`indra/newview/tests/inventory/r2.cmake`)
compiles the real `llinventorymodel.cpp`, `llviewerinventory.cpp` and
`llinventoryobserver.cpp` against `tests/llinventorymodel_parity_stubs.*`
(every outward call journaled, see the header), on the real llinventory,
llmessage (with `scripts/messages/message_template.msg`), llcommon (gzip),
llfilesystem (`LLDir`), llcorehttp and llwindow (a headless window). The
corpora are in `tests/llinventorymodel_parity_{c1,c2,c3,c4,msgs,real}.h`.

- Replay (the default, also run after every build of the target): each corpus
  is produced again and compared line by line with the fixture; a difference
  is reported as `case <id> line <n> field <name>`, expected and actual.
- Record: `VKS_INVMODEL_DUMP=<dir>` writes the fixtures under `<dir>/cache`
  and `<dir>/msgs` instead (`VKS_INVMODEL_COMMIT=<hash>` goes into the
  headers). `tools/inventory-fixtures/cache/regenerate.py` in
  vkstorm-core-rust runs the executable this way and copies the result into
  the viewer and the crate (`crates/vkstorm-inventory/tests/fixtures/`).
- `VKS_INVMODEL_BENCH=<runs>` (and `VKS_INVMODEL_BENCH_STRESS=1`) print the
  timings; `VKS_INVMODEL_USER_CACHE=<path>` checks a real cache locally and
  prints counts and timings only. Neither writes a fixture.

## Files

| file | corpus | content |
|---|---|---|
| `c1_cache_files.txt`, `c1/*.inv.llsd`, `c1/*.notation.txt` | C1 | `cache()` at logout for model states made through the C++ API: `standard`, `types` (every asset, inventory and folder type, shadow_id, thumbnails, favourites, `script.runtime`, names with `|`, control bytes, LF, non-ASCII, empty and long names, high-bit flags, negative and zero dates, sale info, group-owned, links, broken links, a link to a link), `incomplete` (incomplete root, unknown version under a complete parent, a complete child of an incomplete parent), `history` (push_back and swap-remove orders), three seeded random worlds (`random_1001` in full, the other two as digests). Per file: the uncompressed bytes (binary fixture) and their notation rendering, the decode by `loadFromFile` (every field of every record), the re-encode by `saveToFile`, the files in the cache directory after `cache()`. |
| `c1_realistic.txt`, `c1/realistic_100k.inv.llsd.gz` | C1 | the realistic synthetic set: 8,000 folders with a depth distribution, 100,000 items, a 20,000-item flat Objects folder, a Trash of 10,000 items and 400 folders, 200 outfit folders of links, a COF of 40 links. Size and SHA-256 of the 57.8 MB cache file (committed gzipped), the decode digest, the re-encode identity, `validate()`, and a login from that cache (digests and validation). |
| `c2_load.txt`, `c2/<case>/` | C2 | 60 login load and discard cases. Inputs per case: `case.txt` (flags), `skel_agent.llsd` and `skel_library.llsd` (the login response's skeletons, notation, ids as strings as XML-RPC sends them), `files/` (the cache directory before login). `c2/cases.txt` lists the cases in order. Output per case: the files before and after each step, `loadSkeleton()`'s return value and journal for the library and the agent, the model after each (objects new or changed), `buildParentChildMap()` with its outgoing messages, the child vectors in order, `validate()`'s result, the usable flag, root and `sFirstTimeInViewer2`, `createCommonSystemCategories()`, the startup notify, and for `logout` cases the logout save (a frozen copy of `LLAppViewer::disconnectViewer`'s inventory part) and the saved files' digests. |
| `c3_mutations.txt` | C3 | 17 scripted sequences through the public API (`updateItem`, `updateCategory`, `moveObject`, `change*Parent` with the FS locked folders, `deleteObject`, `onObjectDeletedFromServer`, `onDescendentsPurgedFromServer` on a 30-deep chain, `accountForUpdate` in all overloads, broken links and their rebuild, notify re-entry and the backlog, remove to Trash and the overflow check, `createNewCategory` through AIS, system folders, `onItemUpdated`/`onCategoryUpdated`, the server write helpers of `llviewerinventory.cpp`, folder fetch and fetch states, and the lookups). After each step: every journal line in order, then the model lines that changed. |
| `c3_random.txt` | C3 | 40 seeded random sequences of 40 mutations each: the first two in full, all 40 as digests of their complete recorded stream. |
| `c4_validation.txt` | C4 | `validate()` over 37 constructed states: loops, orphans, wrong parents, missing and duplicate system folders (fatal and not), library exemption, unknown counts, locks, tree and map mismatches, missing backlinks, broken links, links to links, the root-name hack. Per case: `asLLSD` as binary-LLSD hex (no header) and notation, every field, every warning key, `mLog`, the model summary. |
| `c4_validation_qa.msvc.txt`, `c4_validation_qa.libstdcxx.txt` | C4 | `QAModeFakeSystemFolderIssues` (see runtime edges). |
| `library_real.txt` | real data | the public Linden Library cache, read from `indra/llinventory/tests/fixtures/library/` (committed by R1; the test fails, not skips, when it is missing): the decode (counts per type, record digest), the uncompressed re-encode identity, and its load as a library skeleton next to the standard agent inventory. |

The `msgs/` directory beside this one holds survey 4 set 8 (the inventory UDP
message handlers), recorded through the same harness.

## Line format

Every text fixture: `#` header lines (generator, viewer commit, platform,
seed; not compared), then `== <case>` sections of lines. Ids are lower-case
hyphenated UUIDs. Strings are quoted with `\\`, `\"` and `\xNN` (bytes below
0x20 and above 0x7e) escaped.

- `step <label>`: a step of the case; the lines after it up to the next
  `step` belong to it. Journal lines are indented by two spaces and keep
  call order.
- Journal lines (see `llinventorymodel_parity_stubs.h`): `msg <name> <hex>`
  (an outgoing message, `LLTemplateMessageBuilder::buildMessage()` output:
  six zero header bytes, the message number, the blocks; not zero-coded),
  `in <name> <hex>` (a message given to the viewer), `batch mask=<hex>
  tid=<id|-> changed=[...] added=[...]` (one `notifyObservers()` delivery:
  the union mask, the transaction id, the changed and added ids in UUID
  order), `callback <tag> <item>`, `ais <call> <id> [<notation body>]`,
  `fetch.start|item|folder ...`, `fetch.completion_callback`,
  `market.update|listing_id|get_listing ...`, `appearance.wear ...`,
  `gesture.activate|deactivate`, `preview.hide`, `notify <name> [args]`,
  `observer.start_new_inventory`, `rlv.saved_asset`, `trash_preview.show`,
  `create callback <id>`, and lines a case notes itself (`returned mask=`,
  query results).
- Model lines: `model root= library_root= library_owner= usable= cats=
  items= cat_trees= item_trees=`; `cat <id> parent= pref= atype= name=
  owner= version= count= thumb= fav= fetching= timer{started= expired=
  to_expire=}`; `item <id> parent= type= inv= flags= asset= name= desc=
  created= thumb= fav= runtime= perms=<creator,owner,last owner,group,base,
  owner,group,everyone,next owner,group owned> sale=<type,price> complete=
  txn= running= faulted=` (the stored fields, not the link-resolved getters);
  `children <parent> cats=[...] items=[...]` (the child vectors in order;
  pairs that are both empty and both present are omitted); `backlink
  <target> <link>`; `broken <target> [links]`; `rebuild [ids]`; `locks
  cat_entries= item_entries=` and `lock cat|item <id>` for set locks;
  `pending mask= changed= added= backlog_mask= backlog_changed=
  backlog_added=`. Within a case, after the first dump, keyed lines (`cat`,
  `item`, `children`, `broken`, `backlink`) appear only when new or changed,
  and `gone <key>` when removed.
- `validation llsd=<hex>`, `validation notation=`, `validation fatal= ...`,
  `validation warning <key> <count>`, `validation log=<quoted>`.

## Determinism

- Ids: `pid(tag, n)` puts the tag in bytes 0-3, 0x40 in byte 6, 0x80 in byte
  8 and n big-endian in bytes 10-15 (`f0000000-...` agent folders,
  `a0000000-...` items, `a5000000-...` assets, `f1000000-...`/`a1000000-...`
  library, `a9e00000-...-000000000001` the agent, `11b00000-...` the library
  owner). Random ids and values come from xorshift64* (`s ^= s>>12; s ^=
  s<<25; s ^= s>>27; return s*0x2545F4914F6CDD1D`) seeded as stated in the
  headers. The realistic set uses `pid(tag, (u32)(counter*0x9E3779B1))` so
  that its UUID order is unrelated to creation order while staying
  compressible.
- No wall clock: `LLFrameTimer`'s frame time never advances in the test, so
  every timer state is a function of the calls made.
- Observers are called in pointer order; the test's observers live in fixed
  slots of one static block, so the order is the same in every run.
- Stub answers: Second Life main grid, AIS available, no agent region (so
  `fetch()` goes to the background fetch with AIS and to UDP
  `FetchInventoryDescendents` without), folder fetch inactive unless a case
  says so, `LLTrans::getString` returns its key, the avatar name cache is not
  running unless a case starts it, `start_new_inventory_observer()` only
  journals (the global new-inventory observers of `llviewermessage.cpp` are
  not run).
- The gzip inputs of C2 are made with the viewer's `gzip_file` (zlib-ng,
  default level) and their header OS byte is set to 3; the `.gz` bytes the
  viewer writes are never compared (only their uncompressed content).

## Runtime edges and platform differences

Recorded on Windows (MSVC 17.14, RelWithDebInfo) and Linux (WSL Ubuntu 26.04,
GCC 15.2 and GCC 14.3, CI's configure). Every output is byte-identical on all
three except:

- `QAModeFakeSystemFolderIssues`: `validate()` draws from a function-static
  `std::default_random_engine`, which is `mt19937` in MSVC's library and
  `minstd_rand0` in libstdc++. The draws are recorded per library
  (`c4_validation_qa.<msvc|libstdcxx>.txt`); a port must state which it
  reproduces. The engine's state also carries over between calls in a
  process, so the cases run in a fixed order in one process.
- The header lines (`# platform:`) differ by design.

## Behaviour recorded as it is (not fixed)

- `deleteObject` notifies once mid-delete even when told not to (survey 2
  Q4): C3 `delete_object/item_no_notify_q4`, `deleted_from_server/ais_style_flags`.
- A `BulkUpdateInventory` folder block resets the folder's version and count
  to unknown (Q3): msgs `bulk_update_udp/existing_folder_same_parent_resets_version_q3`
  (no folder fetch is ever scheduled), and the GenericMessage form.
- The incomplete-root reload (Q8): C2 `incomplete_root_reload` (the root,
  saved with its version but without its items, reloads with its count set
  from the cached children, so it reads as complete).
- The clear-cache marker and the library (Q9): C2 `marker_clear_cache` (the
  library is loaded from its cache before the agent call deletes both files;
  the logout save writes the library back).
- Unknown asset types invalidate their folder at every login (Q10): C1
  `types`, C2 `item_unknown_type`, `item_bad_lookup_type`, `types_world`.
- A corrupt or truncated `.gz`: `gunzip_file` now returns false, nothing is
  loaded, and the `.gz` is **kept** (not obsolete, because the plain file was
  never opened): C2 `gz_truncated`, `gz_corrupt_crc`, `gz_corrupt_data`. A
  stale plain file is then used if present (`plain_and_corrupt_gz`, Q7). A
  0-byte `.gz` inflates to an empty file, which is obsolete and deleted.
  Concatenated members, plain bytes under the `.gz` name and trailing
  garbage after a member all load the whole cache.
- A non-array `categories` (or a non-map root) makes every folder unknown and
  drops every item, without deleting the file.
- Lost folders at login are reparented with one `MoveInventoryFolder` each;
  a typed folder goes to the root, where a second Trash makes validation
  fatal and the inventory unusable (C2 `skeleton_parent_absent`); a second
  FT_ROOT_INVENTORY folder named "My Inventory" (any case) replaces the root
  id and also ends unusable (`skeleton_second_root`).
- `updateItem` of a new item with type AT_NONE puts it in its parent's child
  vector although `addItem` refused it (C3 `update_item_new/type_none`).
- Changing a link's target leaves its backlink stale (Q13): C3
  `update_item_existing/link_retarget_stale_backlink`.
- In a multi-block UDP `BulkUpdateInventory`, every item carries block 0's
  callback id, so only the first item's callback fires and the others stay
  registered (msgs `items_multi_block_callback_block0`, `null_item_block`:
  the callback fires for the item after a null block).
- `moveObject()` accepts a move that makes a folder cycle, after which
  `isObjectDescendentOf()` never returns; the random sweep skips such moves
  (they hang the C++).

## Not covered

- OpenSim paths (`OPENSIM` is off in the Release configuration): the
  null-parent folder rule, suitcase skipping inside `validate`, the OpenSim
  validation notification, the UDP fallbacks gated on `isInOpenSim`.
- Crashes in the C++ are not driven: `wearable_item` null dereference in both
  BulkUpdate paths when a wearable was refused, `consolidateForType` with
  missing arrays, `getFirstDescendantOf` on an unknown id, the
  `onItemUpdated`/`onCategoryUpdated` `LL_ERRS` on unknown keys, and locked
  arrays (`llassert_always`).
- The HTTP paths (`FetchItemHttpHandler`, `createNewCategoryCoro`) belong to
  R3b.

## Notes for the port

- The cache's folder records use the key `cat_id` (survey 2 section 2.2
  writes `folder_id`); the login skeleton uses `folder_id`; the
  GenericMessage folder records are read through `category_id`.
