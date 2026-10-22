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
(`indra/newview/tests/inventory/r4-task.cmake`) drives frozen copies of the
task-inventory code (`indra/newview/tests/llinventory_task_oracle.cpp`):
`llviewerobject.cpp` lines 155-160, 2823-3231, 3375-3978, 6252-6269,
7229-7235 and 8204-8285 (blob `c9ac3bdc20`), and the `LLViewerInventoryItem`
members they use from `llviewerinventory.cpp` lines 87-88, 104-233,
335-408 and 547-601 (blob `a13210dce6`). The copies are verbatim; they reach
fakes for the viewer singletons by ordinary name lookup in the namespace
`vks_task_oracle` (`llinventory_task_oracle.h`, `llinventory_task_oracle_fakes.cpp`).
`indra/newview/tests/check_r4a_copies.py` checks every copied region against
the live source.

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

Fakes (what they model):

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
in the file headers); no wall clock (timers fire on events); the working
files live under the build tree (`VKS_TASK_WORK_DIR`), never in the system
temporary directory.

Run, replay and regenerate:

```sh
# viewer, after configuring with LL_TESTS ON (builds, then runs the replay):
cmake --build <build> --config RelWithDebInfo --target INTEGRATION_TEST_llinventory_task_oracle
# record again into the crate and the viewer (from vkstorm-core-rust):
python tools/inventory-fixtures/task/regenerate.py <path to INTEGRATION_TEST_llinventory_task_oracle> --viewer <viewer checkout>
python indra/newview/tests/check_r4a_copies.py      # in the viewer
```

The replay compares text files line by line, skipping `#` lines (the header
names the platform), binary inputs byte for byte, and the large sweeps by
the SHA-256 of their canonical text. A failure names the file, the content
line, the case or trace it belongs to, and the expected and actual lines.
`VKS_TASK_ONLY=<1-4>` runs one set (development aid).

## Files

Every text file starts with a `#` header (generator, viewer commit and copied
blobs, platform, seed) and a legend.

### Set 1: task inventory files (`loadTaskInvFile`)

- `task_files.txt`, inputs `files/<label>.inv`: per case, the input's size and
  SHA-256; `direct`: `loadTaskInvFile(<name>)`'s return value, whether the
  cache file was removed, the object state and the side effects (listener
  callbacks); then every entry of `mInventory` front to back (`push_front`
  reverses file order) with every field, and the root (`getInventoryRoot()`
  = `back()`). Field order: kind (item/object), uuid, parent, raw asset type,
  name, thumbnail, favourite, runtime, creation date; for items asset,
  description, raw inventory type, flags, sale type/price, the five masks,
  creator, owner, last owner, group, group-owned, `mIsComplete`, transaction
  id, running, faulted, CRC. Cases marked `via_xfer` also run the bytes
  through ReplyTaskInventory -> xfer -> `processTaskInvFile` with the pending
  list `[assetP, assetE, assetA]` and record the list afterwards.
  `missing_file` is the open failure.
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
  sweep of 3000 mutations of `files/normal.inv` (line deletion, duplication,
  insertion of tokens, swaps, byte replacement, truncation, long inserts);
  every 97th case is kept with its input; `digests_set1.txt` holds the
  SHA-256 of the concatenated case text of all 3000.

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
  of 2000 bodies; every 97th kept; digest in `digests_set2.txt`.

### Set 3: serial and request-state traces

- `serial_traces_hand.txt`: hand-written traces (labels in the file):
  loading through each path, the capability URL, wrap at 32767/-32768 on
  both paths, every stuck-pending case, every ReplyTaskInventory branch,
  the local mutations, listeners.
- `serial_traces_len1_2.txt`: every sequence of 1 and 2 events, per
  configuration (udp/L0, udp/L1, cap/L0, cap/L1: capabilities absent or
  present, no listener or one).
- `serial_traces_len3_samples.txt`, `serial_traces_len4_samples.txt`: every
  97th trace of all sequences of 3 and 4 events; `digests_set3.txt` holds the
  count and SHA-256 of each length's complete text (lengths 1-4: 66, 1090,
  18018, 298114 traces).

