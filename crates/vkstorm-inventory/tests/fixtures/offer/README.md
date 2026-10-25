# offer fixtures

Inventory offers: received offers decoded into `LLOfferInfo`, its LLSD form,
the accept and decline packets, the forced-response indexing, the offer
throttle, the offer policy tables, offline offers. Recorded by R4b of the
inventory rung (vkstorm-rust rung seven) from the unmodified viewer C++
(viewer base `472ecf270d`, `codex/rust-inventory`), on Windows (MSVC) and
Linux (GCC 15 and GCC 14). Never edit them by hand: change the recorder and
record again. Survey 4 section 7: the instant message part of set 4, sets 5
and 6. The give side (set 4 gives, set 7) is in `../give/`; one recorder, one
test, one format.

## How they are made

`INTEGRATION_TEST_llinventory_offer_oracle` (`indra/newview/tests/`,
registered by `tests/inventory/r4-offer.cmake`).

- The decision code is frozen verbatim in `llinventory_offer_oracle_copies.{h,cpp}`
  (49 compiled regions, each naming its source file, line range and blob) and
  compiled against the fakes in `llinventory_offer_oracle_fakes.{h,cpp}`.
  `llinventory_offer_oracle_guards.h` (never compiled) freezes, as text, live
  code the copies depend on: `process_improved_im` (the `U8[MTUBYTES]` bucket
  read, its size, the sender host), the `processNewMessage` preamble up to the
  dialog switch (antispam, name clean-up, mute, Omnifilter), the
  ReadOfflineMsgs coroutine before its message loop, `LLMuteList::add` and
  `remove`.
- `check_r4b_copies.py` runs as the test's last test. It compares each region
  line by line after removing comments and `#if 0` blocks, collapsing
  whitespace outside literals only (literals verbatim), requires exactly one
  match per region and the regions of `processNewMessage`, the offline loop
  and their guards in source order; it checks every element of the frozen
  notification forms and runs an identifier fence (every live `indra/newview`
  line touching an offer or give identifier lies in a checked region or in its
  reviewed allowlist).
- Every packet is built by the real message system from
  `scripts/messages/message_template.msg` and the real `pack_instant_message`;
  `LLUUID`, `LLSD`, `LLPermissions`, `LLInventoryItem`, `LLAssetType`,
  `LLFolderType`, `LLHost` and `LLCacheName`'s name helpers are real too.

```sh
# replay (the build runs this): the test with no environment
# write the fixtures (exits non-zero on purpose: a dump is never a pass)
VKS_R4B_DUMP=<dir> VKS_R4B_COMMIT=<viewer commit> INTEGRATION_TEST_llinventory_offer_oracle
# diagnosis: also write the sweeps' full canonical streams
VKS_R4B_FULL=<dir> INTEGRATION_TEST_llinventory_offer_oracle
# a partial dump needs the developer flag
VKS_R4B_DEVELOPER=1 VKS_R4B_ONLY=policy,offline VKS_R4B_DUMP=<dir> ...
```

`tools/inventory-fixtures/offer/regenerate.py` in vkstorm-core-rust dumps twice
into fresh directories, requires identical dumps, writes both this directory
and `crates/vkstorm-inventory/tests/fixtures/{offer,give}`, and replays.

The test's tests: 1 dump; 2 live `pack_instant_message` against its frozen
copy (3000 seeded cases); 3 replay of every full fixture, aligned by case id
(case, key, expected, actual); 4 every sweep's digest, chunk digests (the first
changed chunk names its case range), sample (aligned by case id) and outcome
classes (the first changed class with its first case); 5 the fixture
directories hold exactly the registered files (missing and unexpected files
fail); 6 the copy check.

## The W6/W7 seam

Every corpus reaches the decision code only through
`r4b_boundary::Decisions` (`llinventory_offer_oracle_boundary.h`); today its
one implementation is `FrozenDecisions` (`llinventory_offer_oracle_frozen.cpp`,
the copies). W6/W7 add an implementation that calls the Rust decisions
through the FFI and runs the returned action list with the C++ executor
translation unit, linked into this test against the same fakes; the fixtures
must replay unchanged.

Inputs: the method arguments plus the state of the fakes (model, mutes,
settings, RLVa verdicts, agent and avatar, objects, live notifications).
Outputs: the returned values plus the action list, the events the fakes log
in order (below).

