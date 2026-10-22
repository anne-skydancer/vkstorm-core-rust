# give fixtures

Inventory giving: the ImprovedInstantMessage of item and folder gives, the
folder bucket, the give and drop predicates, `willObjectAcceptInventory`, the
world-to-agent accept test and the share dialog. Recorded by R4b of the
inventory rung (vkstorm-rust rung seven) from the unmodified viewer C++ (viewer
commit `472ecf270d`), on Windows and Linux. Never edit them by hand.
Survey 4 section 7: the give part of set 4 and set 7.

Generator, format, event vocabulary, fixed ids and fake renderings: see
`../offer/README.md` (one recorder, one test, one format).

## Files

| file | what |
|---|---|
| `give_item.txt` | `LLGiveInventory::doGiveInventoryItem` to the other avatar: every defined asset type (29); worn object, clothing, body part, active gesture; links (to a notecard: type byte 7 with the link's id; to a worn object; link to a link and broken link: 24; folder link: 8; broken folder link: 25; a link whose own permissions are no-copy); names: empty (Message 0 bytes), corrected by `LLInventoryObject` (`\|`, control and non-ASCII bytes, 64 → 63 bytes), raw 1182/1183/1184/1199/1200/1300 bytes (the 17-byte bucket truncated to `1200 - strlen(name)`, Message capped at 1199), embedded and leading NUL; not transferable; not the owner; null item; invalid avatar; IM session; recipient muted by id (unmuted: `mute.remove` and the auto-unmute notice) or by name only; RLVa `@share`; RLVa hidden names; open IM session; no beam. No-copy: the `CannotCopyWarning` alone, Yes (give, then `deleteObject` locally), No (`TransactionCancelled`), Yes after the item went (`CannotGiveItem`), Yes under `@share` (blocked notice), Yes with an invalid avatar (empty FromAgentName), Yes losing the IM session. 70 cases. |
| `give_folder.txt` | `doGiveInventoryCategory` and its answers: empty (`NoItems`), one item, flat mixed types, nested (bucket order: root, sub-categories in pre-order with the root's type byte, then items with their subtree's items first), 4 deep, 42 entries (731-byte bucket), 43 (`TooManyItems`), OpenSim limit 100 with 70/100/101 entries (bucket truncated), raw names of 700/1199/1300 bytes, embedded NUL, empty, a 70-byte name corrected to 63; worn and no-transfer items excluded, only excluded (`NoItems`); links inside; incomplete sub-folder (`IncompleteInventory`, also with too many and with nothing); giving the Trash itself; RLVa `@share`; muted recipient; invalid avatar. No-copy items: `CannotCopyCountItems` alone, Yes (give, then the uncopyable items deleted locally), No, Yes with a success notification, **Yes after the count grew past the limit (`TooManyItems`, and the uncopyable items are still deleted)**, Yes after the folder went (`CannotGiveCategory`), Yes under `@share`, worn no-copy items not deleted. 34 cases. |
| `predicates.txt` | `isInventoryGiveAcceptable`, `isInventoryGroupGiveAcceptable`, `itemTransferCommonlyAllowed`, `LLGiveable` (result/countNoCopy), `LLUncopyableItems`, `LLDroppableItem(true)` and `(false)` (result/countNoCopy), `LLDropCopyableItems` over 9 kinds (object, body part, clothing, gesture, notecard, texture, calling card, link to object, link to notecard) × not worn / attached / worn wearable / active gesture × owner agent / other / group-owned × the 8 copy/modify/transfer owner masks × everyone copy × base with or without transfer × avatar valid; plus a category alone, null item and null both. 6915 lines. |
| `will_accept.txt` | `LLToolDragAndDrop::willObjectAcceptInventory` (raw `EAcceptance` and its name) over object modify × allow-inventory-drop × owned by you × volume × attachment and item owned by agent × transferable × base unrestricted × object/clothing/calling card/notecard/link × worn, with RLVa off, an item drop, a finished item from elsewhere; plus null object and null item. 2562 cases. |
| `will_accept_sweep.digest`, `.sample.txt` | Every combination (102,400): the above × finished × dropped onto its own object × category drop × RLVa off / on / locked attachment / `@unsit` sitting on the linkset / `@sittp` sitting elsewhere. Sample modulus 101. |
| `world_to_agent.txt` | `move_inv_category_world_to_agent`: object contents as every sequence of length 0..3 over c (copy and transfer), n (no copy), t (no transfer), x (an `LLInventoryObject` that is not an item) × object owned by you × filter (none, object-type, date passing all, date passing only c) × drop × object found. Result, and on drop the forced `MoveInventoryFromObject` answer with the move list or `warn_move_inventory` (move). **The accept flag is sticky**: once the first item is acceptable, later items that are not (no copy in an object you do not own) are not checked and still go in the move list (`seq_cn.owned0...`); `is_move` only reflects later items. 1361 cases. |
| `share.txt` | `give_inventory_cb` of the share dialog (llavataractions.cpp): **the first folder in UUID order is given and the selection loop stops** (items after it are skipped); copyable items one IM each; no-copy items batched into one `CannotCopyWarning` for the first avatar only, after which later avatars get nothing; cancel; no avatars; empty selection. 13 cases. |
| `pack_sweep.digest`, `.sample.txt` | 4000 seeded random `pack_instant_message` calls (names 0..1399 bytes with NULs and high bytes, buckets 0..1499 bytes or null, every dialog). Sample modulus 50. |
| `folder_sweep.digest`, `.sample.txt` | 1500 seeded random folder trees (depth ≤ 4, worn, no-copy, no-transfer, links, incomplete folders, limit 42 or 100) given and answered Yes or No. Sample modulus 15. |
| `predicate_sweep.digest`, `.sample.txt` | 20000 seeded random 32-bit permission masks, owners, types and worn states through the predicates. Sample modulus 100. |

The test also packs 3000 seeded random messages through the live
`pack_instant_message` and the frozen copy of `pack_instant_message_block`
and requires identical packets.

## Behaviour recorded as it is (not fixed)

- Giving to a muted avatar unmutes them (`LLMuteList::autoRemove`, with the
  auto-unmute notice), for items and folders.
- No-copy gives delete the item locally on "Yes" whatever the give did, and a
  folder's uncopyable items are deleted even when the give failed.
- The folder bucket reuses the root's type byte for every sub-category, counts
  exclude the root (43 entries go out under the limit of 42), and the bucket
  is silently truncated to `1200 - strlen(name)`; a link is given with the
  linked item's type byte and the link's own id.
- The share dialog gives only the first folder, in UUID order.
- `move_inv_category_world_to_agent`'s accept flag is sticky.
- Not in scope, noted only: the bulk-permission floater sends a random
  `FolderID` (`llfloaterbulkpermission.cpp`).

## Runtime edges

Names longer than 63 bytes and names with control bytes cannot come from the
inventory model (`LLInventoryObject::correctInventoryName`); the `name_raw_*`
cases set them directly to exercise `pack_instant_message`'s truncation and
NUL rules. In the folder trees the item order is the order the fake model
holds the children (the real model's order is survey 2's).

## Platform differences

See `../offer/README.md`.
