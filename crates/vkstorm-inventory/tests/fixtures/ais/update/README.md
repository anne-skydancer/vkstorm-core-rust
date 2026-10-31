# ais/update: the AIS response decision corpus (recorder R3a)

Survey 3, layer A (sets 1-11), plus a seeded random set (12) and the sets the
V0 audit asked for (13-18). What `indra/newview/llaisapi.cpp` does with an
AIS command result, and what its command functions send: the ordered list of
calls it makes of the inventory model and of the viewer inventory objects,
the queries it makes, the yields, the outgoing messages and requests, the
notifications and the completion callback ids. The port moves the parse to
Rust (`vkstorm-inventory` `ais/update.rs`) producing an ordered op list that
C++ applies; that path must reproduce these recordings.

Recorded from the viewer C++ at `codex/rust-inventory` a6d6a04641 merged with
`codex/inventory-upstream-fixes-rust` 85991ba914 (llaisapi.cpp blob
8d34822459c9c59ca7cd8ac8338cb76da6089c71 at recording; the headers name the
exact commit), i.e. *after* the fixes the user approved: LL #5358 (per-task
8 ms timer plus a per-frame 10 ms batch timer shared by all updates),
FIRE-33455 (count-only descendent accounting in `doUpdate`, the null check in
the version loop, AIS versions only raise a category's version), bug 15
(both timers are started before their expiry is set, so a new update no
longer yields at once), bug 51 (`checkTimeout()` loops: after resuming it
rechecks the shared budget and yields again if another update spent it) and
bug 52 (`updateServer()` accounts count-only on the AIS path; the harness
never reaches `updateServer()`). Re-recorded after `vkstorm-rust` took the
inventory fixes of PR #181 (merged into this branch; llaisapi.cpp
unchanged): of those, only bug 23 ("Don't list a new item that addItem()
refused") reaches what R3a emulates: `updateItem()` now returns at once,
with the mask as it came, when `addItem()` refuses a new item of no asset
type (type <= -1). The stub does the same (`model` `refused_bad_type`) and
s15h records it; no earlier case sends such an item, so every other
recording is unchanged but for the headers' commit. Re-recorded again for
the fix of finding 9 (approved 2026-10-28): `AISUpdate::parseItem()` no
longer counts a created item of no asset type, which `addItem()` refuses,
as a new descendant of its folder (the folder's delta entry is still made,
with 0, as for an updated item). Only s15h reaches it. Re-recorded once more
for the AIS depth cap (approved 2026-10-29): `InvokeAISCommandCoro` sets a
256-level LLSD depth limit on its adapter, so an over-deep reply reaches the
coroutine as an unparsable one (`../adapter` records that); every output is
unchanged but its header. The fixtures are never edited by hand: change the
recorder or `gen_cases.py` and regenerate.

This directory is identical, byte for byte, in the viewer
(`indra/newview/tests/fixtures/inventory/ais/update`) and in the crate
(`crates/vkstorm-inventory/tests/fixtures/ais/update`). Its `.gitattributes`
(`* -text`) keeps git from converting line ends in the binary and gzip files.

## Files

| path | what |
|---|---|
| `cases/index.txt` | the case ids, in run order |
| `cases/<id>.case.xml` | LLSD XML: `id`, `set`, `title`, `agent_id`, `session_id`, `root_id`, `sched` {`call_us`, `frame_us`, `others_us`, `yield_extra_us`}, `large`, `seed` (sets 10 and 12), `skeleton` {`categories`, `items`}, `steps` |
| `cases/<id>.<n>.xml` | the result of step n, raw LLSD XML (set 11 holds duplicate map keys on purpose) |
| `cases/<id>.<n>.h<h>.<a>.xml` | the result of a step that hook h of step n runs (action a) |
| `cases/s10a_large_recursive.0.xml.gz` | the large result, gzipped (gzip mtime 0). The compressed bytes depend on the zlib that wrote them (Python 3.14 on Windows and on Linux differ); the content does not: 26,734,355 bytes, SHA-256 06e0e63e59f5d0f8e688cf33fb9d47120a39d06d9953e9219bf69d66f20af7ee. `gen_cases.py` reproduces every other file byte for byte. |
| `expected/<id>.expected.llsd` | binary LLSD: `{case, never: {ops, final}, sched: {ops, final}}` |
| `expected/<id>.expected.txt` | review rendering of the same: a header, then one line per op / category / item in compact notation (binary as b16); the sched run's final skeleton is shown only where it differs from the never run's |
| `expected/s10a_large_recursive.digest.llsd` / `.txt` | the large case as digests and samples (below) |
| `expected/manifest.txt` | header, then `<case> <file> <sha256 of the binary file>` |

The replay fails on a missing *or* an unexpected file anywhere in this tree
(the top level holds exactly `.gitattributes`, `README.md`, `cases/`,
`expected/`; `cases/` exactly the index, the case files and the result
files they name; `expected/` exactly the manifest and two files per case).

### Skeletons

Categories are `{category_id, parent_id, name, type_default, owner_id,
version, descendents}`; items are AIS item maps (read with
`LLInventoryItem::fromLLSD`), `_complete` false marks an incomplete one and
`_transaction_id` sets the item's transaction id.

### Steps

A step is one of:

* **A result through the coroutine**: `{type, target_id, url, body,
  response, same_frame, disconnected, callback, hooks}`. `type` is the
  `AISAPI::COMMAND_TYPE` name; `target_id`, `body` and `type` are the
  coroutine's arguments (`body` is what the command passes: `{depth: N}` for
  the folder fetches, the request LLSD for writes, undefined otherwise);
  `response` names the result file (the LLSD the HTTP adapter would have
  produced, `http_result` included). The real `AISAPI::InvokeAISCommandCoro`
  runs with an invoke function that returns it: the error branch,
  `AISUpdate(result, type, body)` + `doUpdate()`, the callback contract.
