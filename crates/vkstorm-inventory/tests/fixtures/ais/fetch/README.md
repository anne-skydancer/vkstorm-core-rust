# ais/fetch: the inventory fetch wire oracle (recorder R3b)

Survey 3, section 8.2 (layer B): what the viewer sends to an AIS and
legacy-capability server, request by request, through the login fetch and
the scenarios around it, and what it does with the replies. Recorded from the
unmodified C++ (after the approved F1 fixes: LL #5358, FIRE-33455, the cache
save check, gunzip) so that the port of the scheduler and of the request
building to Rust (K5, K10, W3, W4) can be checked against it: the same test,
with the production code behind it changed, must stay at zero differences.

Inventory is mission critical: in this area a wrong port multiplies server load
on large inventories, stalls fetching for good, or writes to the server's
folders. The request counts are therefore the first line of every scenario.

This directory is identical, byte for byte, in the viewer
(`indra/newview/tests/fixtures/inventory/ais/fetch`) and in the crate
(`crates/vkstorm-inventory/tests/fixtures/ais/fetch`). Its `.gitattributes`
(`* -text`) keeps git from converting line ends. The files are never edited by
hand: change the test or the mock and regenerate.

## The code under test

`INTEGRATION_TEST_llinvfetch_parity` (`indra/newview/tests/inventory/r3-fetch.cmake`)
compiles the real `llaisapi.cpp`, `llinventorymodelbackgroundfetch.cpp`,
`llinventorymodel.cpp`, `llviewerinventory.cpp` and `llinventoryobserver.cpp`
with recorder R2's stubbed viewer environment
(`tests/llinventorymodel_parity_{stubs,harness,generic}.*`, used read-only, with
`VKS_INVMODEL_STUB_FETCH=0` so the real AIS and background fetch replace R2's
stubs), on the real coroutines (`LLCoros`), coprocedure pool
(`LLCoprocedureManager`, pool "AIS"), `HttpCoroutineAdapter` and llcorehttp
(the Rust transport of rung four). Nothing in the production code is changed.

What cannot be linked is copied verbatim into the test
(`tests/llinvfetch_parity_support.cpp`, checked by `tests/check_r3b_copies.py`,
which test 1 runs on every run):

* `llstartup.cpp`: the inventory part of `STATE_INVENTORY_SEND` (library root
  and owner), `STATE_INVENTORY_SKEL` (the two `loadSkeleton` calls),
  `STATE_INVENTORY_SEND2` (`buildParentChildMap`, `InventoryUnusable`,
  `start()`, `createCommonSystemCategories`), `STATE_INVENTORY_CALLBACKS`
  (`start()`, the COF id, the COF marked `FETCH_RECURSIVE`), and the
  `findLostItems()` call of `STATE_STARTED`. `do_startup_frame()` is a no-op
  in the test (it drains the UDP socket and draws).
* `llappearancemgr.cpp`: `callAfterCOFFetch`, `callAfterCategoryFetch`,
  `callAfterCategoryLinksFetch`, `CallAfterCategoryFetchStage1`'s constructor
  and `startFetch()`, `LLAppearanceMgr::getCOF` and `initCOFID`. The stage-1
  observer's `done()` records `cof.stage1.done` and calls the callback instead
  of starting stage 2 (wearing). `nullary_func_t` is a type of the test's own
  (a `std::function<void()>`) so the copies' unqualified calls resolve to the
  copies without editing them.

The check compares literals verbatim, strips comments and `#if 0` blocks,
requires exactly one match per region, flags a statement folded onto a `//`
line, and fences the identifiers the copies depend on (`mCOFID`, the stage-1
class, the COF helpers, the startup inventory calls and login-response keys):
every live line that mentions one lies in a copied region or in a commented
allowlist.

## How a scenario runs

