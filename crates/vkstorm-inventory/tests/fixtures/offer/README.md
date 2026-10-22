# offer fixtures

Inventory offers: received offers decoded into `LLOfferInfo`, its LLSD form,
the accept and decline packets, the forced-response indexing, the offer
throttle and the offer policy table. Recorded by R4b of the inventory rung
(vkstorm-rust rung seven) from the unmodified viewer C++ (viewer commit
`472ecf270d`, `codex/rust-inventory`), on Windows and Linux. Never edit them by
hand: change the recorder and record again. Survey 4 section 7: the instant
message part of set 4, sets 5 and 6. The give side (set 4 gives, set 7) is in
`../give/`; the two directories share one recorder and one format.

## How they are made

`INTEGRATION_TEST_llinventory_offer_oracle` (`indra/newview/tests/`,
registered by `tests/inventory/r4-offer.cmake`). The decision code is frozen
verbatim in `llinventory_offer_oracle_copies.{h,cpp}` (46 regions; each names
its source file, line range and blob) and compiled against the fakes in
`llinventory_offer_oracle_fakes.{h,cpp}`; `check_r4b_copies.py` confirms the
copies still match the live source. Every packet is built by the real message
system from `scripts/messages/message_template.msg` (the Rust template builder
since rung three) and the real `pack_instant_message`; `LLUUID`, `LLSD`,
`LLPermissions`, `LLInventoryItem`, `LLAssetType`, `LLFolderType`, `LLHost` and
`LLCacheName`'s name helpers are real too.

```sh
# write the fixtures (viewer):
VKS_R4B_DUMP=<dir> VKS_R4B_COMMIT=<viewer commit> INTEGRATION_TEST_llinventory_offer_oracle
# replay them: run the test with no environment (the build does this).
# VKS_R4B_ONLY=<name,name> limits a dump to some corpora.
```

`tools/inventory-fixtures/offer/regenerate.py` in vkstorm-core-rust runs the
dump and copies the result here and into
`crates/vkstorm-inventory/tests/fixtures/{offer,give}`.

## Format

Each file starts with `#` header lines (generator, viewer commit, platform,
seed). Every other line is `<case>\t<key>\t<value>`. Keys `eNN` are the case's
events in order; `note.*` lines are for review only and are neither compared
nor digested (packet field lists, LLSD in notation, notification arguments,
the full event log of a sampled policy case). The test compares every other
line and reports case, key, expected and actual.

Events (the fakes log them in the order the C++ does them):