* **A call of a command function** (set 18): `{call, args, caps, no_region,
  pending, idle, idle_pending, callback, response}`. The real `AISAPI`
  function runs (`CreateInventory`, `SlamFolder`, `RemoveCategory`,
  `RemoveItem`, `PurgeDescendents`, `UpdateCategory`, `UpdateItem`,
  `FetchItem`, `FetchCategoryChildren` by id and by name
  (`FetchCategoryChildrenByName`), `FetchCategoryCategories`,
  `FetchCategorySubset`, `FetchCOF`, `FetchCategoryLinks`, `FetchOrphans`,
  `CopyLibraryCategory`, plus `isAvailable`, `getCapNames`) with the region's
  capabilities `caps` {`inv`, `lib`} (absent = empty), the AIS pool holding
  `pending` commands; the procedure it enqueues runs at once on a recording
  HTTP adapter that returns `response` (default: an empty successful map),
  then through the same coroutine. `idle` runs the idle callbacks afterwards
  with `idle_pending` in the pool (`AISAPI::onIdle` drains postponed
  commands).

`same_frame` keeps a step in the frame the previous step ended in
(otherwise the main loop moves to the next frame first); `disconnected` sets
`gDisconnected`; `callback` false runs without a completion callback.

**Hooks** script what other main-thread code does while the update is
suspended. The harness tells the parse from the apply phase exactly: the one
call of `AISUpdate::doUpdate()` passes an extra argument whose conversion
marks the start of `doUpdate()` (see "The harness"). Every `checkTimeout()`
tests the task timer first, so the harness counts the calls, whether or not
they yield; the test the `while` loop (bug 51) repeats after a yield is a
recheck, not a new checkpoint. A hook is one of:

* `{apply_checkpoint: k, actions}`: the k-th `checkTimeout()` in `doUpdate()`
  (1 = its start, then one per backlog flush in the create loops, the last at
  its end). **Fires in both runs**; used by the mandatory check.
* `{parse_checkpoint: k, actions}`: the k-th `checkTimeout()` before
  `doUpdate()` (one per `parseEmbedded()`). **Sched run only**.
* `{yield: k, actions}`: after the step's k-th yield, once the frame has moved
  on. **Sched run only**.

Actions: `set_category` / `set_item` (change fields of a model object),
`delete` (an id and its descendents), `run_step` (another AIS reply,
processed in full through the coroutine; nested hooks do not fire),
`construct_update` (another `AISUpdate` is constructed now: its constructor's
batch-timer logic runs), `advance_us` (the other code ran that long; sched
clock), `stop` (shutdown: at a checkpoint that `checkTimeout()` yields, in
either run, and the `LLCoros::checkStop()` after the yield throws
`LLCoros::Stopping`; after a yield the next `checkStop()` throws). The
update ends where it is, recorded as `abort` with `ops_after_stop` and
`model_unchanged`.

## How the corpus is made