| method | C++ it stands for |
|---|---|
| `receive_im(from, from_group, dialog, session, timestamp, name, message, bucket, size, sender, aux)` | `LLIMProcessing::processNewMessage` for dialogs 4, 9, 32, 37 (after `process_improved_im` copied the bucket into `U8[1200]`) |
| `receive_offline(messages)` | the ReadOfflineMsgs capability's message loop |
| `respond(offer, notification, response)` | `LLOfferInfo::handleRespond` (a user's answer; the fake notifications route through it) |
| `force_response(offer, IOR)` | `LLOfferInfo::forceResponse` |
| `send_receive_response(offer, accept, folder)`, `send_auto_receive_response`, `send_decline_response` | the response senders |
| `offer_as_llsd`, `offer_from_llsd`, `offer_from_llsd_assign` | `asLLSD`, `LLOfferInfo(LLSD)`, `fromLLSD` |
| `check_offer_throttle(from, check_only)` | the offer throttle |
| `give_item`, `give_category`, `answer(handler, notification, response)`, `share(...)` | `LLGiveInventory`, its notification handlers, the share dialog |
| `is_give_acceptable`, `is_group_give_acceptable`, `transfer_commonly_allowed`, `collect_functor(name, cat, item, count)` | the give and drop predicates |
| `will_object_accept`, `world_to_agent`, `give_drag_and_drop`, `dad3d_give(kind, tool, obj, drop)` | the drop and drag-and-drop acceptance tests |

## Format

Each file starts with `#` header lines (generator, viewer commit, platform,
seed). Every other line is `<case>\t<key>\t<value>`. Keys `eNN` are a case's
events in order, `pNN` a policy case's projected events (below), `input`,
`in` and `input.NN` the records the case runs on (F3), `out`/`result` a
predicate's verdicts. `note.*` lines are for review only: neither compared
nor digested (packet field lists, LLSD in notation, notification arguments,
full event logs of sampled policy cases).

Inputs. Gives, shares and the folder sweep carry `input.NN` lines: the agent
and avatar, what is worn, the transfer limit and system-folder overrides, the
settings, the RLVa verdicts, the mute list, IM sessions and cached names, then
the model: parents in id order, each parent's categories then items in the
order the model holds them, every record in full (`cat <id> parent= pref=
complete= name=`, `item <id> parent= type= inv_type= asset= flags= finished=
name= creator= owner= last_owner= group= group_owned= base= owner_mask=
group_mask= everyone= next=`; names raw, as stored). Matrix corpora
(predicates, will_accept, world_to_agent, drag_give) carry a `legend` case
with the shared records and one `in`/`input` line per case with the values
that vary, masks always resolved to hex.

Events (the fakes log them in the order the C++ does them):

| event | meaning |
|---|---|
| `send host=<ip:port> bytes=<hex> zc=<hex>` | `msg->sendReliable(host)` (responses go to the offer's sender host) |
| `send agent_region=<ip:port> ...` | `gAgent.sendReliableMessage()` (gives, the do-not-disturb reply) |
| `pack dialog= to= message= bucket=` | what was handed to `pack_instant_message` (bucket before its truncation; `EMPTY` for the 1-byte empty bucket) |
| `notify <name> args= payload=` | `LLNotificationsUtil::add` (binary LLSD hex; `functor=1` when it has a response handler) |
| `notify.postponed <name> ...` | `LLPostponedNotification::add` (posted once the name resolves) |
| `notification.respond <name> live=<0/1> selected=<element>` | a response: `live=0` is `forceResponse` (not in the store: the callbacks' `find()` is null) |
| `offer.<where> im= from= from_group= from_object= txn= folder= object= type= from_name= desc= host= persist=` | every live `LLOfferInfo` when a notification is posted (`postponed`), answered (`respond`) or handed to the group panel (`group_panel`) |
| `responder.deleted` | an `LLOfferInfo` deleted |
| `name.deferred <what>` | a name lookup answering from idle (deferred mode) |
| `fetch.start <observer> ids= complete= incomplete=`, `model.addObserver <observer>`, `fetch.done <observer>` | fetch observers, openers, discarders |
| `idle.queue`, `idle.run`, `main_coro.post`, `model.removeObject <id>` | deferred work, the discard's move to Trash |
| `model.deleteObject <id>`, `model.findCategoryUUIDForType <type> -> <id>` | local deletion; the folder lookup (the fake answers `000000f0-0000-4000-8000-<type as U16>`) |
| `mute.add`, `mute.remove`, `mute.auto_unmute_notice` | mute list changes |
| `new RlvGiveToRLVAgentOffer`, `new RlvGiveToRLVTaskOffer`, `rlv.sendNotification` | RLVa give-to-#RLV |
| `stage ...` | the harness's stage markers (arrival, late mute, response, unreachable) |

Packets: `bytes` is the message as `buildMessage` lays it out (offset 0);
bytes 0..4 (flags and packet id) are recorded as zero, as `sendMessage` zeroes
them before stamping the circuit's packet id and the reliable flag. `zc` is the
zero-coded form (`-` when coding would not shrink it). The viewer never adds
`EstateBlock` or `MetaData`: the packet ends `... BinaryBucket, 0x00`.

Fixed values: agent `000000a1-...-000000000001`, session `...a1...02`, the
other avatar `...b2...01`, transaction `...d4...01`, offered object
`...e5...01`, group `...c3...01`, #RLV shared root `...f1...01`; position
<128.25, 64.5, 22.75>; full name "Oracle Tester"; region host 10.0.0.1:13000;
offer sender host 10.0.0.9:13005; `LLUUID::generate()` in the copies is
`7a000000-0000-4000-8000-<counter>` and `LLDate::now()` is 1700000000 (both
documented substitutions). Every setting the copies read is pinned to its
`app_settings` default unless the case says otherwise; an unpinned setting
aborts the test. `LLAvatarName::trimResidentSurname()` is on.
`LLTrans::getString(k)` renders `{k}` (`{k|ARG=value|...}` with arguments);
SLURLs render `secondlife:///app/<cmd>/<id>/<verb>`.

## Files

| file | survey 4 §7 | what |
|---|---|---|
| `responses.txt` | set 4 | `sendReceiveResponse`, `send_auto_receive_response`, `send_decline_response` for dialogs 4, 9, 32, 37, with a transaction and a null one (no message), one-shot, invalid avatar, RLVa hidden names. 72 cases. |
| `incoming.txt` | set 5 | `receive_im`: every bucket size and encoding (agent 0/1/16/17/18/34 bytes, type bytes 0..127, −1, −2, −128; task one byte 0..255 and the legacy `"<type>\|..."` strings with and without NUL in two fill modes; 0..1300-byte task buckets; both group-notice encodings, short and non-terminated); names; mutes. 129 cases. |
| `incoming_sweep.*` | set 5 | 6000 seeded random received offers (108 skipped: uninitialised legacy type). Sample modulus 60. |
| `offline.txt` | set 5/6 (F8) | Offers through ReadOfflineMsgs: agent, task and group-notice messages (with and without `binary_bucket`, transaction from `asset_id`, `from_group` as `"Y"`, local position) and their responses: **the reply goes to the agent's region host**, not an offer host. 11 cases. |
| `offerinfo_llsd.txt` | set 5 | `asLLSD` / `LLOfferInfo(LLSD)` / `fromLLSD` round trips (13 field sets) and 16 crafted documents. |
| `forced_response_index.txt` | set 6 | Form element order of the four offer templates; `forceResponse(r)` for r = 0..6. **Forced responses are indexed by form element (Show = 0, Keep = 1, Discard = 2, Mute = 3), not by button**: ACCEPT passes element 1, DECLINE 2, MUTE 3; SHOW and the silent responses pass −1 and nothing is called. |
| `throttle.txt` | set 6 | `check_offer_throttle` traces over an injected clock. |
| `policy.digest`, `.sample.txt`, `.outcomes.txt` | set 6 | The offer policy cross product (below). 589,824 cases, 1804 outcome classes. |
| `policy_response.digest`, `.sample.txt`, `.outcomes.txt` | set 6 (F1) | What changes between arrival and response (below). 43,008 cases, 3372 classes (hashed index). |
| `policy_present.digest`, `.sample.txt` | set 6 | The offered object already in the model and finished; default settings. 512 cases; sample modulus 8. |
| `policy_named.txt` | set 6 | A readable named case for every survey §9 risk and every audited behaviour (51 cases, full event logs), including offers revived from LLSD (`direct_*`). |

### The policy table (`policy`)

Axes (589,824 = 4 × 2 × 4 × 2 × 2 × 2 × 2 × 3 × 2 × 3 × 8 × 2 × 2 × 2):
dialog 4/9/32/37; from self; mute none / by id / by name (legacy) / by id with
the text-chat exemption; do not disturb; `AutoAcceptNewInventory`;
`FSUseLegacyInventoryAcceptMessages`; RLVa on; give-to-#RLV (no prefix, prefix
with a shared root, prefix without one); `RestrainedLoveForbidGiveToRLV`;
type category / notecard / invalid (127); button (the seven `IOR_*` values and
"closed"); forced or interactive; `FSDismissGroupNoticeAttachmentsToTrash`;
`LogInventoryDecline`. The case id spells every axis.

Each case is arrival (the IM through `receive_im`) then the response.
Interactive: the button of that index in the posted notification's form is
pressed; "closed" answers with the response template; a button the template
lacks is `stage unreachable`. Forced: `force_response(button)`; a forced close
is unreachable. Group notices are answered by forced responses only (toast,
notification well, group panel): the offer is rebuilt from the GroupNotice
payload's `inventory_offer` (32) or taken from the group panel (37). When the
arrival already forced a response or auto-accepted, the response stage is
`stage none`.

### The response table (`policy_response`, F1)

Axes (43,008 = 7 × 2 × 2 × 3 × 2 × 2 × 2 × 8 × 2 × 2 × 2): dialog with its
group flag (4; 9, 32, 37 each without and with: a group-owned object sends
from the group's id, a group notice carries the flag); from self; sender named
"Grid Linden" or not; **a mute added between arrival and response** (none, by
id, by name); **name lookups answering at once or from idle** (deferred mode:
avatar names, group names and postponed notifications answer from idle, as
the real caches usually do, so a Mute button's mute lands after the decline
decision); legacy mode; category or notecard; 8 buttons; forced or
interactive; dismiss-to-Trash; `LogInventoryDecline`. Case id: the policy id
plus `.fg<0/1>.late_<none|id|name>.linden<0/1>.defer<0/1>`.

### Projection, digests, chunks, samples, outcome classes

The canonical stream of a policy case is its **projection**: in order, the
events starting with `send `, `notify.postponed `, `notify `,
`notification.respond `, `model.removeObject `, `model.deleteObject `,
`model.addObserver `, `fetch.start `, `fetch.done `, `open_inventory_offer `,
`new LLOpenTaskGroupOffer`, `new RlvGiveToRLV`, `RlvGiveToRLVAgentOffer.done`,
`rlv.sendNotification `, `mute.add `, `mute.remove `, `recent_people.add `,
`inventory_panel.select `, `group_panel.showNotice `, `stage `,
`nearby_chat `, `toasts.kill `, `blocked_list.show `, `offer.`,
`responder.deleted`, `name.deferred `, rendered as logged except: `notify`
lines lose `args=...`; `send` lines lose ` zc=...`; `offer.` snapshots lose
`from=`, `from_object=`, `from_name="..."` and `desc="..."` (the case id fixes
them) and keep the offer's state (dialog, group flag, transaction, folder,
object, type, host, persist).

- `.digest`: `lines`, `cases`, `sha256` of the canonical stream (every non-note
  line followed by `\n`), then `chunk=<i> first=<case> last=<case> cases=<n>
  sha256=<hex>` for consecutive blocks of `chunk_cases` cases (8192 for
  `policy`, 2048 for `policy_response`, 512 otherwise).
- `.sample.txt`: every line (notes included) of the cases with
  `in_sample(case, sample_modulus)`: FNV-1a 32-bit (offset 2166136261, prime
  16777619) of the case id modulo the modulus equal to 0. For the policy tables
  it also holds, for each (outcome class, RLVa state) pair whose outcome has an
  RLVa action, the first case (canonical lines).
- `.outcomes.txt`: every distinct outcome (a case's canonical lines without
  its id) in order of first appearance, with its case count and first case.
  `policy.outcomes.txt` gives each class's lines (it is the per-class part of
  the stratified sample, compared in full); `policy_response.outcomes.txt`
  gives each class's SHA-256 instead (size budget; `VKS_R4B_FULL` writes the
  text).