* **The mock** (`tests/vksinventory_mock_server.py`) runs the test executable.
  It binds `127.0.0.1:0` in its main thread before anything else (a failure
  exits 2; there is no readiness wait), exports the port as `LL_TEST_PORT` and
  its request log (a temporary file of its own unless `--log` names one) as
  `VKS_HTTP_MOCK_LOG`. It logs every request (arrival order, connection, the
  exact head, the body, the status, the response) as JSON lines, writing each
  record before sending the response. The test fails when the mock does not
  answer; it never skips.
* **The world.** Each scenario builds a synthetic inventory (below), writes
  it to the mock (`POST /control/world`) and loads the matching login skeleton
  through the frozen login steps. The region (R2's shim) advertises the
  capabilities the scenario chooses, all pointing at the mock:
  `InventoryAPIv3` `/cap/inv`, `LibraryAPIv3` `/cap/lib`,
  `FetchInventoryDescendents2` `/cap/fid2`, `FetchLibDescendents2` `/cap/fld2`,
  `FetchInventory2` `/cap/fi2`, `FetchLib2` `/cap/fl2`,
  `CreateInventoryCategory` `/cap/cic`.
* **Frames.** One frame is what `LLAppViewer` does each frame in this area:
  `LLFrameTimer::updateFrameCount()`, the idle callbacks (the background
  fetch, `AISAPI::onIdle`), `gInventory.idleNotifyObservers()`, the
  `"mainloop"` post (the HTTP pumpers, coroutines waiting for the next frame),
  then two fiber yields.
* **The fake clock.** `LLFrameTimer`'s frame time (the category fetch timers:
  `fetch()`'s 10 s, `setFetching`'s 180 s / 30 s / 60 s, the 360 s of a -1
  version) moves only when a scenario says so (`clock_advance`), from 1000 s
  at the start of each scenario. `LLTimer`'s total time (the background
  fetch's 1 s / 6 ms frame budget) is frozen: the test sets the
  clock-to-microsecond factor of llcommon's `TimerInfo` to 2^-20, so a real
  second moves it by about 10 us (QueryPerformanceCounter) or 1 us
  (gettimeofday). The budget therefore never ends a dispatch loop early, and
  the 1 s / 6 ms choice by startup state is not observable (see "Not
  covered"). `LLTimer`'s expiry timers (the AIS update's 8 ms task and 10 ms
  batch timers) read the raw clock and stay real-time.