1. `tools/inventory-fixtures/ais-update/gen_cases.py` (vkstorm-core-rust)
   writes `cases/`. It is the normative description of every input (its
   Python is deterministic: dicts keep insertion order, evaluation is left to
   right); the inputs are committed, so the Rust side never has to
   regenerate them. Ids are patterned (`I(n)` = `a0000000-0000-4000-8000-<n>`
   items, `L(n)` = `b...` links, `C(n)` = `c...` categories, `E(n)` = `e...`
   assets), so the `std::map` order in which `doUpdate` applies objects is
   the order of the id text: items, then links, then categories. Sets 10 and
   12 draw from xorshift64* (Vigna; state `s` non-zero, `x ^= x>>12; x ^=
   (x<<25) mod 2^64; x ^= x>>27; s = x; return (x * 0x2545F4914F6CDD1D) mod
   2^64`; `below(n)` = `(next() >> 32) % n`), seeds `0x5EED0A15DA7E0010`
   (s10a) and `0x5EED0A15DA7E0012` (set 12, one stream for the 9 cases in
   order), recorded in the case files.
2. The viewer test `INTEGRATION_TEST_llaisupdate_parity`
   (`indra/newview/tests/llaisupdate_parity_test.cpp`,
   `llaisupdate_parity_stubs.{h,cpp}`, `tests/inventory/r3-update.cmake`)
   writes `expected/` in dump mode (`VKS_AISUPDATE_DUMP=<dir>`). Dump mode
   always fails (exit status non-zero), so it cannot pass for a replay.
3. `tools/inventory-fixtures/ais-update/regenerate.py <viewer> <exe> <libdirs>`
   generates and dumps into a temporary tree, replays that tree
   (`VKS_AISUPDATE_FIXTURES=<tree>`) and only if it passes replaces this
   directory in the viewer and in the crate.

`VKS_AISUPDATE_FULL=<dir>` also writes the full output of the large case (not
committed); `VKS_AISUPDATE_TRACE=1` prints every op as it is recorded. There
is no case filter.

## The checks

The test runs every case twice (below) in one process, case after case in
index order, and reports three separately named tut tests:

1. **`never: op lists and query streams`** (mandatory). Every case's `never`
   run, compared as binary LLSD: the op list, which holds the query stream
   (every `q.*` event, in order, with what it returned) between the model
   calls, and the final skeleton; the apply-phase hooks (interleaving inside
   `doUpdate()`, scripted shutdowns); the fixture tree (missing or
   unexpected files); the write tids (a failure names the case, the run and
   the op). It also asserts **the shutdown contract** in both runs: after a
   stop the update aborts, no callback fires, no model call follows, and the
   model is as it was at the stop (`ops_after_stop` 0, `model_unchanged`
   true). A difference reads `case <id>: never op <index> (<op>)/<field path>:
   expected <value> actual <value>`.
2. **`sched: yield points`** (relaxable). The `sched` runs, compared the same
   way: the `never` op lists plus the yields, the parse-phase and
   yield-keyed hooks, the batch-timer schedule.
3. **`drift guard: frozen copies and emulated functions`**:
   `tests/check_r3a_copies.py` (see "The harness").

**The split, as decided for the W3 port.** The port keeps the C++ applier,
`doUpdate()`, with its `checkTimeout()` call sites; the parse may move to
Rust and need not yield at the same points. So hooks used by the mandatory
check fire only at apply-phase checkpoints in `doUpdate()`; hooks at
parse-phase checkpoints (`parseEmbedded()` and the like) belong to the
relaxable `sched` check only. A port that parses in Rust may replace check 2
with a documented one (the same model calls, queries, callbacks and final
skeletons as `never`, the yields where its applier puts them) without
touching check 1, which stays byte-exact.
3. **`drift guard: frozen copies and emulated functions`**:
   `tests/check_r3a_copies.py` (see "The harness").

### The two runs

* `never`: the timers never report expiry. The op list has no yields but the
  one a `stop` hook forces at its checkpoint: the canonical order of the
  model calls.
* `sched`: the timers keep `LLTimer`'s exact arithmetic over the injected
  clock. The clock moves `call_us` (case file; default 100 us) at every
  recorded event except step and hook markers, and `frame_us` (16667) at
  every yield, plus `yield_extra_us[k]` after yield k. `others_us` (cycled
  over the yields) scripts concurrent AIS updates: after a yield another
  update is taken to have resumed first in the new frame, re-armed the
  shared batch timer with `AISUpdate::AIS_BATCH_EXPIRY_SECONDS` (read from
  the class at the first run) and run for that long.

## The harness

The test compiles the real `llaisapi.cpp` (included verbatim by the stubs'
translation unit after every header it uses, so the renames below reach its
own text only):

* `LLTimer` -> a class with `LLTimer`'s arithmetic over an injected
  microsecond clock (start() and reset() leave the expiry at 0,
  setTimerExpirySec() adds `(U64)(F32)(s * 1e6)`, hasExpired() is now >=
  expiry); `llcoro::suspendUntilNextFrame` -> a stub that records a `yield`
  and moves the injected frame counter (`LLFrameTimer::getFrameCount()`) on;
  `LLCoros` -> `checkStop()` that throws when a hook says so;
