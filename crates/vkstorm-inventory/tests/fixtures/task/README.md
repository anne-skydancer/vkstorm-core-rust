# task fixtures

Object contents ("task inventory"): task inventory files and capability
replies parsed into records, the serial and request-state machine of
`LLViewerObject`, the local-update permission guess, and the
task-inventory messages the viewer sends. Recorded by R4a of the inventory
rung (vkstorm-rust rung seven) from the C++ before any port commit. Never
edited by hand: change the recorder and record again.

The same bytes live in the viewer (`indra/newview/tests/fixtures/inventory/task/`)
and in vkstorm-core-rust (`crates/vkstorm-inventory/tests/fixtures/task/`).
`.gitattributes` here marks every file `-text`: inputs contain CRLF, NUL
and bytes of 0x80 and above, and are compared byte for byte.

## How they are made

`INTEGRATION_TEST_llinventory_task_oracle`
(`indra/newview/tests/inventory/r4-task.cmake`) drives the task-inventory
decision code through one boundary (below). Its implementation today is the
frozen copies (`indra/newview/tests/llinventory_task_oracle.cpp`):
`llviewerobject.cpp` lines 155-160, 2823-3231, 3375-3978, 6252-6269,
7229-7235 and 8204-8285 (blob `c9ac3bdc20`), and the `LLViewerInventoryItem`
members they use from `llviewerinventory.cpp` lines 87-88, 104-233,
335-408 and 547-601 (blob `a13210dce6`). The copies are verbatim; they reach
fakes for the viewer singletons by ordinary name lookup in the namespace
`vks_task_oracle` (`llinventory_task_oracle.h`, `llinventory_task_oracle_fakes.cpp`).
The oracle's own `LLViewerObject` mirrors the live declarations and the
constructor and destructor lines, which are kept as checked text in
`llinventory_task_oracle_decls.h` (`llviewerobject.cpp` 303-308 and 370-375,
`llviewerobject.h` 548-578 and 972-997).

### The copy check

`indra/newview/tests/check_r4a_copies.py`, run by the test itself (test 7)
and standalone, checks every region of both files against the live source.
Both sides are normalised: comments removed, string and character literals
kept verbatim, whitespace outside literals collapsed within each line, empty
lines dropped; in the live file `#if 0` blocks are dropped too. A region
passes only if its lines occur exactly once, consecutively, in the live file.
So reindenting or rewording a comment passes, while a token or literal
change, a statement folded into a `//` comment, code moved under `#if 0` or
`/* */` with a changed version beside it, and a duplicated region all fail.
An identifier fence then requires every live line of `llviewerobject.cpp`
and `llviewerobject.h` that names `mInventory*`, `mInvRequest*`,
`mExpectedInventorySerialNum` or `mPendingInventoryItemsIDs` to lie inside a
checked region or on the script's (empty) allowlist. The check stays in
force until W5 retires the copies.

### The boundary (inputs in, actions out)

`indra/newview/tests/llinventory_task_oracle_boundary.h` declares
`vks_task_oracle::TaskInventoryBoundary`; the harness calls the decision code
only through it (`bnd()` in the test). `frozen_boundary()`
(`llinventory_task_oracle_boundary.cpp`) forwards each call to the frozen
copy of the `LLViewerObject` member of the same name. At W5 an implementation
calling the Rust decisions over FFI, plus the C++ that carries out their
actions, is installed with `set_boundary()` and replays these fixtures under
the same fakes.

Inputs, synchronous (each takes the object):
`registerInventoryListener`, `removeInventoryListener`, `requestInventory`,
`dirtyInventory`, `processTaskInv(msg)` (ReplyTaskInventory: TaskID, Serial,
Filename, read through the message getters), `loadTaskInvFile(filename)`,
`updateInventory(item, key, is_new)`, `updateInventoryLocal(item, key)`,
`updateViewerInventoryAsset(item, new_asset)`,
`updateMaterialInventory(item, key, is_new)`, `removeInventory(item_id)`,
`moveInventory(folder_id, item_id)`,
`saveScript(item, active, is_new, template_id)`,
`createInventoryItem(asset_type, inventory_type, sub_type, name, description, permissions, params, callback)`,
`deleteInventoryItem(item_id)`, and the read helpers `getInventoryRoot`,
`getInventoryContents`, `countInventoryContents(type)`,
`getInventoryItemByAsset(asset)` and `getInventoryItemByAsset(asset, type)`.