| event | meaning |
|---|---|
| `send host=<ip:port> bytes=<hex> zc=<hex>` | `msg->sendReliable(host)` (responses go to the offer's sender host) |
| `send agent_region=<ip:port> ...` | `gAgent.sendReliableMessage()` (gives, the do-not-disturb reply) |
| `pack dialog=<n> to=<id> message="<text>" bucket=<hex>` | what was handed to `pack_instant_message` (bucket before its truncation; `EMPTY` for the 1-byte empty bucket) |
| `notify <name> args=<binary LLSD hex> payload=<binary LLSD hex>` | `LLNotificationsUtil::add` (`functor=1` when it has a response callback) |
| `notify.postponed <name> ... from= group= offer_from_agent= responder=` | `LLPostponedNotification::add` (offer notifications, posted once the name resolves) |
| `notification.respond <name> live=<0/1> selected=<element>` | a response reaching a notification: `live=0` is `forceResponse` (the notification is not in the store, so the callbacks' `find()` is null) |
| `offer.<where> im= from= ... persist=` | every live `LLOfferInfo`, field by field, when a notification is posted (`postponed`), answered (`respond`) or handed to the group panel (`group_panel`) |
| `fetch.start <observer> ids= complete= incomplete=` | a fetch observer started (sorted as the real `startFetch` does) |
| `model.addObserver <observer>` / `fetch.done <observer>` | an opener or discarder waiting, or done at once |
| `idle.queue`, `idle.run`, `model.removeObject <id>` | `LLDiscardAgentOffer`'s deferred move to Trash |
| `model.deleteObject <id>` | a local deletion |
| `model.findCategoryUUIDForType <type> -> <id>` | the folder lookup (input recorded; the fake answers `000000f0-0000-4000-8000-<type as U16>`) |
| `mute.add`, `mute.remove`, `mute.auto_unmute_notice` | mute list changes |
| `new RlvGiveToRLVAgentOffer`, `new RlvGiveToRLVTaskOffer`, `rlv.sendNotification` | RLVa give-to-#RLV |
| `responder.deleted` | an `LLOfferInfo` deleted (`delete this` or its owner released) |

Packets: `bytes` is the message as `LLTemplateMessageBuilder::buildMessage`
lays it out (offset 0); bytes 0 to 4 (flags and packet id) are recorded as
zero, as `sendMessage` zeroes them before stamping the circuit's packet id and
the reliable flag. `zc` is the zero-coded form (`vks_msg_zero_code`, as
`compressMessage` does for ImprovedInstantMessage), `-` when coding would not
shrink it. The viewer never adds `EstateBlock` or `MetaData`: the packet ends
`... BinaryBucket, 0x00`.

Fixed values: agent `000000a1-...-000000000001`, session `...a1...02`, the
other avatar `...b2...01`, transaction `...d4...01`, offered object
`...e5...01`, group `...c3...01`, #RLV shared root `...f1...01`; position
<128.25, 64.5, 22.75>; full name "Oracle Tester"; region host
10.0.0.1:13000; offer sender host 10.0.0.9:13005; `LLUUID::generate()` in the
copies is `7a000000-0000-4000-8000-<counter>` and `LLDate::now()` is
1700000000 (both documented substitutions). Every setting the copies read is
pinned to its `app_settings` default unless the case says otherwise; an
unpinned setting aborts the test. `LLTrans::getString(k)` renders `{k}` (with
arguments `{k|ARG=value|...}`), SLURLs render
`secondlife:///app/<cmd>/<id>/<verb>`: UI text, recorded but not the viewer's.

## Files

| file | survey 4 §7 | what |
|---|---|---|
| `responses.txt` | set 4 | `sendReceiveResponse(accept, dest)`, `send_auto_receive_response()`, `send_decline_response()` for dialogs 4, 9, 32 and 37, with a transaction and with a null one (no message), accept then decline (one-shot), legacy decline twice, invalid avatar (empty FromAgentName), RLVa hidden names (no Recent People). 72 cases. |
| `incoming.txt` | set 5 | `process_offer_im`: the inventory-offer and group-notice cases of `LLIMProcessing::processNewMessage` (name clean-up, mute, do not disturb, bucket parsing, the folder lookup, then `inventory_offer_handler` or the group notice paths). Agent buckets of 0, 1, 16, 17, 18, 34 bytes and type bytes 0..127, -1, -2, -128; task buckets of one byte (0..255) and the legacy `"<type>\|..."` strings with and without NUL (fill modes below); 0/16/17/18/34/1300-byte task buckets; session null with aux; both group-notice encodings (binary struct: full, no inventory, 19-byte, 18-byte, not terminated, embedded NUL, sender = group, UUID name; string with aux: full, wearable, NUL-terminated, empty fields, extra fields, alphabetic type, sizes 0 and 1), muted sender, name trimming on and off. 129 cases. |
| `offerinfo_llsd.txt` | set 5 | `LLOfferInfo::asLLSD()` (binary LLSD) and `LLOfferInfo(LLSD)` / `fromLLSD()` for 13 field sets and 16 crafted documents (missing keys, wrong types, unknown asset type names, bad sender strings). |
| `forced_response_index.txt` | set 6 | The form element order of `UserGiveItem`, `UserGiveItemLegacy`, `ObjectGiveItem`, `OwnObjectGiveItem`; `forceResponse(r)` for r = 0..6 and what reaches `inventory_offer_callback`. **Forced responses are indexed by form element (Show = 0, Keep = 1, Discard = 2, Mute = 3), not by button**: ACCEPT passes element 1 (Keep, button 0), DECLINE element 2 (Discard, button 1), MUTE element 3 (Mute, button 2); SHOW and the silent responses pass -1 and nothing is called. `if_button_index_were_used` shows the inversion a port must not make (DECLINE as element 1 would select Keep). |
| `throttle.txt` | set 6 | `check_offer_throttle` traces over an injected clock: bursts, anonymous senders, `FSOfferThrottleMaxCount` 5, 2, 0, before STATE_STARTED, notices off, `check_only`. |
| `policy_present.txt` | set 6 | Supplement: the offered object already in the model and finished (openers and discards complete at once): dialogs × category/notecard × auto-accept × legacy × 8 buttons × forced/interactive, default settings. 512 cases; policy lines for all, full event logs for the cases sampled with modulus 8. |
| `policy.digest`, `policy.sample.txt`, `policy.outcomes.txt` | set 6 | The offer policy cross product (below). |
| `incoming_sweep.digest`, `.sample.txt` | set 5 | 6000 seeded random received offers (seed in the header), 108 skipped (uninitialised legacy type); digest + every case with `in_sample(case, 30)`. |

### The policy table

Axes (589,824 cases = 4 × 2 × 4 × 2 × 2 × 2 × 2 × 3 × 2 × 3 × 8 × 2 × 2 × 2):
dialog 4/9/32/37; from self; mute none / by id / by name (legacy) / by id with
the text-chat exemption flag; do not disturb; `AutoAcceptNewInventory`;
`FSUseLegacyInventoryAcceptMessages`; RLVa on; give-to-#RLV (no prefix /
prefix with a shared root / prefix without one); `RestrainedLoveForbidGiveToRLV`;
type category / notecard / invalid (127); button (the seven `IOR_*` values
and "closed"); forced or interactive; `FSDismissGroupNoticeAttachmentsToTrash`;
`LogInventoryDecline`. The case id spells every axis.

Each case is two stages: arrival (the IM through `process_offer_im`) and the
response. Interactive: the button of that index in the posted notification's
form is pressed (`respond`), "closed" answers with the response template (no
button: the callbacks' default branch); a button the template does not have
is recorded `stage unreachable`. Forced: `info->forceResponse(button)`; a
forced "closed" is unreachable. Group notices are only ever answered by forced
responses (the toast, the notification well and the group panel call
`forceResponse`): the notice's offer is rebuilt from the payload's
`inventory_offer` LLSD (dialog 32) or taken from the group panel (37). When the
arrival already forced a response (mute, invalid type, do not disturb on an
object offer) or auto-accepted, there is no notification and the response
stage records `stage none`.

The committed canonical stream is the **policy projection** of each case's
events: the lines `pNN` keep, in order, the events starting with `send `,
`notify.postponed `, `notify `, `notification.respond `, `model.removeObject `,
`model.deleteObject `, `model.addObserver `, `fetch.start `, `fetch.done `,
`open_inventory_offer `, `new LLOpenTaskGroupOffer`, `new RlvGiveToRLV`,
`RlvGiveToRLVAgentOffer.done`, `rlv.sendNotification `, `mute.add `,
`mute.remove `, `recent_people.add `, `inventory_panel.select `,
`group_panel.showNotice `, `stage `, `nearby_chat `, `toasts.kill `,
`blocked_list.show `; `notify` lines lose their `args=...` (UI text) and keep
name and payload. That is: the notification or forced response, the messages
in order with their bytes, and the local actions (discard-to-trash id, opener
kind, RLV observer, mute added).

- `policy.digest`: `lines`, `cases` and `sha256` of the canonical stream (every
  non-note line followed by `\n`).
- `policy.sample.txt`: every line, notes included (`note.full.eNN` is the full
  event log), of the cases with `in_sample(case, 4001)`, where `in_sample` is
  FNV-1a (32-bit, offset 2166136261, prime 16777619) of the case id modulo the
  modulus equal to 0.
- `policy.outcomes.txt`: the 1340 distinct outcomes (a case's canonical lines
  without its id), each with its case count and first case id.

## Runtime edges

- **Uninitialised legacy type.** A task offer whose bucket is not one byte is
  read as a C string up to `'|'`; when no digits precede it (`"|x"`, a leading
  NUL) the stream conversion stores nothing and `S32 type` keeps the stack's
  value. `incoming.txt` states it (`legacy_read ... type=UNINITIALISED`, then
  `runtime_edge ...`) and keeps the rest of such a case as notes; the sweep
  skips such cases. The port must state an outcome; the C++ has none.
- **Over-read.** The legacy string has no length bound: `process_improved_im`
  copies the bucket into `U8[1200]` on the stack and the read runs past the
  bucket until a NUL. The `_fill0`/`_fill1` cases put zeros or `'A'` after
  the bucket (with a NUL past the array, so the test stays inside its buffer).
  The type only changes when the digits run to the bucket's end and the bytes
  after it are digits (`77` with no NUL would read on into the stack); with
  both fills the result here is the same. From the ReadOfflineMsgs capability
  the bucket is a heap vector of exactly its size, so the over-read reads heap
  bytes, and an empty `binary_bucket` gives `data()` that may be null: not
  executed.
- **Not executed:** a group-notice binary bucket larger than 1200 bytes (the
  NUL check reads past the stack array), and a group-notice string bucket with
  fewer than three `'|'` tokens and inventory (the tokenizer iterator is
  dereferenced past its end; NDEBUG builds return the last token).
- `MTUBYTES` truncation of the Message text, the 1199-byte cap and the
  embedded-NUL stop are recorded in `../give/`.

## Behaviour recorded as it is (not fixed)

- Agent offers are accepted on arrival (`IM_INVENTORY_ACCEPTED` to the default
  folder for the type) outside legacy mode; the buttons only act locally.
- "Discard" on an object offer sends `IM_TASK_INVENTORY_ACCEPTED` into Trash;
  only a muted sender (or Mute) gets `IM_TASK_INVENTORY_DECLINED`.
- Forced responses always go through `inventory_offer_callback`, also for
  task offers and group notices; a forced MUTE adds no mute (no live
  notification).
- `send_decline_response()` does not map 37 to 32 (dialog 39 for a requested
  notice; only reachable for agent offers in practice), has no null-transaction
  guard and does not consume the transaction id.
- Auto-accepted object offers start a fetch observer and an opener for a null
  object id.
- The give-to-#RLV notifications parse the description with `"'  ( http://"`
  only.

## Platform differences

Recorded with MSVC 19.44 (Windows, RelWithDebInfo with the Release feature
set), GCC 15.2 (WSL Ubuntu 26.04) and GCC 14 (CI's compiler), each dump
compared with the committed (Windows) files after the header. Every compared
line and every digest is identical on the three. The only difference is in
review notes: the uninitialised legacy type (runtime edges above) took the
stack's value 3, 4 and 0 under MSVC and 77, 8 and 8 under both GCCs
(`task_legacy_pipe_only_*`, `task_legacy_leading_nul_*`, `task_size_0_fill0`),
which is why those outcomes are not compared.
