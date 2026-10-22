# give fixtures

Inventory giving: the ImprovedInstantMessage of item and folder gives, the
folder bucket, the give and drop predicates, `willObjectAcceptInventory`, the
world-to-agent accept test, the share dialog and drag-and-drop give
acceptance. Recorded by R4b of the inventory rung (vkstorm-rust rung seven)
from the unmodified viewer C++ (viewer base `472ecf270d`), on Windows and
Linux. Never edit them by hand. Survey 4 section 7: the give part of set 4 and
set 7.

Generator, the W6/W7 seam, format, input records, event vocabulary, digests,
chunks and samples: `../offer/README.md` (one recorder, one test, one format).

## Files

| file | what |
|---|---|
| `give_item.txt` | `give_item` (`LLGiveInventory::doGiveInventoryItem`): every defined asset type (29); worn object, clothing, body part, active gesture; links (to a notecard: type byte 7 with the link's id; to a worn object; link to a link and broken link: 24; folder link: 8; broken folder link: 25; a no-copy link to a copyable item); names: empty, corrected by `LLInventoryObject`, 62/63/64 bytes, raw 1182/1183/1184/1198/1199/1200/1300 bytes (the 17-byte bucket truncated to `1200 - strlen(name)`, Message capped at 1199), embedded and leading NUL; not transferable; not the owner; null item; invalid avatar; IM session; recipient muted by id (unmuted) or by name only; RLVa `@share`; RLVa hidden names; open IM session; no beam; no-copy: the warning, Yes, No, Yes after the item went, Yes under `@share`, Yes with an invalid avatar, Yes losing the IM session. 72 cases, each with its world as `input.NN`. |
| `give_folder.txt` | `give_category` and its answers: empty, one item, flat, nested (bucket order: root, sub-categories in pre-order with the root's type byte, then items, a subtree's items before its parent's), 4 deep, 41/42/43 entries, OpenSim limit 100 with 70/99/100/101 entries, raw names of 700/1198/1199/1200/1300 bytes, embedded NUL, empty, a 70-byte name corrected to 63; exclusions; links; incomplete folders; the Trash itself; `@share`; muted recipient; invalid avatar; no-copy answers including **Yes after the count grew past the limit (`TooManyItems`, and the uncopyable items are still deleted)**. 38 cases, each with its world and model as `input.NN`. |
| `predicates.txt` | The give and drop predicates over 9 kinds × 4 worn states × 3 owners × 8 owner masks × everyone copy × base with or without transfer × avatar valid (6912 cases), the permission rules where the rule changes the outcome (`rule_*`: owner mask against base, next-owner copy, group ownership set atomically or not, everyone copy for another owner), and a category alone, null item, null both. The `legend` case gives the shared records, the case id and the `in`/`out` keys; `in` has the masks resolved. |
| `will_accept.txt` | `will_object_accept` (raw `EAcceptance` and its name) over object modify × allow-inventory-drop × owned by you × volume × attachment and item owner × transfer × base unrestricted × object/clothing/calling card/notecard/link × worn, RLVa off, item drop, finished, from elsewhere; null object, null item. `legend` + `in` per case. 2562 cases. |
| `will_accept_sweep.*` | Every combination (102,400): also finished × dropped onto its own object × category drop × RLVa off / on / locked attachment / `@unsit` sitting on the linkset / `@sittp` sitting elsewhere. Sample modulus 401. |
| `world_to_agent.txt` | `world_to_agent` (`move_inv_category_world_to_agent`): contents as every sequence of length 0..3 over c (copy and transfer), n (no copy), t (no transfer), x (an `LLInventoryObject` that is not an item) × owned by you × filter (none, object type, date passing all, date passing only c) × drop × object found. **The accept flag is sticky** (`seq_cn.owned0...`). `legend` + `input` per case. 1361 cases. |
| `share.txt` | `share` (the share dialog's callback): **the first folder in UUID order is given and the loop stops**; copyable items one IM each; no-copy items batched into one `CannotCopyWarning` for the first avatar only; cancel; no avatars; empty selection. 13 cases. |
| `drag_give.txt` | F8: `give_drag_and_drop` (`LLToolDragAndDrop::handleGiveDragAndDrop`) over 10 cargo types × 5 states (copyable item, worn object, not in the model, folder, no-transfer item) × RLVa `@share` (**→ `ACCEPT_NO_LOCKED`**); drops and the `ShareItemsConfirmation` answer (with and without an IM session, cached name or lookup, folder); `dad3d_give` (`dad3dGiveInventoryObject`, `dad3dGiveInventory`, `dad3dGiveInventoryCategory`) × source agent/world × ok / not finished / no transfer / worn × `@share` × object or none × drop. 297 cases. |
| `pack_sweep.*` | 4000 seeded random `pack_instant_message` calls. Sample modulus 100. |
| `folder_sweep.*` | 1500 seeded random folder trees given and answered (each case's world and model as `input.NN`). Sample modulus 60. |
| `predicate_sweep.*` | 20000 seeded random permission masks through the predicates. Sample modulus 100. |

The test also packs 3000 seeded random messages through the live
`pack_instant_message` and the frozen `pack_instant_message_block` and
requires identical packets.

## Generators

xorshift64* as in `../offer/README.md`; each draw a separate statement, in
this order.

- `pack_sweep` (seed 0x52344250414b0001, i = 0..3999):
  len = below(4) == 0 ? below(1400) : below(80);
  each name byte: below(5) == 0 ? below(256) : 'a' + below(26);
  bsize = below(3) == 0 ? below(1500) : 17 × (1 + below(50));
  each bucket byte: below(256); null_bucket = below(10) == 0;
  dialog = below(43); from_group = coin();
  then `pack_instant_message(agent, from_group, session, other, "Oracle Tester",
  name, IM_ONLINE, dialog, TXN, 0, null region, <1,2,3>, 0, null_bucket ? null :
  bucket, bsize)` and `gAgent.sendReliableMessage()`. Case id `pack_%04d`.
- `folder_sweep` (seed 0x523442464f4c0003, i = 0..1499): the tree is
  `gen(0)`, depth first: name = below(100) ("f<n>"); incomplete = below(25) == 0;
  kids = below(depth ≥ 3 ? 4 : 9); for each kid: at depth < 3, sub = below(4) == 0
  (no draw at depth 3); a sub-folder is `gen(depth + 1)`; otherwise an item:
  worn = below(8) == 0 ? 1 + below(2) : 0 (1 attached, 2 wearable);
  transfer = below(6) != 0; copy = below(4) != 0; type = [0, 5, 6, 7, 10, 13, 20,
  21, 24][below(9)]; name = below(1000) ("i<n>"). Then limit = below(5) == 0 ?
  100 : 42; give; when a question is pending, option = below(4) == 0 ? 1 : 0.
  Records: the `input.NN` lines (ids from a counter per case:
  `0000002c-...-<n>` folders, `00000021-...-<n>` items, in creation order).
- `predicate_sweep` (seed 0x5234425052450004, i = 0..19999):
  avatar_valid = below(10) != 0; owner = [agent, other, group][below(3)];
  with_group = coin() (the permission's group is GROUP or null);
  next, group, everyone, owner, base masks = next() each, in that order (low 32
  bits), passed to `initMasks(base, owner, everyone, group, next)`; a group owner
  is set with `setOwnerAndGroup(null, null, GROUP, true)`; type = [0, 2, 5, 6, 7,
  13, 20, 21, 24, 25, 56][below(11)]; worn = below(4) (none, attached, wearable,
  active gesture; a link's target is the worn id). The out line starts with the
  resolved masks.

These orders reproduce the values recorded before the draws were sequenced
(MSVC and GCC both evaluated the argument lists right to left): every digest
replayed unchanged.

## Behaviour recorded as it is (not fixed)

- Giving to a muted avatar unmutes them (with the auto-unmute notice).
- No-copy gives delete the item locally on "Yes" whatever the give did, and a
  folder's uncopyable items are deleted even when the give failed.
- The folder bucket reuses the root's type byte for every sub-category, counts
  exclude the root (43 entries go out under the limit of 42), and the bucket is
  silently truncated to `1200 - strlen(name)`; a link is given with the linked
  item's type byte and the link's own id.
- The share dialog gives only the first folder, in UUID order.
- `move_inv_category_world_to_agent`'s accept flag is sticky.

## Runtime edges

Names longer than 63 bytes and names with control bytes cannot come from the
inventory model (`LLInventoryObject::correctInventoryName`); the `name_raw_*`
cases set them directly. Folder child order is the order the fake model holds
the children (the real model's order is survey 2's); the input lines record it.

## Platform differences

See `../offer/README.md`.