* `LLCoreHttpUtil` -> a recording `HttpCoroutineAdapter` (the six methods the
  commands bind record method, URL, body, Destination, headers and timeout,
  and return the step's result; `getStatusFromLLSD` and `HTTP_RESULTS` are
  the real ones); `LLCoprocedureManager` -> a pool whose pending count is
  scripted and whose enqueue records and runs the procedure at once;
  `gAgent` -> an agent whose region answers the step's capability URLs (and
  `getID()`).

Everything else is real or a stub of this harness:

* **The model.** `gInventory` is a recording stub over the scripted skeleton
  (see "The skeleton model").
* **The viewer inventory objects.** `LLViewerInventoryItem` and
  `LLViewerInventoryCategory` members the parse runs are frozen copies of
  `llviewerinventory.cpp` (constructors, `copyViewerItem`, `copyItem`,
  `copyViewerCategory`, both `unpackMessage(const LLSD&)`, `localizeName`,
  `setTransactionID`, `updateParentOnServer`, the link getters `getType`,
  `getName`, `getLinkedItem`, `getLinkedCategory`) and
  `LLInventoryModel::accountForUpdate` is a frozen copy of
  `llinventorymodel.cpp`. `getVersion`, `setVersion` and `fetch` are
  recording stubs. Every other virtual member throws if called (none is).
* **The drift guard** (`tests/check_r3a_copies.py`, run by check 3 on every
  build of the test; the target is rebuilt when the live sources, the guard
  or the fixtures change):
  * each copy is compared with the live text with backslash splices joined
    first, comments and `#if 0` blocks removed, whitespace collapsed outside
    string and character literals and literals compared verbatim; its
    signature, compared without whitespace, must occur exactly once, and
    the live region must lie outside every preprocessor conditional; the
    recorder's added lines (`// VKS-HARNESS`) must have one of the recording
    forms the script lists and no side effect (no assignment but to the op
    being recorded, no `++`/`--`, only listed calls); a `//` comment holding
    code punctuation inside a copy is flagged;
  * the live code the stub emulates instead of copying is fingerprinted
    (`tests/check_r3a_fingerprints.txt`): the `fetch()` gate, `getVersion`,
    `setVersion`, `getViewerDescendentCount`, the localisation dictionary's
    constructor, the `fetch` default argument, and in the model `getItem`,
    `getCategory`, `findCategoryUUIDForType(InRoot)`, `addChangedMask`,
    `addChangedMaskForLinks`, `collectLinksTo`, `updateItem` (its mesh skip,
    its refusal of a new item of no type, its Lost And Found and
    default-folder moves and their accounting),
    `updateCategory`, `onObjectDeletedFromServer`, `deleteObject`,
    `notifyObservers`, `fetchDescendentsOf`, the constructor, the
    declarations whose default arguments the recordings use,
    `LLCategoryUpdate` and `getChangedIDs`. A change fails until the stub is
    reviewed against it and the table updated; the region's enclosing
    conditionals are part of the fingerprint;
  * an identifier fence: every live line of `llviewerinventory.cpp/.h`
    touching the members the copies depend on (`mIsComplete`,
    `mTransactionID`, `mIsRunning`, `mIsFaulted`, `mOwnerID`, `mVersion`,
    `mDescendentCount`, `mFetching`, `mDescendentsRequested`) must lie in a
    checked copy, a fingerprinted region, or the reviewed allowlist
    `tests/check_r3a_fence.txt` (declarations, accessors and functions the
    harness never reaches); and no `#define` / `#undef` in those files or
    `llinventorymodel.cpp/.h` may name a fenced member or a copied,
    fingerprinted or recorded function.
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
* **Requests.** The default headers (Accept, Content-Type, the UDP port),
  retries and the transport belong to the real adapter and are not recorded
  here (survey 3 layer B, recorder R3b); recorded is what `llaisapi.cpp`
  passes to it. Write tids are checked (well-formed, not null, never seen
  before in the process: a failure fails check 1) and shown as `<tid>`.

## The op list

Every event, in order, is a map with `op`:

| op | fields | recorded when |
|---|---|---|
| `step` | `index`, `type` (command type or function name) | the harness starts a step (costs no time) |
| `q.getCategory` | `id`, `found`, `obj` (the model's category, all fields) | `gInventory.getCategory()` |
| `q.getItem` | `id`, `found`, `obj` | `gInventory.getItem()` |
| `q.getVersion` | `id`, `result` | `getVersion()` on a category of the model (not on the parse's detached copies) |
| `q.findCategoryUUIDForType` | `type`, `result` | `gInventory.findCategoryUUIDForType()` |
| `q.getCapability` | `name`, `result` | the region's capability lookups of a command function |
| `q.countPending` | `pool`, `result` | the AIS pool's pending count (enqueue and idle) |
| `accountForUpdate` | `category`, `delta`, `change_version`, `model` {`found`, `version`, `descendents` after} | `gInventory.accountForUpdate()` |
| `updateCategory` | `mask`, `obj` (the object passed, all fields), `exists`, `parent_known` | `gInventory.updateCategory()` |
| `updateItem` | `mask`, `obj`, `exists`, `parent_known`, `model` (what the stub did with it, below) | `gInventory.updateItem()` |
| `onObjectDeletedFromServer` | `id`, `fix_broken_links`, `update_parent_version`, `do_notify_observers`, `exists` (category / item / none) | `gInventory.onObjectDeletedFromServer()` |
| `fetchDescendentsOf` | `id`, `would_start` | `gInventory.fetchDescendentsOf()` (410 branch) |
| `setVersion` | `id`, `version`, `before` | `setVersion()` on a category of the model |
| `fetch` | `id`, `expiry`, `version`, `would_start`, `is_model_object` | `LLViewerInventoryCategory::fetch()`: `would_start` is the real gate (version unknown and the 10 s / 360 s request timer expired) |
| `updateParentOnServer` | `id`, `parent_id`, `restamp`, `is_model_object` | the lost-item server move |
| `msg.send` | `name`, `reliable`, `body` (binary) | the message `updateParentOnServer` sends |
| `enqueue` | `pool`, `name` | a command procedure enqueued on the AIS pool |
| `http` | `method`, `url` (`<tid>` normalised), `tid_ok`, `body`, `destination` (COPY), `headers`, `timeout` | a request the adapter would send |
| `postponed` | `count`, `idle_registered` | after each command call: the postponed queue and the idle callback |
| `idle` | | the idle callbacks run |
| `isAvailable` / `getCapNames` | `result` | those calls |
| `notifyObservers` | `changed` (size of the changed-id set before the flush) | `gInventory.notifyObservers()` called by AIS code (the 50-change backlog flushes and the final one) |
| `notify` | `name` | `LLNotificationsUtil::add()` |
| `yield` | `frame`, `clock_us` (relative to the run), `others_us`, `extra_us` | `llcoro::suspendUntilNextFrame()` |
| `hook` / `hook_end` | `at`, `index` | a scripted hook runs (its nested ops between) |
| `checkStop` | `throws` | a scripted shutdown |
| `abort` | `what`, `ops_after_stop`, `model_unchanged` | the update ended by `LLCoros::Stop`: the ops recorded after the throw (must be 0) and whether the model still equals its state at the throw (must be true) |
| `callback` | `id` | the completion callback |
| `exception` | `what` | a C++ exception escaped a step (none does; check 1 fails on one) |

Objects are dumped raw (no link indirection): items `{item_id, parent_id,
asset_id, type, inv_type, flags, name, desc, created_at, thumbnail_id,
favorite, runtime, permissions {creator_id, owner_id, last_owner_id,
group_id, is_owner_group, base_mask, owner_mask, group_mask, everyone_mask,
next_owner_mask}, sale_info {sale_type, sale_price}, complete,
transaction_id, running, faulted}`; categories `{category_id, parent_id,
name, type_default, type, thumbnail_id, favorite, owner_id, version,
descendents, fetching}`. Enum values and masks are raw integers (U32 as
signed 32-bit). `final` is the skeleton after the run: every category and
item by id, and the stub's changed-id set.

## The queries the parser makes (the interface the Rust side needs)

Recorded (appear in the op list, compared by check 1):

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
version, descendent count, the fetch request timer, the transaction id);
the category's descendent count (stale check, copy back); the parent id of
removed objects (delta bookkeeping); the size of the model's changed-id set
after each create (the 50-change flush). Steps 3 and 8 read the model
*after* earlier ops of the same update were applied, and after whatever ran
while the update was suspended (set 13): a port applies the op list in
order with those two reads done at apply time.

## The skeleton model

`gInventory` keeps the skeleton and emulates the effects of the calls on it,
so that later queries of the same update (and later steps) see what the
real model would show: `updateCategory` (copy into the existing category, or
add a new one; changed id), `updateItem` (copy or add; changed id; mesh,
GLTF and GLTF-binary types skipped, link types resolved first as the real
`getType()` does; a new item of no type (<= -1) refused before anything
else, as `addItem()` refuses it: not stored, no move, no accounting, no
changed id; a new item with a null parent goes to its type's default
folder, one with `CATEGORIZE_LOST_AND_FOUND_ID` or an unknown parent folder
to Lost And Found, an existing item updated with a null parent keeps its old
parent or goes to Lost And Found, each move accounted on the destination
folder (+1 and a version bump) as the real model does), `onObjectDeletedFromServer`
(descendents removed, parent accounting when asked, the deleteObject
changed-id and flush sequence), `accountForUpdate` (the frozen copy),
`notifyObservers` (clears the changed-id set), `fetch` (the request timer
gate), changed-id bookkeeping (the referent, plus links pointing at it when
LABEL is set).

**`final` and the `model` notes are the stub's state, not the real
model's.** What the stub does not emulate:

* the real descendent count comes from the model's parent-child arrays; the
  stub counts the skeleton objects whose parent is the folder. They differ
  where the real model does not link a child into its parent's array, e.g.
  a new category whose parent is created later in the same update (survey 3
  §6 bug 12; `parent_known` false shows where), or a skeleton built from a
  cache with missing arrays;
* the real model's own server moves inside `updateItem` (`..._server_move`
  notes: it sends `MoveInventoryItem` there; survey 2 and R2's corpus own
  that), calling-card name lookups, the marketplace hooks, the observers,
  `mLastItem`, the backlog sets of a nested `notifyObservers`;
