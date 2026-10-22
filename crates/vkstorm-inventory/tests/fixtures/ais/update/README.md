# ais/update: the AIS response decision corpus (recorder R3a)

Survey 3, layer A (sets 1-11), plus a seeded random set (12). What
`AISUpdate` in `indra/newview/llaisapi.cpp` does with an AIS command result:
the ordered list of calls it makes of the inventory model and of the viewer
inventory objects, the yields, the outgoing messages, the notifications and
the completion callback ids. The port moves the parse to Rust
(`vkstorm-inventory` `ais/update.rs`) producing an ordered op list that C++
applies; that path must reproduce these recordings exactly.

Recorded from the viewer C++ at `codex/rust-inventory` a6d6a04641 (llaisapi.cpp
blob b3ca253a86300c1f9d14713a9b3ff2522e79a3c7), i.e. *after* the two fixes the
user approved: LL #5358 (per-task 8 ms timer plus a per-frame 10 ms batch
timer shared by all updates) and FIRE-33455 (count-only descendent accounting
in `doUpdate`, the null check in the version loop, AIS versions only raise a
category's version). The fixtures are never edited by hand: change the
recorder or `gen_cases.py` and regenerate.

This directory is identical, byte for byte, in the viewer
(`indra/newview/tests/fixtures/inventory/ais/update`) and in the crate
(`crates/vkstorm-inventory/tests/fixtures/ais/update`). Its `.gitattributes`
(`* -text`) keeps git from converting line ends in the binary and gzip files.

## Files

| path | what |
|---|---|
| `cases/index.txt` | the case ids, in run order |
| `cases/<id>.case.xml` | LLSD XML: `id`, `set`, `title`, `agent_id`, `session_id`, `root_id`, `sched` {`call_us`, `frame_us`, `others_us`}, `large`, `seed` (sets 10 and 12), `skeleton` {`categories`, `items`}, `steps` |
| `cases/<id>.<n>.xml` | the result of step n, raw LLSD XML (set 11 holds duplicate map keys on purpose) |
| `cases/s10a_large_recursive.0.xml.gz` | the large result, gzipped (gzip mtime 0). The compressed bytes depend on the zlib that wrote them (Python 3.14 on Windows and on Linux differ); the content does not: 26,734,355 bytes, SHA-256 06e0e63e59f5d0f8e688cf33fb9d47120a39d06d9953e9219bf69d66f20af7ee. `gen_cases.py` reproduces every other file byte for byte. |
| `expected/<id>.expected.llsd` | binary LLSD: `{case, never: {ops, final}, sched: {ops, final}}` |
| `expected/<id>.expected.txt` | review rendering of the same: a header, then one line per op / category / item in compact notation (binary as b16) |
| `expected/s10a_large_recursive.digest.llsd` / `.txt` | the large case as digest and sample (below) |
| `expected/manifest.txt` | header, then `<case> <file> <sha256 of the binary file>` |

A step is `{type, target_id, url, body, response, same_frame, disconnected,
callback}`: `type` is the `AISAPI::COMMAND_TYPE` name; `target_id`, `body`
and `type` are the coroutine's arguments (`body` is what the command passes:
`{depth: N}` for the folder fetches, the request LLSD for writes, undefined
otherwise); `response` names the result file; `same_frame` keeps the step in
the frame the previous step ended in (otherwise the main loop moves to the
next frame first); `disconnected` sets `gDisconnected`; `callback` false runs
the command without a completion callback.

Skeleton categories are `{category_id, parent_id, name, type_default,
owner_id, version, descendents}`; skeleton items are AIS item maps (read with
`LLInventoryItem::fromLLSD`), `_complete` false marks an incomplete one.

## How the corpus is made

1. `tools/inventory-fixtures/ais-update/gen_cases.py` (vkstorm-core-rust)
   writes `cases/`. Ids are patterned (`I(n)` = `a0000000-0000-4000-8000-<n>`
   items, `L(n)` = `b...` links, `C(n)` = `c...` categories, `E(n)` = `e...`
   assets), so the `std::map` order in which `doUpdate` applies objects is
   the order of the id text: items, then links, then categories. Sets 10 and
   12 draw from xorshift64* (Vigna; `x ^= x>>12; x ^= x<<25; x ^= x>>27;
   return x * 0x2545F4914F6CDD1D`), seeds `0x5EED0A15DA7E0010` (set 10) and
   `0x5EED0A15DA7E0012` (set 12), recorded in the case files.