Inputs, asynchronous (delivered through the fakes to whatever the
implementation registered): an xfer completion (the callback and user data
handed to `gXferManager->requestFile`, with an error code); an HTTP response
(`complete_http`: body and status, to the coroutine suspended in
`getAndSuspend`/`postAndSuspend`); a timer (`fire_timer`, to the coroutine
suspended in `llcoro::suspendUntilTimeout`). The object's existence is an
input too (`gObjectList.findObject`).

Outputs: the object's task-inventory state, which stays C++
(`mInventorySerialNum`, `mExpectedInventorySerialNum`, `mInvRequestState`,
`mInvRequestXFerId`, `mInventoryDirty`, `mInventory` with every field of
every entry in list order, `mPendingInventoryItemsIDs`,
`mInventoryCallbacks`); the side effects in order, as the fakes record them
(`msg.send <message>` and in capture mode the datagram, `xfer.request`/
`xfer.requested`/`xfer.abort`, `coro.launch`, `http.get`/`http.post` with
URL and body, `timer.start`, `callback L<n>` with serial and entries,
`floater.hide`, the create-item callback); the cache files left behind; and
the return values of the calls.

### Real code and fakes

Real code is used wherever it links outside the viewer:

- llinventory: `LLInventoryObject`/`LLInventoryItem` legacy readers and
  writers, `fromLLSD`/`asLLSD`, `LLPermissions` (including
  `setOwnerAndGroup`), `LLSaleInfo`, `getCRC32`, `packMessage`;
- llmessage: `LLMessageSystem` built from `scripts/messages/message_template.msg`,
  its builder and reader, zero-coding, the circuit's UDP send path, and
  `LLXferManager`/`LLXfer_File` (`RequestXfer`, `AbortXfer`, the `.tmp` rule
  and the `substr` throw);
- llfilesystem: `LLDir::getScrubbedFileName`, `getExpandedFilename`, `LLFile`;
- llcommon: LLSD and its serializers; llcorehttp's `HttpStatus` and
  llmessage's `HttpCoroHandler::writeStatusCodes`/`getStatusFromLLSD`.

| fake | models |
|---|---|
| `gAgent` | agent `a0000000-...-0001`, session `...-0002`, active group `...-0003` |
| `gObjectList` | `findObject` over the objects of the case (events `die`/`revive` remove and restore the object) |
| `LLViewerRegion` | the `RequestTaskInventory` and `CreateTaskInventoryItem` capabilities (present or absent per case) and the host |
| `LLSelectMgr::findObjectPermissions` | NULL, or (event `select`) owner `other`, group `othergroup` |
| `permYouOwner` | true, or false after event `notowner` |
| `LLFloaterReg::hideInstance` | records the name and key |
| `LLTrans::getString` | returns `{L10N:<key>}`, so a localised name shows where `LLLocalizedInventoryItemsDictionary` applied and with which key |
| `LLCoros::launch`, `llcoro::suspendUntilTimeout`, `HttpCoroutineAdapter::getAndSuspend/postAndSuspend` | real boost fibers launched with `launch::dispatch` as `LLCoros` does: a coroutine runs at once to its first suspension; the test delivers HTTP results and fires timers |
| HTTP result | the body as `HttpCoroLLSDHandler` hands it on (made a map; a non-map moves under `content`; an empty body becomes an empty map; for errors an empty map, or the body for 4xx) plus `http_result` from `writeStatusCodes`; headers empty |
| `gXferManager` (sets 1 main, 3) | ids 1001, 1002, ... per case; `abortRequestById` marks the transfer aborted and it never completes (an in-progress receive whose AbortXfer the simulator does not echo) |
| `gXferManager` (scrub corpus, set 4) | the real `LLXferManager`; its ids are random and are recorded as `real#k` |
| `LLViewerInventoryItem` link overrides | not reproduced: with the oracle's empty agent model they return the item's own values, as the viewer does for a link whose target is not in `gInventory` |
| `LLMessageSystem::sendReliable` (copies only) | records `msg.send <name>`; in set 4 the send continues on the real UDP path to a loopback receiver |

Determinism: patterned UUIDs; xorshift64* for the two random sweeps (seeds
in the file headers, generators specified below); no wall clock (timers fire
on events); every process works in its own directory under the build tree
(`VKS_TASK_WORK_DIR/pid-<pid>`, removed at exit), never in the system
temporary directory.

Run, replay and regenerate:

