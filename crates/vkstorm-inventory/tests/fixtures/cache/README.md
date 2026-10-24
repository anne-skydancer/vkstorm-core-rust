# cache fixtures

The inventory model and its cache file (`LLInventoryModel`): cache files
written and read, cache load and reconciliation at login, cache write
safety, mutation sequences with every notification batch and outgoing
message, and model validation.

Filled by recorder R2 of the inventory rung (vkstorm-rust rung seven). The
fixtures were first recorded from the viewer C++ (`codex/rust-inventory` at
a6d6a04641, which includes the approved fixes LL #5358, FIRE-33455, the cache
save check and the `gunzip_file` fix), before any port commit, on Windows and
Linux, and are never edited by hand. To change them, change the recorder and
record them again.

Re-recorded on Windows at 8d18dd15dd (`codex/rust-inventory-r2` with
`vkstorm-rust` merged), after the inventory bug fixes of PR #181 and bug 52.
They changed `c2_load.txt` (bugs 20, 21, 27 and the gunzip follow-up
dabe25973f), `c3_mutations.txt` (bugs 23 and 52), `c4_validation.txt`
(bug 21) and `msgs/set8_handlers.txt` (bug 24); every other Windows record
changed only in its header. The Linux-only records
(`c4_validation_qa.libstdcxx.txt`, `c5_write_safety.linux.txt`) were not
re-recorded: none of these fixes touches `validate()`'s QA draws or the
cache save.

## How they are made

`INTEGRATION_TEST_llinventorymodel_parity` (`indra/newview/tests/inventory/r2.cmake`)
compiles the real `llinventorymodel.cpp`, `llviewerinventory.cpp`,
`llinventoryobserver.cpp` and `llviewerfoldertype.cpp` against
`tests/llinventorymodel_parity_stubs.*` (every outward call journaled, see the
header), on the real llinventory, llmessage (with
`scripts/messages/message_template.msg`), llcommon (gzip), llfilesystem
(`LLDir`), llcorehttp and llwindow (a headless window). The corpora are in
`tests/llinventorymodel_parity_{c1,c2,c3,c4,c5,msgs,real}.h`; the closing
checks in `tests/llinventorymodel_parity_final.h`.

- Replay (the default, also run after every build of the target): each corpus
  is produced again and compared line by line with the fixture; a difference
  is reported as `case <id> line <n> field <name>`, expected and actual.
  Steps of set 8 also check their expectations in code (a non-empty batch or
  none, the callbacks fired, journal lines that must or must not occur).