* `updateItem`'s computed masks beyond the changed-id bookkeeping.

The skeleton's own `descendents` default to its child count, so accounting
applies unless a case sets a mismatch.

## Coverage

### Survey 3 §8.1

| set | survey case | cases |
|---|---|---|
| 1 | fetch children depth 0: full, no `links` key, COF links only, outfit links only | s01a-s01g (plus a plain folder with links only, a known folder refetched, a category without `agent_id`) |
| 2 | recursive depth 50, 52-level tree | s02 (53 levels, each with empty `categories`/`links`/`items` maps but the next level: versions on levels 0-50, not 51-52) |
| 3 | subset fetch: parent not applied, children at depth 49 | s03a-s03c |
| 4 | links with `_embedded` item and category, broken link | s04a-s04d, set 14 |
| 5 | stale version (FIRE-33337), higher, -1 | s05a-s05i (also missing version, viewer unknown, stale twice in a frame, stale on a write, request depth -1) |
| 6 | fetch item, fetch link, null parent (server move), unknown parent, mesh/GLTF | s06a-s06h (also no Lost And Found, lost link, every item field) |
| 7 | orphans | s07, s16d |
| 8 | create category, create links, slam, update item, update category with `_updated_categories`, remove item and category, every removed list, `_updated_category_versions` higher / lower / equal / -1 / unknown id | s04b, s08a-s08n, the callback matrix s08z_* (every command type x six result shapes, and no callback) |
| 9 | 4xx map with `item_id`, 410 remove item / category, 403 depth 0 and 50, non-map body, empty body | s09a-s09o (also 499, 500, 4xx non-map, disconnected, create failure, error bodies that parse as objects, transport failure) |
| 10 | 20,000 items, 2,000 folders, recursive | s10a (digests and samples), s10b-s10c (the batch timer), s10d-s10l (limits) |
| 11 | duplicate keys in XML maps | s11a-s11e (`version`, an item id, `_embedded`, meta keys, a `_updated_category_versions` entry) |
| 12 | (added) random combinations | s12_random_00-08 |

### Added after the V0 audit

| set | what | cases | audit finding |
|---|---|---|---|
| 13 | interleaving: a COF update racing a newer write (the copy back reads the model after the yield); a COF fetch racing a newer write (the parse-time snapshot applied over the newer version: the window FIRE-33455 names); an item snapshot that goes stale; the updated folder deleted during the yield; shutdown at the first yield and mid-apply; another update constructed in the new frame before this one resumes | s13a-s13g | V15, T18, T2/T3/T17 |
| 14 | CREATEINVENTORY and SLAMFOLDER with a folder link whose embedded category is known, unknown, or listed in `_created_categories` | s14_* | F5 |
| 15 | partial item maps over current values (fetch, update, link), transaction ids in the skeleton | s15a-s15c | P15 |
| 16 | depth cut: a folder link at depth 0 whose embedded folder gets a version; writes creating outfit / COF-typed folders with links only; the default depth 50 (orphans, link fetch) reached at exactly 0 and -1 | s16a-s16e | D7, D13, D15 |
| 17 | loose typing: versions as strings and reals (and a non-numeric string), `_updated_category_versions` values as a string and a real, non-UUID map keys on a fetch and on a write, a COF map without `type_default` | s17a-s17h | V13, V14, P21, P22 |
| 18 | the 16 command functions (and `isAvailable`, `getCapNames`) with both, one or no capability and with no region; replies carrying ids; the pool limit | s18a-s18g | U1, U2 |
| - | drift guard: fingerprints and fence | check 3 | X1, X3 |