```sh
# viewer, after configuring with LL_TESTS ON (builds, then runs the replay):
cmake --build <build> --config RelWithDebInfo --target INTEGRATION_TEST_llinventory_task_oracle
# record again into the crate and the viewer (from vkstorm-core-rust):
python tools/inventory-fixtures/task/regenerate.py <path to INTEGRATION_TEST_llinventory_task_oracle> --viewer <viewer checkout>
python indra/newview/tests/check_r4a_copies.py      # in the viewer (also run by the test)
```

The replay compares text files line by line, skipping `#` lines (the header
names the platform), binary inputs byte for byte, and the large sweeps by
the SHA-256 of their canonical text, whole and per block of 97 units. A
failure names the file, the content line, the case or trace it belongs to,
and the expected and actual lines; a digest failure names the block and its
unit range. Test 6 fails on any file missing from, or unexpected in, this
directory (README.md and .gitattributes included). A dump run
(`VKS_TASK_DUMP=<dir>`) writes the fixtures and always fails;
`regenerate.py` dumps twice, requires identical trees, installs, and
replays. `VKS_TASK_ONLY=<1-4>` runs one set and is honoured only with
`VKS_TASK_DEV=1` (otherwise a skipped set fails).

## Files

Every text file starts with a `#` header (generator, viewer commit and copied
blobs, platform, seed) and a legend. After the entries of any inventory, a
`helpers` line records the read helpers over it: `getInventoryRoot`,
`getInventoryContents`, `countInventoryContents` for every type present,
`getInventoryItemByAsset` for every item's asset (`byasset`) and with its
type (`bytype`). Those lookups cast entries of a matching type to
`LLViewerInventoryItem` without a check; where that would cast a plain
`LLInventoryObject` (an `inv_object` whose type is neither category nor
none) the call is not made and the line says `not-called`.

### Set 1: task inventory files (`loadTaskInvFile`)

- `task_files.txt`, inputs `files/<label>.inv`: per case, the input's size and
  SHA-256; `direct`: `loadTaskInvFile(<name>)`'s return value, whether the
  cache file was removed, the object state and the side effects (listener
  callbacks); then every entry of `mInventory` front to back (`push_front`
  reverses file order) with every field, the root (`back()`), and the
  helpers line. Field order: kind (item/object), uuid, parent, raw asset
  type, name, thumbnail, favourite, runtime, creation date; for items asset,
  description, raw inventory type, flags, sale type/price, the five masks,
  creator, owner, last owner, group, group-owned, `mIsComplete`, transaction
  id, running, faulted, CRC. Cases marked `via_xfer` also run the bytes
  through ReplyTaskInventory -> xfer -> `processTaskInvFile` with the pending
  list `[assetP, assetE, assetA]` and record the list afterwards.
  `missing_file` is the open failure. `toplevel_line_253` ... `_300` put a
  top-level line of that many characters between two blocks;
  `item_type_missing_shares_asset` has an item without a type line (AT_NONE)
  sharing A's asset; `duplicate_item_id` has two items with A's id.
- `scrub.txt`: Filename values through the real builder and reader,
  `LLDir::getScrubbedFileName`, and `processTaskInv` with the real
  `LLXferManager`: every forbidden character (`\ / : * ? " < > |`), `..`
  forms, names of 0-4 characters, control characters, 254/255/300 bytes. The
  outcome is `returned` or `threw std::out_of_range` (names of 1-3 bytes:
  `remote_filename.substr(length()-4)` in `requestFile`, caught by the test;
  in the viewer it is unhandled), the object state after it, whether a
  sentinel cache file of the scrubbed name survived (the `.tmp` rule removes
  it), and the RequestXfer datagram with every field (`FilePath` = 4,
  `DeleteOnCompletion` = 1, `UseBigPackets` = 0, `VFileID` null,
  `VFileType` -1).
- `task_files_random_samples.txt`, inputs `random/random_NNNN.inv`: a seeded
  sweep of 3000 mutated files (generator below); every 97th case (0, 97, ...)
  is kept with its input; `digests_set1.txt` holds the SHA-256 of the
  concatenated case text of all 3000 and of each block of 97.

### Set 2: RequestTaskInventory capability replies (`loadTaskInvLLSD`)