- The closing test fails when a corpus test did not run (unless the
  developer flag `VKS_INVMODEL_PARTIAL=1` is set), when the fixture tree holds
  a file no corpus read or wrote (beyond the READMEs, `.gitattributes` and
  the other platform's records), and when `tests/check_r2_copies.py` fails
  (it runs inside the test). A missing fixture fails in its corpus.
- Record: `VKS_INVMODEL_DUMP=<dir>` writes the fixtures under `<dir>/cache`
  and `<dir>/msgs` instead (`VKS_INVMODEL_COMMIT=<hash>` goes into the
  headers), writes `<dir>/DUMP_COMPLETE` (the list of files) and then
  **fails on purpose**: a record run never passes. On Linux the failing
  post-build step makes make delete the executable; the next build relinks
  it. `tools/inventory-fixtures/cache/regenerate.py` in vkstorm-core-rust
  records twice into fresh directories, requires both to be complete and
  identical, and only then replaces the viewer and crate trees.
- Each process works in `<build>/newview/llinventorymodel_parity_work/<pid>`
  and removes it at the end, so concurrent runs share nothing.
- `VKS_INVMODEL_BENCH=<runs>` (and `VKS_INVMODEL_BENCH_STRESS=1`) print the
  timings; `VKS_INVMODEL_USER_CACHE=<path>` checks a real cache locally and
  prints counts and timings only. Neither writes a fixture.
  `VKS_INVMODEL_TRACE=1` prints progress on stderr.

## Files

| file | corpus | content |
|---|---|---|
| `c1_cache_files.txt`, `c1/*.inv.llsd`, `c1/*.notation.txt` | C1 | `cache()` at logout for model states made through the C++ API: `standard`, `types` (every asset, inventory and folder type, the shadow_id rule at its edges, thumbnails, favourites, `script.runtime`, names with `\|`, control bytes, LF, non-ASCII, empty and long names, high-bit flags, negative and zero dates, sale info, group-owned, owner mask differing from base, links, broken links, a link to a link), `incomplete` (incomplete root, unknown version under a complete parent, a complete child of an incomplete parent), `history` (push_back and swap-remove orders), three seeded random worlds (`random_1001` in full, the other two as named digests). Agent and library folders carry thumbnails and favourites. Per file: the uncompressed bytes (binary fixture) and their notation rendering, the decode by `loadFromFile` (every field of every record), the re-encode by `saveToFile`, the files in the cache directory after `cache()`. |
| `c1_realistic.txt`, `c1/realistic_100k.inv.llsd.gz` | C1 | the realistic synthetic set: 8,000 folders with a depth distribution, 100,000 items, a 20,000-item flat Objects folder, a Trash of 10,000 items and 400 folders, 200 outfit folders of 10 links each, a COF of 40 links. Size and SHA-256 of the 57.8 MB cache file, the decode digest in chunks of 10,000 records, the re-encode identity, `validate()`, and a login from that cache (digests in chunks of 10,000 lines, validation). The `.gz` is an input for the crate: the replay compares its **decompressed** SHA-256, never its bytes. |
| `c2_load.txt`, `c2/<case>/` | C2 | 64 login load and discard cases (the list and its SHA-256 are pinned in `llinventorymodel_parity_c2.h`). Inputs per case: `case.txt` (flags), `skel_agent.llsd` and `skel_library.llsd` (the login response's skeletons, notation, ids as strings as XML-RPC sends them), `files/` (the cache directory before login). `c2/cases.txt` lists the cases in order. Output per case: the files before and after each step, `loadSkeleton()`'s return value and journal for the library and the agent, the model after each (objects new or changed), `buildParentChildMap()` with its outgoing messages, the child vectors in order, `validate()`'s result, the usable flag, root and `sFirstTimeInViewer2`, `createCommonSystemCategories()`, the startup notify, for `logout` cases the logout save (a frozen copy of `LLAppViewer::disconnectViewer`'s inventory part) and the saved files' digests, and the content digest of every plain file left in the cache directory. |
| `c3_mutations.txt` | C3 | 26 sequences through the public API: `updateItem`, `updateCategory`, `moveObject`, `change*Parent` with the FS locked folders, `deleteObject`, `onObjectDeletedFromServer`, `onDescendentsPurgedFromServer` on a 30-deep chain, `accountForUpdate` in all overloads, broken links and their rebuild, notify re-entry and the backlog, remove to Trash, the Trash capacity at limit −1, limit and +1, `createNewCategory` through AIS, system folders, `onItemUpdated`/`onCategoryUpdated`, the server write helpers of `llviewerinventory.cpp`, folder fetch and fetch states, `LLInventoryAddedObserver` and `LLInventoryFetchDescendentsObserver`, the lookups, and lost items at login (1, 36, 37, 38, 74, 75 and 200 items: a `MoveInventoryItem` packet closes after its 37th block). After each step: every journal line in order, then the model lines that changed. |
| `c3_random.txt` | C3 | 40 seeded random sequences of 40 mutations each: the first two in full, all 40 as named digests of their complete recorded stream. |
| `c4_validation.txt` | C4 | `validate()` over 40 constructed states: loops, orphans, wrong parents, missing and duplicate system folders (fatal and not), library exemption, unknown counts, locks, tree and map mismatches, missing backlinks, broken links, links to links, the root-name hack with one extra root and with two (in one vector and under different parents), a 20,005-item vector whose last entry has the wrong parent. Per case: `asLLSD` as binary-LLSD hex (no header) and notation, every field, every warning key, `mLog`, the model summary (child vectors over 4,000 characters by length and digest). |
| `c4_validation_qa.msvc.txt`, `c4_validation_qa.libstdcxx.txt` | C4 | `QAModeFakeSystemFolderIssues` (see runtime edges). |
| `c5_write_safety.txt`, `c5_write_safety.linux.txt` | C5 | the cache save at logout: normal, over a stale `<cache>.gz.t` file, with `<cache>.gz.t` uncreatable (a directory: gzip fails, the old `.gz` stays byte-identical, the temp file stays), with the temp directory missing (saveToFile fails before writing), and, Linux only, failing part-way through the temp file (`RLIMIT_FSIZE`; Windows has no portable way to fail a write part-way). |
| `library_real.txt` | real data | the public Linden Library cache, read from `indra/llinventory/tests/fixtures/library/` (committed by R1; the test fails, not skips, when it is missing): the decode (counts per type, record digest in chunks of 1,000), the uncompressed re-encode identity, and its load as a library skeleton next to the standard agent inventory (model digest in chunks of 1,000 lines). |

The `msgs/` directory beside this one holds survey 4 set 8 (the inventory UDP
message handlers), recorded through the same harness.

## Line format

Every text fixture: `#` header lines (generator, viewer commit, platform,
seed; not compared), then `== <case>` sections of lines. Ids are lower-case
hyphenated UUIDs. Strings are quoted with `\\`, `\"` and `\xNN` (bytes below
0x20 and above 0x7e) escaped. LLSD in journal lines and notation files is
written with binary as `b16"..."`.

- `step <label>`: a step of the case; the lines after it up to the next
  `step` belong to it. Journal lines are indented by two spaces and keep
  call order.
- Journal lines (see `llinventorymodel_parity_stubs.h`): `msg <name> <hex>`
  (an outgoing message, `LLTemplateMessageBuilder::buildMessage()` output:
  six zero header bytes, the message number, the blocks; not zero-coded),
  `in <name> <hex>` (a message given to the viewer), `in.event_queue <name>
  <binary LLSD hex>` (a message given through the event queue), `batch
  mask=<hex> tid=<id|-> changed=[...] added=[...]` (one `notifyObservers()`
  delivery: the union mask, the transaction id, the changed and added ids in
  UUID order), `callback <tag> <item>`, `ais <call> <id> [<notation body>]`,
  `fetch.start|item|folder ...`, `fetch.completion_callback`,
  `market.update|listing_id|get_listing ...`, `appearance.wear ...`,
  `gesture.activate|deactivate`, `preview.hide`, `notify <name> [args]`,
  `observer.start_new_inventory`, `rlv.saved_asset`, `trash_preview.show`,
  `create callback <id>`, `added_observer done`, `descendents_observer done`,
  and lines a case notes itself (`returned mask=`, query results,
  `move_messages=`).
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
- `files <label> [name size; ...]`: a directory listing (`.gz` sizes are
  written as `gz`; random temp names as `<uuid>.tmp`). `content <name>
  sha256=`: a plain file's digest.
- `validation llsd=<hex>`, `validation notation=`, `validation fatal= ...`,
  `validation warning <key> <count>`, `validation log=<quoted>`.
- Digests: `<label>[<first>..<last>] sha256=` is the SHA-256 of lines
  `<first>` to `<last>` (0-based) of that list, each followed by `\n`.

## Determinism

- Ids: `pid(tag, n)` puts the 32-bit tag in bytes 0-3 (big-endian), 0x40 in
  byte 6, 0x80 in byte 8 and n big-endian in bytes 10-15, all other bytes 0
  (`f0000000-...` agent folders, `a0000000-...` items, `a5000000-...` assets,
  `f1000000-...`/`a1000000-...` library, `a9e00000-...-000000000001` the agent,
  `11b00000-...` the library owner).
- The generator is xorshift64*: `s ^= s>>12; s ^= s<<25; s ^= s>>27; return
  s*0x2545F4914F6CDD1D` (64-bit). `below(n)` is `(next()>>11) % n`. A random
  UUID takes two draws, written little-endian into bytes 0-7 and 8-15, then
  byte 6 becomes `(b6 & 0x0F) | 0x40` and byte 8 `(b8 & 0x3F) | 0x80`. Every
  draw is its own statement, so the draw order is the program order.
- `random_world(seed, nfolders, nitems)` (C1, C3 random): start from the
  standard world; the folder pool is its agent folders in list order; for
  each folder i: id = uuid, parent = pool[below(|pool|)], type = FT_OUTFIT if
  below(10)==0 else FT_NONE, version = 1+below(50), name "folder i", appended
  to the pool. For each item i: id = uuid, parent = pool[below(|pool|)],
  r = below(100); if r<8 and there are earlier items: target = (r==0 ? uuid
  : earlier_items[below(n)]), a link "link i"; otherwise kind =
  kinds[below(12)] (texture, object, notecard, clothing, bodypart, lsl,
  landmark, sound, animation, gesture, settings, material, in that order),
  base = (below(4)==0 ? copy|move|transfer : all), asset = uuid, flags =
  below(16), created = 1600000000+below(100000000), then a thumbnail = uuid
  if below(20)==0 and favourite if below(25)==0; the item joins the earlier
  items.
- `realistic_world(seed, 8000, 100000, 20000, 10000)` follows
  `llinventorymodel_parity_real.h` with the same rules; its ids are
  `pid(tag, (u32)(counter*0x9E3779B1))` per kind of id (folders f2000000,
  items a2000000, assets a6000000, thumbnails 7c000000), so the UUID order is
  unrelated to creation order while staying compressible. Its full bytes are
  committed (gzipped), so no regenerator is needed for it.
- No wall clock: `LLFrameTimer`'s frame time never advances in the test, so
  every timer state is a function of the calls made.
- Observers are called in pointer order; the test's observers live in fixed
  slots of one static block, so the order is the same in every run.
- Stub answers: Second Life main grid, AIS available, no agent region (so
  `fetch()` goes to the background fetch with AIS and to UDP
  `FetchInventoryDescendents` without), folder fetch inactive unless a case
  says so, the avatar name cache not running unless a case starts it,
  `start_new_inventory_observer()` only journals (the global new-inventory
  observers of `llviewermessage.cpp` are not run). `LLTrans::getString`
  returns its key except for five dictionary keys set before the
  localisation dictionary is built ("New Folder" → "Nouveau dossier", "New
  Script" → "Nouveau script", "New Note" → "Nouvelle note", "Contents" →
  "Contenu", "New Shirt" → "Nouvelle chemise"), so localisation shows in the
  records. New folder names for AIS creations come from the real
  `LLViewerFolderType::lookupNewCategoryName` (`llviewerfoldertype.cpp` is
  linked).
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
- `c5_write_safety.linux.txt` exists only for Linux (see C5).
- The header lines (`# platform:`) differ by design.

## Behaviour recorded as it is

Entries naming a bug describe the C++ after that fix (PR #181, bug 52);
the others are recorded unfixed.

- `deleteObject` notifies once mid-delete even when told not to (survey 2
  Q4): C3 `delete_object/item_no_notify_q4`, `deleted_from_server/ais_style_flags`.
- A `BulkUpdateInventory` folder block resets the folder's version and count
  to unknown (Q3): msgs `bulk_update_udp/existing_folder_same_parent_resets_version_q3`
  (no folder fetch is ever scheduled), and the GenericMessage form.
- The incomplete-root reload (Q8): C2 `incomplete_root_reload` (the root,
  saved with its version but without its items, reloads with its count set
  from the cached children, so it reads as complete).
- The clear-cache marker and the library (Q9, fixed as bug 27): the marker
  is named after the agent and is looked for on every `loadSkeleton()` call,
  so the library's call, the first of the login, removes the agent and
  library cache files before anything is read: C2 `marker_clear_cache` (the
  library loads with no cache; the logout save writes an empty library).
  A marker named after the library owner, which the viewer never writes, is
  left alone and both caches load (`marker_library_owner`).
- Unknown asset types invalidate their folder at every login (Q10): C1
  `types`, C2 `item_unknown_type`, `item_bad_lookup_type`, `types_world`.
- A corrupt or truncated `.gz` (bug 20): `gunzip_file` returns false,
  nothing is loaded, every folder is unknown, and the cache is obsolete, so
  the `.gz` is removed: C2 `gz_truncated`, `gz_corrupt_crc`,
  `gz_corrupt_data`. A stale plain file under the same name is neither
  loaded nor kept (`plain_and_corrupt_gz`, Q7; its removal is the follow-up
  dabe25973f). A 0-byte `.gz` inflates to an empty file, which is obsolete
  and deleted.
  Concatenated members, plain bytes under the `.gz` name and trailing
  garbage after a member all load the whole cache.
- A non-array `categories` (or a non-map root) makes every folder unknown and
  drops every item, without deleting the file.
- Lost folders at login are reparented with one `MoveInventoryFolder` each;
  a typed folder goes to the root, where a second Trash makes validation
  fatal and the inventory unusable (C2 `skeleton_parent_absent`). A second
  FT_ROOT_INVENTORY folder named "My Inventory" (any case) no longer replaces
  the login's root (bug 21: the root-name hack runs only when the login root
  is not a root inventory folder); validation reports a non-fatal duplicate
  root and the inventory stays usable (`skeleton_second_root`,
  `skeleton_three_roots`; C4 `two_roots_named_my_inventory`,
  `three_roots_named_my_inventory`, `three_roots_different_parents`).
- `updateItem` of a new item with type AT_NONE, which `addItem` refuses, is
  no longer listed in its parent's child vector nor reported (bug 23; C3
  `update_item_new/type_none` returns mask 0x0).