## Generators (seeded sweeps)

xorshift64*: state `s` = seed (0 becomes 0x9E3779B97F4A7C15); `next()`:
`s ^= s >> 12; s ^= s << 25; s ^= s >> 27; return s * 0x2545F4914F6CDD1D`
(mod 2^64); `below(n)` = `next() % n` (64-bit modulo, then 32 bits);
`coin()` = `next() & 1`. Every draw below is a separate statement in this
order (a conditional draws its condition first, then only the branch taken).

- `offer/incoming_sweep` (seed 0x5234424f46460002, i = 0..5999):
  dialog = [4, 9, 32, 37][below(4)];
  size = below(4) == 0 ? below(200) : (dialog 4: 17; 9: 1; 32/37: 19 + below(30));
  each bucket byte: below(3) == 0 ? below(256) : (below(3) == 0 ? '|' : '0' + below(10));
  if the bucket is not empty: below(2) == 0 sets its last byte to 0;
  session = below(4) == 0 ? null : TXN; aux = below(3) == 0 ? GROUP : null;
  from_group = coin(); from = below(5) == 0 ? AGENT : OTHER;
  message = coin() ? "Subject|Body" : "#RLV/~x"; name = coin() ? "Sender Resident" : "";
  a task offer whose bucket is not one byte and whose legacy type would stay
  uninitialised is skipped (its index is kept). Case id `incoming_%04d`.