2. The viewer test `INTEGRATION_TEST_llaisupdate_parity`
   (`indra/newview/tests/llaisupdate_parity_test.cpp`,
   `llaisupdate_parity_stubs.{h,cpp}`, `tests/inventory/r3-update.cmake`)
   writes `expected/` in dump mode (`VKS_AISUPDATE_DUMP=<dir>`).
3. `tools/inventory-fixtures/ais-update/regenerate.py <viewer> <exe> <libdirs>`
   does both and copies this directory into the crate.

Without the dump variable the test replays every case and compares the
binary bytes; a difference is reported as `case <id>: <run> op <index>
(<op>)/<field path>: expected <value> actual <value>`. `VKS_AISUPDATE_FULL=<dir>`
also writes the full output of the large case (not committed);
`VKS_AISUPDATE_TRACE=1` prints every op as it is recorded.

## The harness

The test compiles the real `llaisapi.cpp` (included verbatim by the stubs'
translation unit) and runs each step through the real
`AISAPI::InvokeAISCommandCoro` with an invoke function that returns the
scripted result instead of making a request. So each step exercises the
coroutine's error branch (410 / 403 handling), `onUpdateReceived` ->
`AISUpdate(result, type, body)` + `doUpdate()`, and the completion callback
contract, exactly as a reply would. No HTTP, no coroutine scheduler, no real
inventory model.

* **The model.** `gInventory` is a recording stub over the scripted skeleton
  (see "The skeleton model").