- `cap_replies.txt`, inputs `replies/<label>.llsd` (binary LLSD; notation
  shown for review): `requestInventory()` on an object whose region has the
  capability, then the reply delivered as HTTP 200; state and effects after
  the request and after the reply, and the entries. Covers `contents`
  missing, undefined, empty, a map, a string, entries without `item_id`,
  `running`/`faulted`, `inventory_serial` of every type and outside S16
  (the `(S16)` truncation), stale serials, localisation, links, group-owned,
  `shadow_id`, malformed items, a non-map body and an empty body.
- `cap_replies_random_samples.txt`, inputs `replies_random/`: a seeded sweep
  of 2000 bodies (generator below); every 97th kept; whole and per-block
  digests in `digests_set2.txt`.

### Set 3: serial and request-state traces

- `serial_traces_hand.txt`: hand-written traces (labels in the file):
  loading through each path, the capability URL, wrap at 32767/-32768 on
  both paths, every stuck-pending case, every ReplyTaskInventory branch, the
  serial an xfer completion records, the object dying while a delayed
  refetch waits (both paths), the local mutations, listeners.
- `serial_traces_len1_2.txt`: every sequence of 1 and 2 events, per
  configuration (udp/L0, udp/L1, cap/L0, cap/L1: capabilities absent or
  present, no listener or one).
- `serial_traces_len3_samples.txt`, `serial_traces_len4_samples.txt`: every
  97th trace of all sequences of 3 and 4 events; `digests_set3.txt` holds the
  count and SHA-256 of each length's complete text (lengths 1-4: 66, 1090,
  18018, 298114 traces) and of each block of 97 traces (block k starts with
  sample k).

Alphabets: udp `req dirty rep.lo rep.cur rep.exp rep.hi rep.none xfer.ok
xfer.err xfer.nofile timer upd.known upd.guess upd.new remove move.nocopy
create.ok`; cap `req dirty rep.lo rep.exp cap.ok cap.stale cap.304 cap.err
cap.nocontents timer upd.known upd.guess upd.new remove move.nocopy
create.ok`. The legend in each file defines every event. Each line after an
event is `<event> | st= ser= exp= xid= dirty= inv= cache= | <side effects in order>`,
where `cache` lists the task inventory files of the trace still in the cache
(a file is removed only by a successful `loadTaskInvFile`, so for example an
xfer completion with an older serial leaves its file behind).

A port reproduces a length's digests by generating the sequences in the
stated order (configurations in the order above; within one, lexicographic
by alphabet index) and the trace text exactly as in the sample files.

### Set 4: messages the viewer sends

- `messages.txt`: per case, the state before and after, the side effects,
  and each UDP datagram received on the loopback receiver: the wire bytes
  (packet id, bytes 1-4, zeroed; no acks are appended since nothing is
  received), the zero-expanded body, and every block and variable as raw
  bytes; then the entries. A StartPingCheck marker sent through the same
  socket after each action delimits the capture (a marker missing after 30 s
  fails the case); before the first capture, markers are resent every 500 ms
  until one arrives, since on Windows the first datagram of a new loopback
  flow was seen to arrive seconds late under load. Cases: UpdateTaskInventory
  with key 0 and 1 through each branch of `doUpdateInventory`'s owner guess
  (selected object, group-owned old item, known owner, guessed owner, not
  owner, AT_OBJECT not atomic, no inventory, a rich item with transaction
  id, flags, sale, masks and a creation date above `INT_MAX`), the owner
  change with next-owner mask `PERM_MOVE` for a notecard and an AT_OBJECT on
  the selected and not-owner branches (`is_atomic`: `PERM_TRANSFER` is added
  to the new base only for the notecard), `updateMaterialInventory`, the
  local-only updates, RemoveTaskInventory (present, absent, a duplicated id:
  the first match only is removed), MoveTaskInventory (null and explicit
  folder, copyable, no-copy, absent), RequestTaskInventory (and a duplicate
  request dropped while pending), RezScript with and without
  `NewScriptInfo`, RequestXfer and AbortXfer from the real xfer manager (a
  newer reply replacing the transfer, a delayed refetch aborting it), and
  the capability URL. RequestXfer/AbortXfer ids are random and shown as
  `<xfer id real#k>`; they are zeroed in `body` (and in `wire` when the
  message is not zero-coded; RequestXfer is zero-coded, so its wire bytes are
  not recorded).