### Added after the V1 re-audit

| what | cases | finding |
|---|---|---|
| the shutdown contract in the mandatory check: a stop at the start of `doUpdate()` and at its first backlog flush, in both runs (no callback, no model call after the stop, the model unchanged); a stop at a natural parse-phase yield (sched) | s13i, s13f, s13e | N12, N13, N13b |
| apply-phase interleaving: hooks at the create-folders flush and the create-items flush change a folder that the same reply updates and names in `_updated_categories` and `_updated_category_versions` (the copy back must re-read the count after the first, the versions loop must re-read the version after the second) | s13j | N1, N2 |
| the COF fetch race at an apply-phase checkpoint (mandatory; s13b keeps the parse-phase one) | s13h | V15 (mandatory) |
| write-path null parents: a created item with a null parent (default folder, accounted), an updated item with a null parent whose old parent is null (Lost And Found, accounted) or known (kept, not accounted), a created item with `CATEGORIZE_LOST_AND_FOUND_ID` (Lost And Found, accounted, no server move); the stub's results agree with R2's real-model steps `null_parent`, `categorize_lost_and_found`, `null_parent_old_parent_known` (codex/rust-inventory-r2 3c0b7bddda) | s15d-s15g | N16, N17, S1, S2, S3, S6 |
| 410 replies to RemoveCategory and PurgeDescendents through the command functions | s18g | N6, N8b |
| bug 51: the batch budget already spent when the update resumes | s10m | (fix) |
| drift guard: splices, conditionals, re-spaced signatures, harness side effects, macros | check 3 | drift probes SC2a-d, SC3c-e, SC4, SC5 |

### Added for the inventory fixes (PR #181)

| what | cases | fix |
|---|---|---|
| a write creating items of no asset type (-1), one in a known folder and one with a null parent, next to a notecard: the parse leaves the typeless item out of Objects' count (`accountForUpdate` +1 there, finding 9 fixed) and fires a callback for each of the three created ids; the model refuses both typeless items before any parent handling (no default-folder lookup or move, no accounting of its own, not stored, no changed id) and adds the notecard (one changed id) | s15h | bug 23, finding 9 |

### Limits, at limit-1, limit and limit+1

| limit | cases |
|---|---|
| `MAX_UPDATE_BACKLOG` 50 (flush when the changed-id set holds more than 50) | s10d (50 ids), s10e (51), s10f (52) |
| task timer 8 ms, armed at construction (expired when now >= expiry) | s10g (first checkpoint at 7999 us), s10h (8000), s10i (8001) |
| batch timer 10 ms, re-armed by another update | s10j (9999 us), s10k (10000), s10l (10001); s10m (already spent: the loop yields again) |
| `MAX_FOLDER_DEPTH_REQUEST` 50 (non-recursive depth clamped) | s18a: depth 49, 50, 51 |
| depth cut (version only at depth >= 0) | s02, s16d, s16e (levels at depth 1, 0, -1) |
| `MAX_SIMULTANEOUS_COROUTINES` 2048 | s18f: pending 2047 (enqueued), 2048, 2049 (postponed); idle drains |

Counts: 140 cases (9 in set 12), 389 steps (153 of them command-function
calls), 11 hooks, 280 runs. Committed per tree: 2.76 MB of inputs, 6.53 MB of
expected outputs, 9.33 MB in all (671 files).

### The large case (s10a)

`FETCHCATEGORYCHILDREN` at depth 50 answered with a 10-ary tree of 2,000
folders (all three `_embedded` keys present, so every count and version is
known), 20,000 items (10 per folder, kinds from the seeded stream; about one
in eleven is a mesh, which the model skips) and 500 links. Its full output
(45,411 ops in `never`) is about 62 MB as binary LLSD, so it is committed as
`expected/s10a_large_recursive.digest.llsd`: `sha256` of the whole binary
output, and per run `ops_sha256` and `final_sha256` (SHA-256 of the binary
LLSD of the op array and of the final skeleton), `op_count`, `counts` per op
kind, `head` (first 60 ops), `tail` (last 40), `events` (every op that is
not a query or an update, with its index: the observer flushes, the yields,
the callback), `chunks` (`{first, last, sha256}` of the binary LLSD array of
ops `[first, last]`, 2,000 ops each: a mismatch names the op range),
`final_categories`, `final_items`. The test recomputes the whole structure
and compares bytes; every risk the large case touches also has a readable
named case (backlog flushes s10d-f, yields s10b-c and s10g-l, depth s02).