- `give/pack_sweep`, `give/folder_sweep`, `give/predicate_sweep`: see
  `../give/README.md`.

## Runtime edges

- **Uninitialised legacy type.** A task offer whose bucket is not one byte is
  read as a C string up to `'|'`; with no digits before it, `S32 type` keeps
  the stack's value (`legacy_read ... type=UNINITIALISED`, then
  `runtime_edge ...`; the rest of such a case is kept as notes).
- **Over-read.** The legacy string has no length bound; the `_fill0`/`_fill1`
  cases put zeros or `'A'` after the bucket. From ReadOfflineMsgs the bucket is
  a heap vector of exactly its size; an empty `binary_bucket` gives `data()`
  that may be null: not executed.
- **Not executed:** a group-notice binary bucket larger than 1200 bytes; a
  group-notice string with fewer than three `'|'` tokens and inventory.

## Out of scope (stated)

- The NaCl antispam queue (`UseAntiSpam`, default 0) and the Omnifilter
  (`OmnifilterEnabled`, default 0) can swallow any IM, offers included, before
  the offer code runs, with no reply. They are filters in front of this area,
  stay C++ and are off by default; the processNewMessage preamble that calls
  them is guarded by text (`guard_process_new_message_preamble`).
- Calling-card offers (`OfferCallingCard`, survey §1.6) and the Firestorm
  autoresponse item give (`FSAutoresponseItemUUID`) are not recorded.