- `create_item.txt`, bodies `create/<label>.llsd`: `createInventoryItem`'s
  POST URL and LLSD body (binary file, notation and XML), the reply, and the
  effects (`++expected`, `dirtyInventory`, `requestInventory`, then the
  caller's callback), and the immediate failures without region or
  capability.

## The sweep generators

Both use xorshift64*: state `s` (64-bit; a seed of 0 becomes
`0x9E3779B97F4A7C15`); `next()`: `s ^= s >> 12; s ^= s << 25; s ^= s >> 27;`
return `s * 0x2545F4914F6CDD1D` (all modulo 2^64); `below(n)` = `next() % n`
(0 when n = 0); `coin()` = top bit of `next()`. Every random draw below is
one call, in the order written.

Set 1 (seed `0x52344154`, 3000 files). The base is `files/normal.inv`, split
into lines each keeping its `\n` (a last line without `\n` is kept as is).
For each file: `ops = 1 + below(4)`; then `ops` times, while lines remain:
`at = below(line count)`; `op = below(7)`; if `op >= 4` and the line at `at`
is protected, `op = 1`. A line is protected when its first
whitespace-delimited token (at most 63 bytes) is one of `creation_date
base_mask owner_mask group_mask everyone_mask next_owner_mask creator_mask
creator_id owner_id last_owner_id group_id group_owned` (the readers scan
these into uninitialised locals; see runtime edges). Then: 0 delete the line;
1 insert a copy of it before it; 2 insert `TOKENS[below(13)]` before it; 3
`j = below(line count)` and swap lines `at` and `j`; 4 if the line is not
empty, `value = 0x20 + below(0x5f)`, then `pos = below(length)`, and the
byte at `pos` becomes `value`; 5 truncate the line to `below(length + 1)`
bytes; 6 `count = below(300)`, then `pos = below(length + 1)`, insert `count`
bytes `m` at `pos`. The file is the lines concatenated. `TOKENS`, in order:
`\tinv_item\t0\n`, `\tinv_object\t0\n`, `\t{\n`, `\t}\n`, `\n`, ` \n`,
`\tjunk\t1\n`, `\t\tname\tX|\n`, `\t\ttype\ttexture\n`, `\tpermissions 0\n`,
`\tsale_info\t0\n`, `\t\tflags\t-1\n`, `\t\tsale_price\tx\n`. File `i`
(0-based) is kept as `random/random_%04d.inv` when `i % 97 == 0`.

Set 2 (seed `0x52344152`, 2000 bodies). For each body, in order:
`switch below(6)`: 0 no `inventory_serial`; 1 `inventory_serial` = integer
`(S32)(next() & 0xffffffff)`; 2 = the decimal string of `below(70000) - 35000`;
3-5 = integer `below(70000) - 35000`. If `below(10) == 0` the body has no
`contents`. Otherwise `n = below(6)` entries, each: `kind = below(10)`; 0 the
string `junk`; 1 the map `{name: "no id"}`; else an item: id
`1a000000-0000-4000-8000-%012x` of `next() & 0xffff`; asset
`a5000000-0000-4000-8000-%012x` of `next() & 0xffff`; asset type
`[7, 6, 10, 0, 24, 8, 57, 56][below(8)]`; inventory type
`[7, 6, 10, 0, -1, 26][below(6)]`; name `["New Script", "Contents", "Thing",
"", "  padded  ", "dance8", "a|b", "\xc3\xa9", "New Folder"][below(9)]`; base,
owner, group, everyone and next-owner masks, five `(U32)next()` in that
order; owner `agent` if `coin()` else `other`; group null if `coin()` else
`othergroup`; flags `(U32)next()`; sale type `below(4)`; sale price
`below(1000)`; creation date `next() & 0xffffffff`. The entry is
`LLInventoryItem::asLLSD()` of that item (creator `creator`, last owner
null, parent `obj`, permissions `init(creator, owner, null, group)` then
`initMasks(base, owner, everyone, group, next)`, `LLSaleInfo(type, price)`,
the creation date through the constructor's `(S32)` cast), then: if
`below(4) == 0`, `running = coin()`; if `below(4) == 0`, `faulted = coin()`;
if `below(8) == 0` the `permissions` key is erased; if `below(8) == 0` the
`sale_info` key is erased. After the body, the case's starting
`mExpectedInventorySerialNum` is `(S16)(below(70000) - 35000)` if
`below(5) == 0`, else 0. Body `i` is kept as `replies_random/random_%04d.llsd`
when `i % 97 == 0`.

## Recorded quirks (as they are; not fixed)

- `inventory_serial` is never added to the capability URL: the condition
  `mInventorySerialNum && mInventory` is evaluated after
  `fetchInventoryFromServer` deleted `mInventory` (the coroutine runs at once
  under `launch::dispatch`). So the 304 branch is reached only when a server
  answers 304 unasked (`cap_304`, `cap_304_after_edit`).
- Stuck pending: a failed xfer (`xfer.err`), a missing file
  (`xfer.nofile`), an object gone during the transfer, and a capability reply
  without `contents` (or a non-map or empty body) leave the state at X;
  `requestInventory` is then dropped while `mInventory` is NULL. On the UDP
  path a later ReplyTaskInventory starts a new transfer; on the capability
  path a ReplyTaskInventory only sets `mExpectedInventorySerialNum` and calls
  the no-op `fetchInventoryFromServer`, so nothing unsticks it
  (`stuck_cap_no_contents_reply_does_not_unstick`). While `mInventory` is
  not NULL, `requestInventory` serves the cached list and its
  `doInventoryCallback` resets the state to S even with a transfer still
  outstanding (`older_serial_file_ignored`). An object that leaves
  `gObjectList` while a delayed refetch waits stays in W for good once it is
  back (`object_gone_during_delay_udp`).
- Signed S16 serials without wrap handling: `-32768` after `32767`; a reply
  of `32767` loads when `-32768` is expected (`wrap_old_reply_loaded`); the
  capability serial is `(S16)asInteger()` (65535 -> -1, 32768 -> -32768,
  -32769 -> 32767).
- An unsolicited ReplyTaskInventory with the capability may lower
  `mExpectedInventorySerialNum`.
- `deleteInventoryItem` removes the first match only and calls back only
  when nothing was deleted: an update of an absent item notifies twice.
- The update by asset key (key 1) keeps the incoming item's id
  (`update_by_asset`: E replaces A).
- `push_front` reverses file order and the first object in the file becomes
  the root; any blank or whitespace-only top-level line ends parsing; a line
  of 255 characters or more (excluding the newline) ends parsing silently,
  both at the top level (`loadTaskInvFile`'s own `getline(buffer, 255)`,
  `toplevel_line_255`) and inside an item (the item reader, `desc_line_255`),
  while 254 characters still parse; the 26th unknown top-level token ends
  parsing; `inv_object` is renamed `Contents`; the file is removed after a
  successful open only (an xfer completion with an older serial leaves it in
  the cache).
- Localisation applies on the capability path only (`{L10N:...}` in
  `cap_replies.txt`, none in `task_files.txt`).
- The 1-3 character remote filename throws `std::out_of_range` after the
  serials were updated and with the `LLFilenameAndTask` leaked.
- AbortXfer is sent unreliably (`sendMessage`), the task messages reliably.

## Runtime edges (undefined or C-library-defined behaviour)

- `creation_date` with a non-numeric value, and a permissions mask or id
  line whose value fails to scan, read an uninitialised local (stack
  garbage). No committed case does this; the random sweep never alters those
  lines in place.
- `creation_date` above `INT_MAX` (`date_over_int_max`): `sscanf("%d")`
  overflow is undefined by the C standard; MSVC's runtime and glibc both give
  the low 32 bits (4000000000 -> -294967296).
- `moveInventory` and `getInventoryItemByAsset` cast an `LLInventoryObject`
  to `LLViewerInventoryItem` without a check; with an `inv_object` of a
  non-category type matched by id, asset or type this reads past the object.
  The helpers line marks such lookups `not-called`; not exercised.
- A NULL `mRegionp` is dereferenced by the UDP request, `removeInventory`,
  `moveInventory`, `updateInventory` and `saveScript`. Not exercised.

## Platform differences

Recorded on Windows (MSVC 17, RelWithDebInfo) and on WSL Ubuntu 26.04
(g++ 15.2 and g++ 14.3, glibc 2.43); the committed files are the Windows
recording. Apart from the `# platform:` header line the outputs are
identical on all three, including the digests of the sweeps and
`date_over_int_max`. The cache directory delimiter differs (`\` on Windows)
and is shown as `/` (`<cache>/<name>`); file names never contain either
(they are scrubbed). CI's toolchain (Ubuntu 24.04, g++ 14, glibc 2.39) was
not run locally.

## Not covered

- Real simulator task inventory files: they cannot be obtained without
  logging in. Needs a live simulator.
- The simulator's answers to AbortXfer and the aborted-transfer callback:
  the fake never completes an aborted transfer.