* **The viewer inventory objects.** `LLViewerInventoryItem` and
  `LLViewerInventoryCategory` members the parse runs are frozen copies of
  `llviewerinventory.cpp` (constructors, `copyViewerItem`, `copyItem`,
  `copyViewerCategory`, both `unpackMessage(const LLSD&)`, `localizeName`,
  `setTransactionID`, `updateParentOnServer`, the link getters `getType`,
  `getName`, `getLinkedItem`, `getLinkedCategory`) and
  `LLInventoryModel::accountForUpdate` is a frozen copy of
  `llinventorymodel.cpp`. Each copy is fenced by `VKS-COPY` markers naming
  the file, line range and blob; `indra/newview/tests/check_r3a_copies.py`
  checks them against the live source (lines tagged `VKS-HARNESS` are the
  recorder's additions and are excluded). `getVersion`, `setVersion` and
  `fetch` are recording stubs. Every other virtual member throws if called
  (none is).
* **Name localisation.** The real `LLLocalizedInventoryItemsDictionary` fills
  its map from `LLTrans` strings the test does not load; the copy's map is
  empty, so names pass through unchanged.
* **Logging.** The default log level is raised to ERROR while recording, so
  getters called only to build a warning or debug line do not show up as
  model queries. Nothing the code decides depends on a log line.
* **Outgoing messages.** The real `LLMessageSystem` on the real
  `message_template.msg` builds the `MoveInventoryItem` of
  `updateParentOnServer`; the stubbed `gAgent.sendReliableMessage()` records
  `msg.send` with the bytes the template builder produced
  (`buildMessage(buf, size, 0)`: 6 zero bytes reserved for the packet header,
  then the message number and the blocks; before the circuit's sequence
  number, flags and zero-coding) and drops the message.
* **Timers and yields.** The stubs' translation unit includes every header
  `llaisapi.cpp` uses, then renames `LLTimer` (in `llaisapi.h/.cpp` only) to a
  class with `LLTimer`'s arithmetic over an injected microsecond clock, and
  `llcoro::suspendUntilNextFrame` to a stub that records a `yield` and moves
  the injected frame counter (`LLFrameTimer::getFrameCount()`) on.

### The two runs

Every case runs twice, in one process, case after case in index order:

* `never`: the timers never report expiry. The op list has no yields: the
  canonical order of the model calls.
* `sched`: the timers keep `LLTimer`'s exact arithmetic over the injected
  clock. The clock moves `call_us` (case file; default 100 us, 5 us in
  s10a) at every recorded event except step markers, and `frame_us` (16667)
  at every yield. `others_us` (cycled over the yields) scripts concurrent
  AIS updates: after a yield, another update is taken to have resumed first
  in the new frame, re-armed the shared batch timer (10 ms) and run for that
  long, as a concurrent `InvokeAISCommandCoro` would. Without it the batch
  timer can never be the cause of a yield (see "Findings" 1).

## The op list

Every event, in order, is a map with `op`:

| op | fields | recorded when |
|---|---|---|
| `step` | `index`, `type` | the harness starts a step (costs no time) |
| `q.getCategory` | `id`, `found`, `obj` (the model's category, all fields) | `gInventory.getCategory()` |
| `q.getItem` | `id`, `found`, `obj` | `gInventory.getItem()` |
| `q.getVersion` | `id`, `result` | `getVersion()` on a category of the model (not on the parse's detached copies) |
| `q.findCategoryUUIDForType` | `type`, `result` | `gInventory.findCategoryUUIDForType()` |
| `accountForUpdate` | `category`, `delta`, `change_version`, `model` {`found`, `version`, `descendents` after} | `gInventory.accountForUpdate()` |
| `updateCategory` | `mask`, `obj` (the object passed, all fields), `exists`, `parent_known` | `gInventory.updateCategory()` |
| `updateItem` | `mask`, `obj`, `exists`, `parent_known`, `model` (what the real model does with it, below) | `gInventory.updateItem()` |
| `onObjectDeletedFromServer` | `id`, `fix_broken_links`, `update_parent_version`, `do_notify_observers`, `exists` (category / item / none) | `gInventory.onObjectDeletedFromServer()` |
| `fetchDescendentsOf` | `id`, `would_start` | `gInventory.fetchDescendentsOf()` (410 branch) |
| `setVersion` | `id`, `version`, `before` | `setVersion()` on a category of the model |
| `fetch` | `id`, `expiry`, `version`, `would_start`, `is_model_object` | `LLViewerInventoryCategory::fetch()`: `would_start` is the real gate (version unknown and the 10 s / 360 s request timer expired) |
| `updateParentOnServer` | `id`, `parent_id`, `restamp`, `is_model_object` | the lost-item server move |
| `msg.send` | `name`, `reliable`, `body` (binary) | the message `updateParentOnServer` sends |
| `notifyObservers` | `changed` (size of the changed-id set before the flush) | `gInventory.notifyObservers()` called by AIS code (the 50-change backlog flushes and the final one) |
| `notify` | `name` | `LLNotificationsUtil::add()` |
| `yield` | `frame`, `clock_us` (relative to the run), `others_us` | `llcoro::suspendUntilNextFrame()` |
| `callback` | `id` | the completion callback |
| `exception` | `what` | a C++ exception escaped a step (none does) |

Objects are dumped raw (no link indirection): items `{item_id, parent_id,
asset_id, type, inv_type, flags, name, desc, created_at, thumbnail_id,
favorite, runtime, permissions {creator_id, owner_id, last_owner_id,
group_id, is_owner_group, base_mask, owner_mask, group_mask, everyone_mask,
next_owner_mask}, sale_info {sale_type, sale_price}, complete,
transaction_id, running, faulted}`; categories `{category_id, parent_id,
name, type_default, type, thumbnail_id, favorite, owner_id, version,
descendents, fetching}`. Enum values and masks are raw integers (U32 as
signed 32-bit). `final` is the skeleton after the run: every category and
item by id, and the model's changed-id set.

## The queries the parser makes (the interface the Rust side needs)

Recorded (appear in the op list):

* `getCategory(id)`: the target of a fetch or write reply (stale check,
  "default to current values"), every embedded category, every id in
  `_categories_removed`, then in `doUpdate` every category with a descendent
  delta (accounting), every updated category (copy back of version and
  count *after* the accounting), every id in `_updated_category_versions`
  (version rule, *after* every create, update and delete), and the 410
  REMOVECATEGORY target.
* `getItem(id)`: every parsed item and link (top level or embedded), every
  id in `_category_items_removed`, `_removed_items`, `_broken_links_removed`,
  the 410 REMOVEITEM target.
* `getVersion()` of a model category: the stale check, the copy of the
  current category into the parse result, the fetch version comparison, the
  copy back in step 3 and the version rule in step 8.
* `findCategoryUUIDForType(FT_LOST_AND_FOUND)`: once per update with lost
  items.

Not recordable (inline or base-class reads) but read by the parse, so part
of the interface: the current object's every field (`copyViewerItem`, the
category copy constructor: parent, type, name, owner, thumbnail, favorite,
version, descendent count, the fetch request timer); the category's
descendent count (stale check, copy back); the parent id of removed objects
(delta bookkeeping); the size of the model's changed-id set after each
create (the 50-change flush). Steps 3 and 8 read the model *after* earlier
ops of the same update were applied, so a port either applies the op list
in order with those two reads done at apply time, or reproduces the model's
state between ops.

## The skeleton model

`gInventory` keeps the skeleton and emulates the effects of the calls on it,
so that later queries of the same update (and later steps) see what the
real model would show. Emulated: `updateCategory` (copy into the existing
category, or add a new one; changed id), `updateItem` (copy or add; changed
id; mesh, GLTF and GLTF-binary types skipped, link types resolved first as
the real `getType()` does; a new item with a null parent goes to its type's
default folder, an item whose parent folder is unknown goes to Lost And
Found, an existing item updated with a null parent keeps its old parent or
goes to Lost And Found), `onObjectDeletedFromServer` (descendents removed,
parent accounting when asked, the deleteObject changed-id and flush
sequence), `accountForUpdate` (the frozen copy), `notifyObservers` (clears
the changed-id set), `fetch` (the request timer gate), changed-id bookkeeping
(the referent, plus links pointing at it when LABEL is set). Children of a
folder are the skeleton objects whose parent is that folder.

Not emulated, and recorded only as the `updateItem` `model` note: the real
model's own server moves inside `updateItem` (`..._server_move`:
`updateParentOnServer` and `MoveInventoryItem` are sent by the model there,
survey 2 and R2's corpus own that), calling-card name lookups, the
marketplace hooks, observers, and the bug where a new category whose parent
is created later in the same update is not linked into the parent's child
array (survey 3 §6 bug 12; `parent_known` false shows where it would occur).
The skeleton's own `descendents` default to its child count, so accounting
applies unless a case sets a mismatch.

## Coverage (survey 3 §8.1)

| set | survey case | cases |
|---|---|---|
| 1 | fetch children depth 0: full, no `links` key, COF links only, outfit links only | s01a-s01g (plus a plain folder with links only, a known folder refetched, a category without `agent_id`) |
| 2 | recursive depth 50, 52-level tree | s02 (53 levels: versions on levels 0-50, not 51-52) |
| 3 | subset fetch: parent not applied, children at depth 49 | s03a-s03c |
| 4 | links with `_embedded` item and category, broken link | s04a-s04d (fetch, create with the created filter, top-level link fetch, link update) |
| 5 | stale version (FIRE-33337), higher, -1 | s05a-s05i (also missing version, viewer unknown, stale twice in a frame, stale on a write, request depth -1) |
| 6 | fetch item, fetch link, null parent (server move), unknown parent, mesh/GLTF | s06a-s06h (also no Lost And Found, lost link, every item field) |
| 7 | orphans | s07 |
| 8 | create category, create links, slam, update item, update category with `_updated_categories`, remove item and category, every removed list, `_updated_category_versions` higher / lower / equal / -1 / unknown id | s04b, s08a-s08n, the callback matrix s08z_* (every command type x six result shapes, and no callback) |
| 9 | 4xx map with `item_id`, 410 remove item / category, 403 depth 0 and 50, non-map body, empty body | s09a-s09o (also 499, 500, 4xx non-map, disconnected, create failure, error bodies that parse as objects, transport failure) |
| 10 | 20,000 items, 2,000 folders, recursive | s10a (digest and sample), s10b-s10c (the batch timer) |
| 11 | duplicate keys in XML maps | s11a-s11e (`version`, an item id, `_embedded`, meta keys, a `_updated_category_versions` entry) |
| 12 | (added) random combinations | s12_random_00-39 |

Counts: 117 cases (40 of them in set 12), 253 steps (78 in set 12), 234
runs. Committed per tree: 2.58 MB of inputs, 5.94 MB of expected outputs.

### The large case (s10a)

`FETCHCATEGORYCHILDREN` at depth 50 answered with a 10-ary tree of 2,000
folders (all three `_embedded` keys present, so every count and version is
known), 20,000 items (10 per folder, kinds from the seeded stream; about one
in eleven is a mesh, which the model skips) and 500 links. Its full output
(45,411 ops in `never`, 45,468 in `sched`) is about 62 MB as binary LLSD, so
it is committed as `expected/s10a_large_recursive.digest.llsd`:
`sha256` of the whole binary output, and per run `ops_sha256` and
`final_sha256` (SHA-256 of the binary LLSD of the op array and of the final
skeleton), `op_count`, `counts` per op kind, `head` (first 60 ops), `tail`
(last 40), `events` (every op that is not a query or an update, with its
index: the 406 observer flushes, the yields, the callback),
`final_categories`, `final_items`. The test recomputes the whole structure
and compares bytes; the Rust side can compare the digests of its own
binary output and the sample.

## Runtime edges

* **Process order.** The 403 depth-0 notification is chosen by a
  function-static flag of `InvokeAISCommandCoro`: the first 403 at depth 0 in
  the process gets `InventoryLimitReachedAISAlert`, every later one
  `InventoryLimitReachedAIS`. So s09e's `never` run records the alert first
  and its `sched` run records only the plain one. The test always runs every
  case in index order in one process; running a subset changes this.
* **Static batch frame.** `AISUpdate::sBatchFrameCount` and `sBatchTimer`
  are statics that outlive a run; each run starts in a frame no earlier run
  used, so the first update of a run always restarts the batch timer.
* No undefined behaviour is reached. `mCatDescendentDeltas` keeps deltas in
  `size_t` and reads them back with `static_cast<S32>`; a delta of -1 is
  `SIZE_MAX` and reads back as -1 on both platforms (two's complement): a
  port must use a signed type.

## Platforms

Recorded on Windows (MSVC 17, RelWithDebInfo, the Release feature set) and
on Linux (WSL Ubuntu 26.04, GCC 15.2, and again with GCC 14.3 as on CI's
Ubuntu 24.04). The Linux dump is byte-identical to the Windows one in every
`.llsd` file and in every `.txt` line but the `# platform` and
`# viewer commit` header lines; both platforms replay the committed
fixtures with 0 differences. Nothing recorded depends on floating point
beyond the timers' `(U64)(F32)(seconds * 1e6)` (8000 and 10000 ticks).

## Mock fidelity

The shapes of real AIS replies at the depth limit and of 403 / 410 error
bodies are not known from the viewer alone (survey 3 §8.2, §10). Those cases
(s02, s03c, s05i, set 9) are built from what the viewer's parser and the HTTP
adapter accept: results carry `http_result` as
`LLCoreHttpUtil::HttpCoroHandler` writes it, a 4xx map body becomes the
result, a 4xx non-map body goes under `content`, 5xx and transport failures
leave an empty map, an unparsable LLSD body becomes status 499. They record
what the C++ does with such a reply, not that AIS sends it.

## Findings (recorded as they are, not fixed)

1. `AISUpdate`'s constructor calls `setTimerExpirySec()` and then `start()`,
   which resets `mExpirationTicks` to 0, on both the task timer and (on a new
   frame) the batch timer. Every update therefore yields one frame at its
   first `checkTimeout()` (the first `_embedded` or the start of `doUpdate`),
   however small the reply; after a yield both timers are re-armed together
   (8 ms and 10 ms), so the batch timer can cause a yield only when another
   update resumed first in the frame. Same in Firestorm upstream.
2. A fetch that answers a lower version than the model's is applied (version
   goes down) when the model's descendent count is unknown (s05b): the
   FIRE-33455 "versions only raise" rule covers `_updated_category_versions`
   only.
3. REMOVEITEM 410 deletes the item with the default
   `update_parent_version=true`, bumping the parent's local version (s09b;
   survey 3 §6 bug 14).
4. Error results are parsed: a 4xx map body with `item_id` reports success to
   a FETCHITEM caller (s09a), one with `category_id` and `parent_id`, or a
   full item, is applied to the model (s09m, s09o).
5. The `LL_ERRS("unpack failed")` crash after `unpackMessage` (survey 3 §6
   bug 3) cannot be reached from LLSD input in this tree:
   `LLInventoryCategory::fromLLSD` always returns true, and
   `LLInventoryItem::fromLLSD` returns false only when
   `LLSaleInfo::fromLLSD` does, which it never does.