- The bulk-permission floater's random FolderID (`llfloaterbulkpermission.cpp`).

## Behaviour recorded as it is (not fixed)

- Agent offers are accepted on arrival outside legacy mode; the buttons act
  locally.
- "Discard" on an object offer accepts into Trash; only a muted sender (also
  muted after arrival) or Mute gets `IM_TASK_INVENTORY_DECLINED`.
- Object offers under do not disturb or from a muted sender get **no reply at
  all**: the forced response goes through `inventory_offer_callback`, which
  sends nothing for task offers.
- A forced MUTE adds no mute (no live notification). With deferred names, a
  Mute button's mute is added after the reply was decided.
- The Linden exemption of the decline path only shows for an offer whose
  `from_name` is a Linden name; a group notice's `from_name` is the
  `InvOfferGroupNoticeName` template, so through the IM path it never is
  (`direct_*` cases).
- `send_decline_response()` does not map 37 to 32 (dialog 39), has no
  null-transaction guard and does not consume the transaction id.
- Auto-accepted object offers start a fetch observer and an opener for a null
  object id. The give-to-#RLV notices parse the description with
  `"'  ( http://"` only.

## Platform differences

Recorded with MSVC 19.44, GCC 15.2 (WSL Ubuntu 26.04) and GCC 14 (CI's
compiler); each dump is compared with the committed (Windows) files after the
header. Every compared line and digest is identical on the three. The only
difference is in review notes: the uninitialised legacy type took the stack's
value 3, 4 and 0 under MSVC and 77, 8 and 8 under both GCCs
(`task_legacy_pipe_only_*`, `task_legacy_leading_nul_*`, `task_size_0_fill0`).