- Changing a link's target leaves its backlink stale (Q13): C3
  `update_item_existing/link_retarget_stale_backlink`.
- In a multi-block UDP `BulkUpdateInventory` or `UpdateCreateInventoryItem`,
  each item block fires its own callback (bug 24; msgs `bulk_update_udp`
  `items_multi_block_callback_block0`, where all three fire, and
  `null_item_block`, where the null block's callback stays registered and
  the next item fires its own; `update_create_inventory_item`
  `two_blocks_block0_fires`, where both fire and both items are re-fetched).
- The GenericMessage form of `BulkUpdateInventory` (msgs
  `bulk_update_generic`): a GenericMessage parameter holds at most 254 bytes
  of text, so it carries folder records but never an item record; over UDP,
  LargeGenericMessage keeps at most 1,200 bytes of text and stops at the
  first NUL, so one item record with integer flags fits and two do not;
  through the event queue the whole parameter arrives. The parser accepts
  notation (raw or b16 binary) and XML. What simulators actually send cannot
  be observed without a login; the recorded carriers are the ones the
  viewer's parser accepts.
- `moveObject()` refuses a move of a folder into its own descendant, and
  `isObjectDescendentOf()` stops on a loop (bug 19). The random sweep still
  skips such moves, so no record shows the refusal.

## Not covered

- OpenSim paths (`OPENSIM` is off in the Release configuration): the
  null-parent folder rule, suitcase skipping inside `validate`, the OpenSim
  validation notification, the UDP fallbacks gated on `isInOpenSim`.
- Crashes in the C++ are not driven: `wearable_item` null dereference in both
  BulkUpdate paths when a wearable was refused, `consolidateForType` with
  missing arrays, `getFirstDescendantOf` on an unknown id, the
  `onItemUpdated`/`onCategoryUpdated` `LL_ERRS` on unknown keys, and locked
  arrays (`llassert_always`).
- Survey 2 risk 10 (thread misuse) has no case: the model is single-threaded
  and the test drives it from one thread.
- The HTTP paths (`FetchItemHttpHandler`, `createNewCategoryCoro`) belong to
  R3b.

## Notes for the port

- The cache's folder records use the key `cat_id` (survey 2 section 2.2
  writes `folder_id`); the login skeleton uses `folder_id`; the
  GenericMessage folder records are read through `category_id`.