## For the crate

The crate replays the cases in index order. The 403 depth-0 notification is
chosen by a function-static flag of `InvokeAISCommandCoro` (the first 403 at
depth 0 in the process gets `InventoryLimitReachedAISAlert`, every later one
`InventoryLimitReachedAIS`); a Rust replay passes that flag as explicit state
through the corpus, starting false, set by the first 403 at depth 0 of
either run (each case runs `never` then `sched`). So s09e's `never` run
records the alert and its `sched` run only the plain one.

## Runtime edges

* **Process order.** Above: the test runs every case in index order in one
  process; there is no filter.
* **Static batch frame.** `AISUpdate::sBatchFrameCount` and `sBatchTimer`
  are statics that outlive a run; each run starts in a frame no earlier run
  used, so the first update of a run always restarts the batch timer.
* **Hooks and nesting.** A `run_step` hook processes another reply in full
  inside this one's checkpoint or yield (its yields advance the frame in the
  sched run); the harness cannot interleave two coroutines finer than that.
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
fixtures with 0 differences in all three checks. Nothing recorded depends on
floating point beyond the timers' `(U64)(F32)(seconds * 1e6)` (8000 and
10000 ticks) and LLSD's real-to-integer conversion of s17b/s17c (7.0 and
7.9 read as 7). The PR #181 re-recording (s15h added, every other output
unchanged but its header) was dumped and replayed on Windows only; s15h has
not yet been replayed on Linux.

## Mock fidelity

The shapes of real AIS replies at the depth limit and of 403 / 410 error
bodies are not known from the viewer alone (survey 3 §8.2, §10). Those cases
(s02, s03c, s05i, set 9, s16) are built from what the viewer's parser and the
HTTP adapter accept: results carry `http_result` as
`LLCoreHttpUtil::HttpCoroHandler` writes it, a 4xx map body becomes the
result, a 4xx non-map body goes under `content`, 5xx and transport failures
leave an empty map, an unparsable LLSD body becomes status 499. They record
what the C++ does with such a reply, not that AIS sends it.

## Findings (recorded as they are, not fixed)

1. (Fixed by bug 15, recorded on the fixed code.) The constructor used to
   call `setTimerExpirySec()` and then `start()`, which cleared the expiry,
   so every update yielded at its first `checkTimeout()`. Now a reply
   yields only once its 8 ms (or the frame's shared 10 ms) are spent; the
   batch timer causes a yield when another update resumed (s10b, s10j-m) or
   was constructed (s13g) first in the frame.
2. A fetch that answers a lower version than the model's is applied (version
   goes down) when the model's descendent count is unknown (s05b): the
   FIRE-33455 "versions only raise" rule covers `_updated_category_versions`
   only.
3. The snapshot-then-yield window remains for fetches: a COF fetch whose
   reply was parsed before a newer write landed applies its parse-time COF
   (version and count) over the newer one (s13h at the start of
   `doUpdate()`, s13b during the parse: v13 back to v12, count 3 back to
   2). Writes are safe: the copy back reads the model at apply time
   and the version rule only raises (s13a).
4. A stale parse-time item snapshot is written back over changes made while
   the update was suspended (s13c).
5. REMOVEITEM 410 deletes the item with the default
   `update_parent_version=true`, bumping the parent's local version (s09b;
   survey 3 §6 bug 14).
6. Error results are parsed: a 4xx map body with `item_id` reports success to
   a FETCHITEM caller (s09a), one with `category_id` and `parent_id`, or a
   full item, is applied to the model (s09m, s09o).
7. Loose typing: a version given as a non-numeric string reads as 0, which
   takes the stale path and refetches the folder (s17d); embedded entries
   keyed by a non-UUID still apply on a fetch (s17f).
8. The `LL_ERRS("unpack failed")` crash after `unpackMessage` (survey 3 §6
   bug 3) cannot be reached from LLSD input in this tree:
   `LLInventoryCategory::fromLLSD` always returns true, and
   `LLInventoryItem::fromLLSD` returns false only when
   `LLSaleInfo::fromLLSD` does, which it never does.
9. Fixed (approved 2026-10-28). Since bug 23, a created item of no asset
   type was refused by `updateItem()` but already counted by the AIS parse:
   `accountForUpdate` raised the folder's expected descendant count for it,
   so the folder expected more descendants than it held, the kind of count
   mismatch that makes the viewer refetch a folder (compare ais/fetch
   finding 6). `parseItem()` now leaves such an item out of the count (s15h:
   Objects +1, for the kept notecard). Its completion callback still fires:
   the id comes from the server's `_created_items`, the server did create
   the item, and callers look the id up in the model like any other.