* **Settling.** After an action the test runs frames until nothing is in
  flight: the AIS pool empty, the background fetch's request count zero,
  every request the mock received answered, every legacy item handler done
  (counted through the window's busy count), and nothing changing for 8
  frames and 150 ms (a foreground item request has no counter the test can
  read: the quiet time covers its trip to the mock).
* **Pools and modes.** Sequence scenarios (`s..`) run with `PoolSizeAIS` 2:
  a 2-coroutine AIS pool and a scheduler limit of `PoolSizeAIS - 1` = 1, so
  at most one fetch is in flight and the request log is a total order.
  Concurrency scenarios (`c..`) recreate the pool with 20 coroutines (limit
  19). The pool is created once per size by
  `LLCoprocedureManager::setPropertyMethods`, as the viewer does.
* **Settings** are pinned per scenario: `BatchSizeAIS3` 20, `FSUseAis3Api`
  true (s10b: false), `UseHTTPInventory` true, `DebugAvatarAppearanceMessage`
  false, `InventorySortOrder` 7. Grid: Second Life (R2's default) except
  s10b.

### Recording modes

| mode | used by | request log | journal | model |
|---|---|---|---|---|
| SEQ | s01-s09, s14-s17 | `wire <n>` in arrival order | every line, in order, with a scheduler snapshot (`sch`) after every change | full dump |
| PHASED | s10a, s10b, s12, s13, c04, c11 | `wire@<phase>` sorted within each phase (phases end at `step` lines) | sorted within each phase; `sch@step` snapshots only | child lists sorted |
| CONC | c01, c02, c03, c10 | `wire* <k>x` multiset | only `msg`, `notify`, `callback` lines, as a multiset | category lines without fetch state and timer; child lists sorted; no lock maps |
| large | s03, c03 | as its mode | digest | digest |

PHASED exists because the legacy capability scheduler keeps several POSTs in
flight on its own (up to 12 + 4, hard-coded): their arrival order at the mock
and the order their replies are processed in depend on timing. Each phase is
built to hold as few requests as possible. A step in PHASED mode first waits
until no request has arrived for 300 ms (no frames run meanwhile), so a request
issued before the step is counted in its phase.

## Files

| path | what |
|---|---|
| `index.txt` | the scenario ids, in run order |
| `manifest.txt` | `<scenario> <file> <sha256>` of every file (the digests cover the header lines) |
| `scenarios/<id>.txt` | the record: a `#` header (generator, viewer commit, platform), then `== <id>` and one line per item below |
| `responses/<id>.txt` | the response bodies of a SEQ scenario, normalised as the request log, in request order (`--- <id> <n> <method> <target> status=<s> len=<bytes>`, then the body) |

### Lines of a scenario file

| line | meaning |
|---|---|
| `count requests=<n> caps=<cap>:<n>,... methods=<m>:<n>,... status503=<n>` | how many requests reached the server, by capability and method; 503s count each retry llcorehttp sent |
| `wire <n> <METHOD> <target> status=<s> head=<head> body=<body> resp=<len>:<sha16> rtype=<content type>` | one request: the exact request head (CR LF spelled `\r\n`) and body, the status the mock answered, the response's length and the first 16 hex digits of the SHA-256 of its normalised body |
| `wire@<p> ...`, `wire* <k>x ...` | the same in PHASED (phase p) and CONC (k identical requests) |
| `tids uses=<n> distinct=<n>` | `tid=` query values seen (each appears in the target and the head) and how many distinct ones: every AIS write must carry a fresh one |
| `jrn step <name> requests=<n>` | a scenario step, with the requests received so far |
| `jrn sch <state>`, `jrn sch@step <state>` | the background fetch's private state after it changed: `act=<bg active><folder active> st=<inventory started><library started><marketplace started><all fetched> fc=<mFetchCount> ffc=<mFetchFolderCount> fq=[<id>:<EFetchType>,...] iq=[...] exp=[<expected folder ids>] force=[<forced set>]` |
| `jrn batch* mask=<hex> changed=[...] added=[...]` | the union of consecutive observer notifications (R2's recorder) |
| `jrn market* [<id> <consistency> <skip_clear>,...]` | the union of the `update_marketplace_category` calls `addChangedMask` makes in the same run of lines |
| `jrn msg <name> <hex>` | a UDP message the viewer sent (`gAgent.sendReliableMessage`, R2's stub: the template builder's bytes, 6 zero header bytes first) |
| `jrn notify <name> [args]` | a notification |
| `jrn callback <tag> <id>` | a completion callback the scenario registered (`LLInventoryCallback` or `AISAPI::completion_t`) |
| `jrn fetch.completed` | the background fetch's completion signal (`setFetchCompletionCallback`) |
| `jrn frame <label> count_before=<n> count_after_idle=<n> pool=<n>` | one frame of a limit scenario: the background fetch's request count before and after the idle callbacks, and the AIS pool's queued + active count; deterministic because the mock holds every reply for seconds |
| `jrn world ...`, `jrn startup ...`, `jrn cof.stage1.done ...`, `jrn observer <tag> done ...`, `jrn in flight=<n>`, `jrn items queued ...` | scenario notes |
| `pub ...` | the public predicates of `LLInventoryModelBackgroundFetch` at the end |
| `mdl ...` | R2's model dump (`vks_inv::dump_model`): every folder and item with every field, the fetch state and request timer of each folder (`timer{...}` against the fake clock), child lists in order, backlinks, broken links, lock-map sizes, the pending change batch |
| `jrn-digest`, `jrn-chunk <k> lines=<a>-<b> sha256=...`, `jrn-line <n> ...` (and `mdl-...`) | large scenarios: the SHA-256 of the whole block (lines joined with `\n`), one per chunk of 500 lines (a mismatch names the line range), and the first and last lines with their line numbers as samples |
| `bound viewer_in_flight<=19 yes`, `bound legacy_in_flight<=16 yes`, `bound ais_pool<=20 yes` | CONC: the limits held |
| `# measured ...` | CONC: maximum in flight, dispatches per frame, pool size, overlap at the mock, frames (timing-dependent: not compared) |
| `FAILURE ...` | the scenario could not run as written (never present in a committed fixture) |

### Normalisation

* the mock's port: `127.0.0.1:{PORT}`; the UDP listen port header:
  `X-SecondLife-UDP-Listen-Port: {UDPPORT}`;
* every `tid=<uuid>`: `tid={tid}` (their count and distinctness are recorded);
* any other UUID that is neither one of the world's ids nor patterned
  (`<8 hex>-0000-4000-8000-<12 hex>`): `{gen1}`, `{gen2}`, ... in order of first
  appearance in the scenario (the viewer's `LLUUID::generate`: only the
  `CreateInventoryCategory` folder id in s14). Lines holding one are re-sorted
  where their position followed the id's value (model blocks, id lists);
* consecutive observer batches and marketplace calls are united (above). An
  AIS update that runs past its 8 ms real-time budget yields to the next frame
  and the model's idle notification then flushes the changes made so far as a
  batch of its own; where batches split, and so how often the marketplace hook
  fires for an id changed again after a flush, depends on timing. Their union
  between two other journal events does not.

## The synthetic inventories

Ids are `vks_inv::pid(tag, n)`: `<tag as 8 hex digits>-0000-4000-8000-<n as 12
hex digits>`. Tags: root `c0000000` (n 1); system folder of type t `c0000001`
(n = t + 2); user folders `c0100000` (n counts up in creation order, shared by
outfits and listings); items `a0000000`; links `b0000000`; library root
`d0000000` (n 1) and library folders `d0000000` (n from 2); library items
`d1000000`; the orphan `a0f00000` (n 1); assets: the item's id with its first
byte `e8`. The agent is `a9e00000-...-1`, the session `5e550000-...-1`, the
library owner `ba2a564a-f0f1-4b82-9c61-b7520bfcd09f`. Folder versions are
`1 + (n * 7) % 13` (system folders: n = t + 2; root 30; library root 12).
Ids beginning `f3`, `f4`, `52`, `53`, `5e` or `e0` script the mock (below).
Mock-created ids: categories `c1000000` (create) and `c2000000` (copy), links
`b1000000`, copied items `a2000000`, numbered by one counter per world.

`basic(user_folders, orphan, lib_folders)` (`llinvfetch_parity_scenarios.cpp`),
in this order: the root ("My Inventory", type 8) with the 21 system folders
(types 0 1 2 3 5 6 7 10 13 14 15 16 20 21 23 46 48 50 53 56 57, in that order);
the library root ("Library", type 8, owner the library owner) when there is a
library; items: 2 textures, shirt (flags 4), pants (5), jacket (8), shape (0),
skin (1), 2 objects, a notecard, a script, a landmark, a gesture, an animation,
a snapshot, 2 in Trash, 2 directly in the root; per user folder u: "Folder u"
under the root with 3 items (notecard, object, notecard), two children
"Folder u.s" with 2 items each and one grandchild "Folder u.s.1" with 1 item;
"Outfit A" (type 47, links to shirt, pants, shape) and "Outfit B" (a link to
the jacket) under My Outfits; the COF with links to shirt, pants, shape, skin
and a folder link to Outfit A; two marketplace listings with an object each;
in the library, 2 items in the root, then per library folder l: "Library l"
with 2 items and two children with 2 items each; the orphan (an object with
no parent). `large()` adds to `basic(0, no orphan, 3)`: 279 user folders under
the root (300 children in all), each with 4 children of 3 children, a chain 60
folders deep under the first, extra folders under user folders 2-5 up to 5,000
folders, 12 items per user folder (notecard, object, texture, script in turn)
and objects up to 60,000 items. Each scenario adds its own folders (see the
scenario table and the source).

The mock's world line format is documented in `vksinventory_mock_server.py`
(`World.load`): `F <id> <parent> <type> <version> <owner> <name>`,
`I <id> <parent> <owner> <creator> <asset> <type> <inv_type> <flags> <created>
<base> <owner_mask> <group_mask> <everyone> <next> <sale_type> <sale_price>
<name> <desc>`, `L <id> <parent> <owner> <linked> <type> <inv_type> <created>
<name> <desc>`, names percent-encoded.

## The mock's replies (mock fidelity)

The real AIS reply shapes at the depth limit, and the 403 / 410 bodies, are
not known from the viewer alone (survey 3 sections 8.2 and 10). The mock is
built from what the viewer accepts, consistent with recorder R3a's corpus
(`ais/update`):

* a category: `category_id`, `parent_id`, `name`, `type_default`,
  `agent_id`, `version`; with `_embedded` {`categories`, `links`, `items`}
  when its parse depth is >= 0 (the top of a request at the requested depth,
  each level below one less); a category one level beyond the requested
  depth comes without `_embedded` (so the viewer creates it without a
  version and fetches it later);
* a subset request (`children=`): the parent with `_embedded.categories`
  holding only the named children; `/links`: the folder with only
  `_embedded.links`; a link carries its target in `_embedded` (`item`, or a
  bare `category`), in `/children` too, as R3a's corpus does;
* items: every field (`item_id`, `parent_id`, `asset_id`, `type`,
  `inv_type`, `flags`, `name`, `desc`, `created_at`, `permissions`,
  `sale_info`, `agent_id`); `/orphans`: `_embedded.items` of the parentless
  items;
* errors: 403 with the LLSD map `{error_code: 403, error_description: "too
  many descendents"}` (as R3a), 404 with `{error_code: 404, ...}`, 410 with
  an empty body, 503 with a text body;
* writes change the mock's world and answer with `_created_categories`,
  `_created_items`, `_categories_removed`, `_category_items_removed`,
  `_updated_items`, `_updated_categories`, `_updated_category_versions`
  (the parent's or the folder's version plus one) and `_embedded` holding
  the created objects;
* legacy: `{folders: [{agent_id, owner_id, folder_id, version, descendents,
  categories: [{category_id, parent_id, name, type_default}], items: [...]}],
  bad_folders: [{folder_id, error}]}`; the null folder id (lost and found) is
  answered with the parentless items; an empty or undefined `folders` list
  with `{folders: []}`; items: `{agent_id, items: [...]}`;
  `CreateInventoryCategory` echoes `{folder_id, parent_id, type, name}`.

Scripted by the first two hex digits of the id in the path (AIS) or body
(legacy): `f3` 403 for a full children fetch at depth > 0; `f4` 403 for every
full children fetch, and a legacy request naming one; a subset at depth > 0
naming an `f3` or `f4` child is refused whole; `52` 503 for the first two GETs;
`5e` 503 with `Retry-After: 1` for a full children fetch at depth > 0, always;
`53` 503 for the first write; `e0` (legacy) a 200-with-error the first
`legacy_error` times. Modes set by a scenario: `fail_first=<n>`
`fail_status=<code>`, `fail_subset=<n>`, `delay_ms=<ms>`, `legacy_error=<n>`.

What these record is what the C++ does with such replies, not that the
servers send them. Request bytes do not depend on the mock.

## Scenarios and coverage

Request counts are those of the committed recording.

| survey 8.2 | scenario | requests | what it pins |
|---|---|---|---|
| 1 login_cold | `s01_login_cold` | 9 | COF (`/category/current/links`) and the base outfit (`/links`) first, root at depth 0, 2 subset batches (20 + 5, COF and Marketplace skipped), library at depth 50, Marketplace last (depth 0, then its subset), `/orphans` and the UDP `MoveInventoryItem` of the orphan; the logout cache |
| 2 login_warm | `s02a_login_warm_full`, `s02b_login_warm_partial` | 2, 4 | s01's cache: no folder of the walk requested, except Lost And Found (see findings); two folders changed on the server: each fetched at depth 50 (the whole subtree of the changed one, cached children included) |
| 3 large | `s03_large` (SEQ), `c03_large` (CONC) | 22, 20 | 5,000 folders, 60,000 items, 300 under the root: 15 subset batches, the 60-deep chain's follow-up at its 50th level; digests |
| 4 forbidden | `s04_forbidden` | 13 | a subset batch refused (403) -> its folders one by one at depth 50 -> 403 -> depth 0 (f3: served; f4: 403 and `InventoryLimitReachedAISAlert`, the first in the process) -> version 0, the children as subset batches |
| 5 transient_5xx | `s05_transient_5xx` | 18 | 503, 503, 200 (one logical request, two retries); 503 + `Retry-After: 1` six times (five retries, then the scheduler's depth-0 fallback); a PATCH answered 503 once is sent again |
| 6 outage | `s06_outage` | 10 | the first four requests fail (404, not retried): root and library root set to version 0; a forced fetch of the root inside the 60 s back-off sends nothing, after it one request (the root recovers); `start()` of a known folder nothing; the library root stays at version 0, its own items never fetched |
| 7 forced | `s07_forced` | 11 | two forced fetches during a normal one: one more request after it; forced G, H, G: two; normal schedule of a known folder: none; forced item twice, then again |
| 8 items | `s08_items` | 17 | incomplete item, `fetchFromServer`, a link (`linked_id`), a library item (`/cap/lib/item`), an unknown id (404), the item observer (per-item fetches, an unknown id), a forced complete item |
| 9 writes | `s09_writes` | 24 | create category / outfit (`?tid=`), links, slam (PUT `/links`), PATCH item (LLSD and object), PATCH category (three senders), DELETE item (also immediate, and 410), DELETE category (and 410), protected folder refused, purge (`/children`), library copy with `?tid=...,depth=0` and with subfolders, `Destination` header |
| 10 legacy_caps | `s10a_legacy_caps_sl`, `s10b_legacy_caps_other_grid`, `c10_legacy`, `c11_legacy_limits` | 19, 3, 17, 43 | the legacy scheduler on Second Life without AIS: root and library only, the COF observer, the lost-and-found entry (string folder id, `fetch_folders` false), 403 split (library halves to `FetchInventoryDescendents2`, an empty `folders` POST), 200-with-error re-issued until it succeeds, `bad_folders`, forced fetches; another grid with `FSUseAis3Api` off, UDP `FetchInventory`; up to 16 handlers |
| 11 udp | not covered | - | the adaptive UDP loop is `#ifdef OPENSIM`; this build has `OPENSIM` off |
| 12 region_change | `s12_region_change` | 7 | no region and capabilities not received: nothing sent; legacy only: the AIS-typed queue goes out as legacy POSTs; back to AIS with those replies in flight: nobody pumps them (see findings); caps without inventory caps: the queue drained silently; AIS back: nothing resumes until a UI fetch |
| fallbacks | `s13_sl_fetch_fallbacks` | 12 | Second Life without AIS: UDP `FetchInventoryDescendents` without caps, the legacy scheduler with `FetchInventoryDescendents2`, the COF observer blocked by the 180 s mark then served, the COF over UDP, `fetchFromServer` (foreground, with `agent_id`), the item observer (`cap_name` in the body), nothing without caps |
| write fallbacks | `s14_sl_write_fallbacks` | 7 | Second Life without AIS: `CreateInventoryCategory` (generated folder id), no capability (callback null), UDP `UpdateInventoryItem`, `UpdateInventoryFolder` (three senders), `LinkInventoryItem`, `RemoveInventoryFolder`; remove item, remove a folder with contents, purge: nothing sent |
| boundaries | `s15_batch_boundaries` | 15 | `BatchSizeAIS3` 20: 19 children -> one batch, 20 -> one full batch (re-queued, nothing left), 21 -> 20 + 1 |
| | `s16_depth_boundaries` | 8 | depth 50: chains 48 / 49 / 50 levels below a root child (subset path, children at 49): only the 50 needs a follow-up; 49 / 50 / 51 below the library root (direct path): only the 51 |
| | `c04_ais_limit` | 63 | 18, 19, 20 folders queued in one frame, replies held: 18, 19, 19 dispatched, nothing in the next frame |
| | `c11_legacy_limits` | 43 | batch 10: 9, 10, 11 folders -> 1, 1, 2 POSTs; threshold 12: one POST per frame while the count is <= 12 (1 ... 13), none at 13; 16: four POSTs per frame 0 -> 4 -> 8 -> 12 -> 16, none at 16 |
| re-entrancy, disconnect | `s17_no_library_cap_disconnected` | 6 | no `LibraryAPIv3`: every library command calls back null synchronously inside the scheduler's loop, through the whole fallback chain, without a request; `gDisconnected`: nothing sent, an AIS write calls back without a request; reconnected: the queued fetch goes out |
| concurrency | `c01_login_cold`, `c02_saturate` | 6, 27 | the cold walk on a 20-coroutine pool; a failed batch (20 folders) re-queued one by one: 19 in flight |

### Survey 3 section 10 risks

| risk | readable scenario |
|---|---|
| batching, depth 50, subset vs per-folder | s01, s15, s16 |
| dedupe (fetching state, forced set, known version) | s07, s10a, s06 |
| the "known version -> no request" rule | s02a, s02b |
| concurrency (19, the pool's 20, writes sharing the pool) | c02, c04 |
| the frame budget | not observable (frozen clock; see "Not covered") |
| the legacy unbounded re-queue | s10a (200-with-error: 4 POSTs for three errors) |
| retries | s05 |
| version accounting on writes | s09 (model and `_updated_category_versions`), s14 |
| a read turning into a write (`MoveInventoryItem`) | s01, s10a, s10b |
| the `_created_*` filter | R3a (`ais/update` s04b, s08n); here every write's embedded objects are created ones |
| the callback contract | s09, s14 (`callback` lines), s17 (null callbacks) |
| a fresh `tid` per call | s09 (`tids ... distinct=6`) |
| library copy and slam bodies | s09 |
| re-entrancy (no synchronous `emplace_front`) | s17 |
| completion counts, `FETCH_FAILED` 60 s | s06, s04 |
| path switching per frame | s12 |
| shutdown (`checkStop` mid-apply) | not covered (see below) |

## Findings (recorded as they are, not fixed)

1. **The legacy 403 split posts an empty request.** `processFailure` posts a
   chunk each time the accumulator reaches `size / 2`, so an even-sized
   request is sent as two halves plus a third POST with `folders` undefined;
   an odd-sized one in more than two pieces plus the empty one (s10a). The
   library folders of a split go to `FetchInventoryDescendents2` (survey bug 7).
2. **A switch from the legacy capabilities to AIS strands the legacy replies.**
   Legacy replies are pumped only inside `bulkFetch()`; while AIS is available
   it is never called, so the handlers stay alive, `mFetchCount` stays up and
   the AIS scheduler (limit `PoolSizeAIS - 1`) dispatches that many fewer
   fetches: with the sequence pool none at all, the background fetch stays
   active, until legacy capabilities return (s12).
3. **Capabilities without any inventory capability drain the queue.**
   `bulkFetch()` pops every entry and sends nothing when the region has
   neither `InventoryAPIv3` nor `FetchInventoryDescendents2`; the folders are
   not requested again by themselves when AIS returns (s12).
4. **The COF without AIS waits for the login mark.** `STATE_INVENTORY_CALLBACKS`
   marks the COF `FETCH_RECURSIVE` (180 s with AIS, 30 s without);
   `callAfterCOFFetch`'s `setFetching(FETCH_NONE)` stops the timer without
   clearing its expiry, so the observer's `fetch()` does nothing until it
   expires (s13, s10a). The same holds for any folder a subset batch marked:
   a UI `fetch()` within 180 s of the batch is refused.
5. **The legacy walk on Second Life does not descend** below network-fetched
   folders (survey bug 9): s10a fetches the root and the library root only.
6. **Lost And Found is refetched at every login after an orphan move.** The
   viewer moves the orphan into Lost And Found locally (and tells the server
   over UDP) without changing the folder's server descendent count; the cache
   then disagrees with the skeleton and the folder is fetched at depth 50 at
   the next login (s02a, s02b).
7. **Category `updateServer()` and `changeType()` PATCH `asLLSD()`**:
   `item_id` and `type` keys, not `category_id` / `type_default` (s09); item
   object updates send the legacy LLSD form (`flags` as binary, `inv_type` as a
   name).
8. **The forced set on the legacy path** keeps every forced id for the session
   (survey bug 8): s10a's second forced fetch of the same folder sends nothing.
9. **An outage leaves version 0**: a folder whose depth-0 fallback failed is
   marked version 0 and counts as fetched for the session; only a forced fetch
   after the 60 s back-off requests it again. In s06 the library root keeps
   version 0 and its own two items never arrive (survey bug 15).
10. **A changed folder is refetched whole**: s02b's changed folder is requested
    at depth 50, its unchanged cached subtree included.

## Runtime edges and determinism

* **Process order.** `InventoryLimitReachedAISAlert` is chosen by a
  function-static flag (first 403 at depth 0 in the process); s02a/s02b read
  the cache s01 wrote; the pool changes size once (s.. on 2, c.. on 20). The
  test always runs every scenario in `index.txt` order; a subset
  (`VKS_INVFETCH_ONLY`, only with `VKS_INVFETCH_DEV=1`) never passes as a
  replay.
* **Timing.** Everything compared is independent of timing by construction
  (above); what is not is under `#` (CONC measurements). No undefined
  behaviour is reached.

## Platforms

Recorded on Windows (MSVC 17, RelWithDebInfo, the Release feature set); Linux
(WSL Ubuntu 26.04, GCC 15) and GCC 14 (as on CI's Ubuntu 24.04) replay the
committed tree with zero differences. The request heads (rung four's Rust
transport) and bodies (rung two's LLSD XML) are the same on both platforms.

## Not covered

* The UDP background loop (survey 8.2 scenario 11): `#ifdef OPENSIM`, not in
  this build.
* The frame budget's effect (1 s before `STATE_WEARABLES_WAIT` ends, 6 ms
  after): with the clock frozen it never ends a dispatch loop; making it end
  one deterministically would need a clock that moves per call, which
  llcommon's timer does not offer without a production change. A Rust
  scheduler should take an injected clock and test the budget itself.
* `LLCoros::checkStop()` aborting an update half-applied at shutdown.
* `AISAPI`'s postponed queue (more than 2,048 commands pending).
* The real servers' reply shapes (see "Mock fidelity").

## Regeneration

`tools/inventory-fixtures/ais-fetch/regenerate.py <viewer> <test exe> <library
dirs...>` (vkstorm-core-rust) dumps into a temporary tree (dump mode exits
non-zero by design, and must report no failures), replays that tree
(`VKS_INVFETCH_FIXTURES`), and only then replaces this directory in the viewer
and in the crate, checking the copies byte for byte. By hand:
`python indra/newview/tests/vksinventory_mock_server.py -- <exe>` with
`VKS_INVFETCH_DUMP=<dir>`; `VKS_INVFETCH_ACTUAL=<dir>` makes a replay write its
output for diagnosis.