Alphabets: udp `req dirty rep.lo rep.cur rep.exp rep.hi rep.none xfer.ok
xfer.err xfer.nofile timer upd.known upd.guess upd.new remove move.nocopy
create.ok`; cap `req dirty rep.lo rep.exp cap.ok cap.stale cap.304 cap.err
cap.nocontents timer upd.known upd.guess upd.new remove move.nocopy
create.ok`. The legend in each file defines every event. Each line after an
event is `<event> | st= ser= exp= xid= dirty= inv= | <side effects in order>`.

A port reproduces a length's digest by generating the sequences in the
stated order (configurations in the order above; within one, lexicographic
by alphabet index) and the trace text exactly as in the sample files.

### Set 4: messages the viewer sends

- `messages.txt`: per case, the state before and after, the side effects,
  and each UDP datagram received on the loopback receiver: the wire bytes
  (packet id, bytes 1-4, zeroed; no acks are appended since nothing is
  received), the zero-expanded body, and every block and variable as raw
  bytes. Cases: UpdateTaskInventory with key 0 and 1 through each branch of
  `doUpdateInventory`'s owner guess (selected object, group-owned old item,
  known owner, guessed owner, not owner, AT_OBJECT not atomic, no
  inventory, a rich item with transaction id, flags, sale, masks and a
  creation date above `INT_MAX`), `updateMaterialInventory`,
  RemoveTaskInventory (present, absent), MoveTaskInventory (null and
  explicit folder, copyable, no-copy, absent), RequestTaskInventory (and a
  duplicate request dropped while pending), RezScript with and without
  `NewScriptInfo`, RequestXfer and AbortXfer from the real xfer manager (a
  newer reply replacing the transfer, a delayed refetch aborting it), and the
  capability URL. RequestXfer/AbortXfer ids are random and shown as
  `<xfer id real#k>`; they are zeroed in `body` (and in `wire` when the
  message is not zero-coded; RequestXfer is zero-coded, so its wire bytes
  are not recorded).
- `create_item.txt`, bodies `create/<label>.llsd`: `createInventoryItem`'s
  POST URL and LLSD body, the reply, and the effects (`++expected`,
  `dirtyInventory`, `requestInventory`, then the caller's callback), and the
  immediate failures without region or capability.

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
  outstanding (`older_serial_file_ignored`).
- Signed S16 serials without wrap handling: `-32768` after `32767`; a reply
  of `32767` loads when `-32768` is expected (`wrap_old_reply_loaded`); the
  capability serial is `(S16)asInteger()` (65535 -> -1, 32768 -> -32768,
  -32769 -> 32767).
- An unsolicited ReplyTaskInventory with the capability may lower
  `mExpectedInventorySerialNum`.
- `deleteInventoryItem` calls back only when nothing was deleted: an update
  of an absent item notifies twice.
- The update by asset key (key 1) keeps the incoming item's id
  (`update_by_asset`: E replaces A).
- `push_front` reverses file order and the first object in the file becomes
  the root; any blank or whitespace-only top-level line ends parsing; a line
  of 254 or more characters ends parsing silently; the 26th unknown top-level
  token ends parsing; `inv_object` is renamed `Contents`; the file is removed
  after a successful open only.
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
  overflow is undefined by the C standard; MSVC's runtime gives the low 32
  bits (4000000000 -> -294967296).
- `moveInventory` and `getInventoryItemByAsset` cast an `LLInventoryObject`
  to `LLViewerInventoryItem` without a check; with an `inv_object` of a
  non-category type matched by id or asset this reads past the object. Not
  exercised.
- A NULL `mRegionp` is dereferenced by the UDP request, `removeInventory`,
  `moveInventory`, `updateInventory` and `saveScript`. Not exercised.

## Platform differences

Recorded on Windows (MSVC 17, RelWithDebInfo) and on WSL Ubuntu 26.04
(g++ 15.2 and g++ 14.3, glibc 2.43); the committed files are the Windows
recording. Apart from the `# platform:` header line the outputs are
identical on all three, including the digests of the sweeps and
`date_over_int_max` (both C runtimes give -294967296). The cache directory
delimiter differs (`\` on Windows) and is shown as `/`
(`<cache>/<name>`); file names never contain either (they are scrubbed).
CI's toolchain (Ubuntu 24.04, g++ 14, glibc 2.39) was not run locally.

## Not covered

- Real simulator task inventory files: they cannot be obtained without
  logging in. Needs a live simulator.
- The simulator's answers to AbortXfer and the aborted-transfer callback:
  the fake never completes an aborted transfer.
