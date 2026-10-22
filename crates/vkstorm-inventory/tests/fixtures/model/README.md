# Inventory data model fixtures (rung seven, recorder R1)

These files record the behaviour of the viewer's inventory data model
(`indra/llinventory`, `llcommon/llassettype`) before its port to Rust. Each
form is recorded byte for byte: the legacy text, the LLSD forms, the message
blocks, the CRCs and the type tables. Each read result is recorded field by
field, with the stream position and state after every legacy read. The port
must reproduce them exactly. Survey 1 §7 (corpora C1-C10) is the
specification. Every corpus but C10 comes from the dump mode of
`llinventory_tool`
(`indra/llinventory/tests/llinventory_tool.cpp`), and C10 is the public Linden
Library cache. The values come from the frozen copies in
`indra/llinventory/tests/llinventory_oracle.{h,cpp}`. Those copies were made at
viewer commit `472ecf270d` (branch `codex/rust-inventory`) and are checked by
`check_r1_copies.py`, so a later regeneration still records the C++ as it was
before the port.

The same bytes are kept in the viewer (`indra/llinventory/tests/fixtures/`)
and in vkstorm-core-rust (`crates/vkstorm-inventory/tests/fixtures/model/`).
`tools/inventory-fixtures/model/regenerate.py` in vkstorm-core-rust writes both.
Never edit them by hand.

`INTEGRATION_TEST_llinventory_parity` (viewer) checks the live classes in two
ways. It compares them with the frozen copies in process, over every corpus.
It also replays these files against them: the structured corpora line by line,
the random streams by digest and by sample, the exhaustive sweeps by digest,
and C10 field by field. A difference is reported as
`<corpus>: case <id> field <field>.<column>: expected <x> actual <y>`.

Recorded on Windows (MSVC 19.44.35229, RelWithDebInfo, the Release feature
set) and on Linux (WSL Ubuntu 26.04 with GCC 15.2 and with GCC 14.3, CI's Linux
configuration). The committed files are the Windows recording. The Linux
recording has the same bytes in every file apart from the `# platform:` header
line (see *Runtime edges* for the five values that differ by C runtime and how
they are written).

## Files

| file | corpus | cases | lines |
|---|---|---:|---:|
| `c1_types.txt` | C1 type tables: `LLAssetType`, `LLInventoryType`, `LLFolderType`, `LLSaleInfo` names, `LLSettingsType`, `inventory_and_asset_types_match` over (-1..255)², reverse lookups, `getTypeNames`, `LLCategory`, transaction flags and messages, `mask_to_string`, constants | 1280 | 2848 |
| `c2_normalisers.txt` | C2 `correctInventoryName`, `correctInventoryDescription`, bare `replaceNonstandardASCII` with and without `\|`, `rename`, the constructors, `setDescription` | 1234 | 2468 |
| `c3_permissions.txt` | C3 structured permission cases | 3170 | 3290 |
| `c3_random_sample.txt` | C3 random stream, sample (stream: 200000 cases, digest in `digests.txt`) | 399 | 1596 |
| `c4_sale.txt` | C4 structured sale-info cases | 394 | 1176 |
| `c4_random_sample.txt` | C4 random stream, sample (stream: 100000 cases) | 299 | 973 |
| `c5_items.txt` | C5 generated items, full records | 280 | 6424 |
| `c5_matrix.txt` | C5 every asset type x inventory type, compact (full records: digest `c5mf`) | 972 | 972 |
| `c5_random_sample.txt` | C5 random items, sample (stream: 20000 cases) | 19 | 437 |
| `c5_categories.txt` | C5 generated categories (all four LLSD dialects, legacy) and `LLInventoryObject` | 151 | 3522 |
| `c6_legacy.txt` | C6 malformed legacy text through the item, object, category, permissions and sale-info readers, and notecards through `LLNotecard::importStream` | 253 | 2666 |
| `c7_item_llsd.txt` | C7 LLSD input of `LLInventoryItem::fromLLSD` | 2105 | 2195 |
| `c8_category_llsd.txt` | C8 LLSD input of `LLInventoryCategory::fromLLSD`, `importLLSD`/`importLLSDMap` and the viewer subclass's cache keys | 656 | 1520 |
| `c9_messages.txt` | C9 message blocks through the real `LLMessageSystem` with `scripts/messages/message_template.msg` | 829 | 2213 |
| `c10_library.txt.gz` | C10 every Library item and category: `fromLLSD`/`importLLSDMap`, byte identity of the re-serialization, the fields (gzip of text) | 6075 | 6076 |
| `digests.txt` | digests of the random streams and the exhaustive sweeps | | 9 |
| `library/ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz` | the Linden Library inventory cache, unchanged (see C10) | | |

## Line format

Lines starting with `#` are headers (generator, viewer source, platform, seed,
format). They are not compared. Every other line is

```
<case>\t<field>\t<value>[\t<value>...]
```

Values are escaped: bytes 0x20-0x7e as they are, except `\` written `\\`;
every other byte (TAB and LF included) as `\xHH`. UUIDs are lower-case and
dashed. Masks, flags and CRCs are 8 lower-case hex digits. Integers are
decimal (`date` is 64-bit). Booleans are `0`/`1`. Byte strings (packets,
binary LLSD) are lower-case hex. An LLSD value is two lines,
`<field>.bin` (the binary serialization, `LLSDSerialize::toBinary`, which is
deterministic) and `<field>.txt` (notation, for review). Fields with only
`.bin` have no notation line.

### Records

A field holding a record has these columns, sometimes with `ret` (the
function's return value) or `same` first. The comparer names the columns in
its failure messages.

- item: `id parent asset thumbnail type inv_type flags creator owner last_owner group base owner_mask group_mask everyone next group_owned sale_type sale_price date favorite runtime name desc` (24 columns; `type`, `inv_type`, `sale_type` as raw integers)
- category: `id parent type pref_type thumbnail favorite date runtime name`
- viewer category: category + `owner version` (`LLViewerInventoryCategory`'s cache keys)
- object: `id parent type thumbnail favorite date runtime name`
- permissions: `creator owner last_owner group base owner_mask group_mask everyone next group_owned`
- sale: `type price`
- stream: `state pos rest`. `state` is `good` or a combination of `e` (eofbit), `f` (failbit) and `b` (badbit). `pos` is the get position of the stream buffer (it is valid after a failure too). `rest` is the unread input.

A reader's `ret` comes before the stream columns, as in `ret state pos rest`.

### Marked values

- `UB`: the C++ reads an uninitialised variable here. The value is not
  comparable, and the comparer skips it. The cases, and what each platform
  produced, are listed below.
- `RT(win=<x>;linux=<y>)`: the value depends on the C runtime. The recorder
  writes this form only when the platform's own value equals the stated one.
  Otherwise it writes `RT-MISMATCH(...)`, which fails the comparison.
- `SKIPPED: null sale type name (runtime edge, see README)`: a legacy export of
  a sale type outside 0..3 would stream a null `const char*` (see below).

## Fixed inputs

- `U(tag)`: the UUID whose bytes are `tag 01 23 45 67 89 ab cd ef 10 32 54 76 98 ba tag`. `A=U(0a)` is an owner, `B=U(0b)` another agent, `C=U(0c)` a creator, `G=U(06)` a group and `H=U(07)` another group. `MAGIC` is `3c115e51-04f4-523c-9fa6-98aff1034730`, and `NOT_MAGIC` is its byte complement. Case ids name agents by these letters, `null` for the null UUID.
- Items, categories and objects are built with every field set raw, bypassing the normalising setters, by `RawItem`/`RawCat`/`RawObj` in the tool. Permissions are built with `init(creator, owner, last, group)`, then the five raw `setMask*`, then `yesReallySetOwner(owner, group_owned)`. Sale info is built with its raw fields.
- The base item (`base_item()`): id `U(a1)`, parent `U(a2)`, asset `U(a3)`, type 6, inv_type 6, flags 0, creator C, owner A, last owner B, group G, masks `7fffffff 7fffffff 0 0 00082000`, sale (0, 10), date 1700000000, name `Base item`, desc `Base description`.
- The previous item (`prev_item()`) is the object every reader starts from, so that the fields a reader does not reset are recorded: id `U(11)`, parent `U(12)`, asset `U(13)`, thumbnail `U(14)`, type 7, inv_type 7, flags `00000101`, ids `U(21) U(22) U(23) U(24)`, masks `7fffffff 0008e000 00002000 0 00082000`, sale (2, 25), date 1234567890, favorite, runtime `pre-runtime`, name `pre name`, desc `pre desc`. The previous category and object (`prev_cat()`): id `U(31)`, parent `U(32)`, thumbnail `U(34)`, type 8, pref_type 5, favorite, date 99, runtime `pre-rt`, name `pre folder`, owner `U(33)`, version 3. The previous permissions are `prev_item()`'s. The previous sale info is (1, 77), with `has_perm_mask` = 1 and `perm_mask` = `a5a5a5a5` on entry.
- The legacy readers are called after the caller has consumed the `inv_item\t0` (or `inv_object`, `inv_category`, `permissions`, `sale_info`) line, as the notecard and task-inventory readers call them. Their input starts at the `\t{` line.
- Messages carry AgentID `U(f1)`, SessionID `U(f2)`, TransactionID `U(f3)` (also the viewer item's transaction id), LocalID `01020304` and Key 0.

## The corpora

### C1 type tables

For every value -129..256, plus `INT_MIN` and `INT_MAX` cast to the enum, the
fields are:
- `at`: `lookup`, `lookupHumanReadable` (`NULL` for -1 and 255), `getDesc`, `lookupCanLink`, `lookupIsAssetFetchByIDAllowed`, `lookupIsAssetIDKnowable` and `lookupIsLinkType`.
- `it`: `lookup`, `lookupHumanReadable`, `defaultForAssetType` of the same value, `cannotRestrictPermissions` and `showInWorldPermissions`.
- `ft`: `lookup`, the four predicates, `folderTypeToAssetType` and `assetTypeToFolderType`.
- `sale`: `LLSaleInfo::lookup`.
- `settings`: `getIconName` and `getDefaultName`, with a translation bridge that translates nothing.

Values outside an enumeration's range are formally unspecified in C++.
Both compilers store the integer.

The other C1 lines are:
- `c1.match.<it>`: `inventory_and_asset_types_match(it, at)`, one character per asset type -1..255.
- `c1.name.<n>`: the reverse lookups of 138 names and junk strings, through both the `std::string` and the `const char*` overloads.
- `getTypeNames` of both tables.
- `fromInventoryFlags` for 263 flag values.
- `LLCategory` over its defined domain. The top byte is 0..6; other top bytes index past `CATEGORY_INDEX`, which is undefined behaviour, and are not recorded except through `init`/`getU32`.
- The transaction-flag helpers and `build_transfer_message_*`.
- `mask_to_string`, both forms, over the V M C T X alphabet and the mask edges. The Release build has no `OPENSIM`, so both forms are 4 characters.
- The constants.

### C2 normalisers

The inputs are:
- each byte value alone, leading, trailing and inside a string;
- lengths 0-70, 120-130 and 250-260;
- every pair of the six C whitespace characters at both ends;
- cuts around 63 bytes;
- UTF-8 of 2, 3 and 4 bytes, emoji sequences, invalid UTF-8 and embedded NUL;
- `|` at every position, CRLF, DEL and TAB.

`out` holds these columns, in order:
- `correctInventoryName`;
- `correctInventoryDescription`;
- `replaceNonstandardASCII` then `replaceChar('|')`;
- `replaceNonstandardASCII` alone;
- `rename` of an object named `prev`;
- the item constructor's name and description;
- `setDescription` over `prev desc`;
- the category constructor's name.

### C3 permissions

`c3_permissions.txt` covers:
- `fix` over the mask edges (base x the rest, both values of `mIsGroupOwned`);
- `initMasks` with five arguments (`fixFairUse` + `fix`) over base x next;
- `initMasks(inventory type)` for -1..30 and 255;
- `accumulate` over all pairs of eight sample states (`c3_states()`);
- `setOwnerAndGroup` over agent/owner/group in {null, A, B, G}, both atomic forms, from four states;
- `deedToGroup`;
- each `set*Bits`, set and clear, over six bit sets, agents {null, A, B} and groups {null, G, H};
- for every state: the queries, `getSafeOwner` (owned states only, because it generates a random UUID otherwise), `setLastOwner`, `set`, `==`/`!=`, the legacy text and its re-import, the LLSD form and its re-import, `ll_fill_sd_from_permissions` into a non-empty map, `importLLSD` over another state, and the hyphenated LLSD `packMessage`/`unpackMessage`, which have no caller;
- `importLLSD` of non-maps;
- `LLAggregatePermissions::aggregate(mask)` sequences of length 0-3 over six masks.

The mask edges are 0, `7fffffff`, `ffffffff`, `e000`, the single bits T M C EXPORT MOVE, bit 0 and bit 31, and the seven masks of the debugging key in `llpermissionsflags.h`.

The queries are `allow` (`allowOperationBy` for requester {null, A, B, G, H} x group {null, G, H} x op {T, M, C, MOVE, EXPORT, 0, `7fffffff`}, 105 characters, op fastest), `helpers` (for each requester: `allowModifyBy`, `allowCopyBy`, `allowMoveBy` with no group and with G, then `allowTransferTo` and `allowExportBy`), `crc` (`getCRC32`), and `owned owner_id owner_is_group` from `getOwnership`, which starts from `U(ee)`/true. `is_owned is_group_owned` follow.

The random stream `c3r` has 200000 cases. Each case is `in`, `op`, `out` (`ret` + permissions) and `queries`. The ops are `accumulate`, `setOwnerAndGroup`, `deedToGroup`, the five `set*Bits`, `initMasks` with five masks, `fix`, the LLSD round trip and the legacy round trip.

The exhaustive sweeps run in process only and are recorded as digests:
- `c3x.fix.alpha5.go<0|1>`: `fix()` over every assignment of the 5-bit alphabet {T, M, C, MOVE, EXPORT} to the five masks (2^25), with `mIsGroupOwned` 0 and 1;
- `c3x.fix.alpha4.go<0|1>`: the same over {T, C, bit 0, bit 31} (2^20). Together with the first sweep this covers bits 0 and 31; a single 7-bit alphabet over five masks would be 2^35 cases;
- `c3x.initmasks5`: `initMasks(b, o, e, g, n)` with base and next over the 7-bit alphabet {T, M, C, MOVE, EXPORT, bit 0, bit 31} and owner, everyone, group over {`7fffffff`, 0, T|C, M|MOVE, `ffffffff`} (2048000 cases).

The enumeration order is in `c3x_fix`/`c3x_fair`. Each case appends the five resulting masks (base, owner, group, everyone, next) as little-endian U32, and the digest is the SHA-256 of all those bytes.

### C4 sale info

`c4_sale.txt` covers:
- types {0, 1, 2, 3, -1, 4, 200, 255} x prices {`INT_MIN`, -1, 0, 10, `INT_MAX`}: the raw state, the CRC, the two-argument constructor, the setters, the legacy text and its re-import (valid types only), the LLSD form and its re-import, the `operator LLSD`, `ll_create_sd_from_sale_info`/`ll_sale_info_from_sd`, and the hyphenated `packMessage`/`unpackMessage`;
- `accumulate` over nine sale states, all pairs (`c4.acc`), including the price overflow (a runtime edge, below);
- `fromLLSD` over 20 `sale_type` values x 13 `sale_price` values, absent included;
- `perm_mask` as Binary of 0/3/4/5 bytes, as Integer, as String and as undef;
- non-map inputs.

The random stream `c4r` has 100000 cases. Sums that would overflow S32 are not drawn; they print `overflow`.

### C5 items and categories

`c5_items.txt` has one full record per case:
- every asset type with its default inventory type (`c5.at.*`);
- every inventory type on an object asset (`c5.it.*`);
- the base masks x the assets {null, `U(a3)`, MAGIC, NOT_MAGIC} x types {object, notecard, landmark, lsltext} (`c5.mask.*`);
- thumbnail x favorite x runtime {"", `luau`, 60 bytes, `lu au`} x {lsltext, notecard} (`c5.meta.*`);
- the flags x {object, clothing, settings} (`c5.flags.*`);
- sale types and prices (`c5.sale.*`);
- 16 name/description pairs (`c5.name.*`);
- dates {0, 1, `INT_MAX`, `INT_MIN`, 2^31, 2^32+5, -1};
- group ownership, default and unfixed permissions, a link and a folder link.

The base masks are `7fffffff`, no-modify, no-copy, no-transfer, no-copy-no-transfer, 0 and `e000`. The flags are 0, 1, `ff`, `100`, `1f1100`, `7fffffff` and `ffffffff`.

A record holds these fields:
- `in`;
- `legacy`: `exportLegacyStream` with the asset key;
- `legacy.nokey`: the asset line written without the key, and whether the rest is byte-identical;
- `legacy.ret`;
- `legacy.reimport`: into the previous item, plus its stream;
- `legacy.object`: `LLInventoryObject::exportLegacyStream` on the item;
- `llsd`: `asLLSD`;
- `llsd.into`: `asLLSD(LLSD&)` into a map that already holds `preexisting` and `name`. The columns are the keys afterwards, whether every written value equals `asLLSD()`'s, and `preexisting`;
- `crc`;
- `derived`: `getLinkedUUID`, `getIsLinkType`, `getInventorySubType`, `getCreatorUUID` and `getType`;
- `fromllsd.new`: `fromLLSD(asLLSD)` into a new item, then `ret` and whether the re-serialization is identical;
- `fromllsd.reuse`: `is_new=false` over the previous item;
- `fromllsd.opensim`: with `LLPermissions::setupIsInOpenSim(true)`;
- `sdcreate.bin`: `ll_create_sd_from_inventory_item`;
- `slam`: `accumulatePermissionSlamBits` against the previous item, both ways;
- `setpermissions`;
- `copy`: the copy constructor;
- `subtype`: `setInventorySubType(0x7e)`;
- `copyitem`: `copyItem` over the previous item;
- `ctor`: the full constructor.

`c5_matrix.txt` has these columns: the legacy `type` and `inv_type` tokens (`-` when absent), the LLSD `type` and `inv_type` (`S:` String or `I:` Integer), the CRC, the type, inventory type and base mask after legacy re-import, the same plus `everyone` after `fromLLSD`, and `getLinkedUUID`. The full records of the same 972 items are the `c5mf` digest.

`c5r` has 20000 random items (`draw_item`). Its dates stay within S32, so that the legacy round trip stays clear of the `%d` overflow.

`c5_categories.txt` has one record per category:
- every preferred type -1..101, 127 and -128;
- thumbnail and favorite combinations on six types;
- the 16 names;
- types -1, 255, 6 and 24;
- a runtime;
- null ids.

A category record holds:
- the category legacy text and its re-import;
- the object legacy text and its re-import;
- `asLLSD` (dialect 1, `item_id`);
- `asAISCreateCatLLSD` (dialect 2);
- `exportLLSD` (dialect 4, base);
- `vexport`: the viewer subclass's `exportLLSD` (owner `U(d3)`, version 7);
- `fromLLSD` (dialect 3) of dialects 1 and 2 over the previous category;
- `importLLSDMap` of dialect 4 into a new category;
- the viewer `importLLSDMap` and its identity;
- `ll_create_sd_from_inventory_category` and `ll_create_category_from_sd`;
- the copy constructor and the constructor.

`c5o.*` are `LLInventoryObject` records.

### C6 malformed legacy input

Each case has an `input` line (the reader letters, then the input). It then
has the result of each reader:
- `I`: item (`item`, `item.stream`);
- `O`: object (`obj`);
- `C`: category (`cat`);
- `P`: permissions (`perm` = `ret` + permissions, `perm.stream`);
- `S`: sale info (`sale` = type price has_perm_mask perm_mask ret, `sale.stream`);
- `N`: notecard. `notecard` holds ret, version, embedded version, item count and text. It is followed by `notecard.item.<k>`, `notecard.stream` and, when the import succeeded, `notecard.export` (`exportStream`).

Most inputs are the standard item text (`std_item_lines()`, 28 lines) with one
change, followed by `NEXT\tline\n`, so that the stream position after the
item is visible. The cases are:
- a blank line after each line;
- whitespace-only and `\r` lines;
- EOF at several points, including right after the permissions and sale blocks;
- keyword-only lines (stale values);
- lines of 253/254/255/256/300 bytes, and of 16382-16385 bytes inside the permissions block;
- names and descriptions: empty, without `|`, with leading TABs or spaces, with a space separator, with inner TABs and `|`, with CR, high bytes and control bytes;
- 13 metadata forms, after the permissions block and at the end: valid thumbnail, script and favorite, all three, not LLSD, malformed XML, over 254 bytes, notation on the next line, a runtime with a space, no orphan `|`, empty maps;
- `asset_id`/`shadow_id` in both orders;
- eight UUID spellings (upper case, 35 characters, other characters in the hyphen slots, invalid, 37, 34, no hyphens, empty);
- legacy `creator_mask`, `group_owned`, stale and malformed masks;
- `perm_mask` 0, no-copy, `e000`, `ffffffff` and keyword-only;
- sale types and prices;
- `flags` and `creation_date` spellings;
- unknown keywords and braces;
- type and inventory-type names.

Object, category, sale-block and permissions-block inputs, empty inputs, and
16 notecards complete the corpus. The notecards are: empty; one item; three
items; versions 1 and 3; an item with a blank line after its permissions
block; a long line; a metadata line; an item without its closing brace; a
text length that is too short and one that is too long; count -1; count 2
with one item; CRLF; garbage; UTF-8 text. `c6.nc.export` is a notecard of
three C5 items written by `exportStream` and read back.

`LLViewerObject::loadTaskInvFile` is not in this corpus: recorder R4a owns it.

### C7 item LLSD input

`c7.<key>.<value>.<is_new>` covers 20 keys x 48 LLSD values, each a one-key
map, with `is_new` 1 and 0, over the previous item. `ret` + item is recorded.
The keys are the item keys plus `callback_id` and `hash_id`. The values are:
undef; both booleans; integers 0 1 -1 7 255 256 300 max min; reals; 13
strings; null and non-null UUIDs; a date; a URI; binary of 0, 3, 4 and 5
bytes; eight maps; two arrays.

The other C7 cases are:
- `c7.opensim.*`;
- `c7.doc.*`: whole-document non-maps;
- `c7.assetkeys.<mask>`: the `asset_id`/`linked_id`/`shadow_id` subsets;
- `c7.thumbs.*`: `thumbnail` with `thumbnail_id`;
- `c7.script.*`: `script`, `metadata.script`, both, and `metadata` without `script`;
- `c7.permmask.*`: `sale_info.perm_mask` forms, with and without `permissions`;
- `c7.perm.<key>.<value>`: permissions keys x value types, with `ll_permissions_from_sd`;
- `c7.perm.derived_group`: `is_owner_group` is never read;
- `c7.reuse.*`: one object reused across six records, as the legacy fetch loop does.

### C8 category LLSD input

`c8.<key>.<value>` covers 13 keys x the 48 values: `fromllsd`, then
`importllsd` (`ret` of the single-key `importLLSD`, then `importLLSDMap` of
the map), then `vimportllsd` (the viewer subclass, for `owner_id`, `version`,
`favorite`, `name` and `cat_id`). The keys are `category_id item_id cat_id
parent_id thumbnail thumbnail_id favorite type type_default preferred_type
name version owner_id`.

The other C8 cases are:
- `c8.thumbs.*`;
- `c8.typepair.*`: `type` with `type_default`;
- `c8.reuse.*` and `c8.vreuse.*`: one object across records.

### C9 message blocks

For every C5 item case:
- `UpdateInventoryItem` and `UpdateTaskInventory` are packed by the viewer subclass's `packMessage` (TransactionID in place of AssetID), framed as the viewer's senders frame them;
- `BulkUpdateInventory` is packed by the base `packMessage` (AssetID);
- the `BulkUpdateInventory` packet is unpacked (`LLInventoryItem::unpackMessage`) into a new item (`unpack.fresh`) and into the previous item (`unpack.reused`), with the CRC verdict as `ret`.

A packet is the built message from its message number on: the template
builder's output (`LLMessageSystem::wrapBuiltTemplateData`), before
zero-coding, without the 6-byte header that the sender fills. A packet is read
back through `LLMessageSystem::dispatchTemplate` into the template reader
(rung three's classes).

The other C9 cases are:
- `c9.udp.*`: raw `ItemData` blocks (invalid type bytes, inventory types, sale types 3/4/200/255, prices, the 16 names, raw permissions, date -1, flags) with the CRC the frozen copy computes;
- `c9.udp.multi`: four blocks into one reused object;
- `UpdateInventoryFolder` from `LLInventoryCategory::packMessage` for every C5 category, unpacked again;
- `c9.folder.raw`;
- `c9.agg.<u8>`: `LLAggregatePermissions::unpackMessage` of all 256 bytes, then `packMessage` and the byte read back;
- `c9.aggagg.<a>`: `aggregate(LLAggregatePermissions)` over the 64 valid states, one hex byte per right-hand state;
- `c9.category.*`: `LLCategory` through `ObjectProperties`;
- `c9.sale.*`: `LLSaleInfo::unpackMessage` and `unpackMultiMessage`, and `packMessage`, through `ObjectProperties`.

### C10 the Linden Library cache

`library/ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz` is the public
Linden Library inventory cache, committed unchanged with the user's agreement.
It is `%LOCALAPPDATA%\Firestorm_x64\...` as the viewer wrote it, and its
SHA-256 is `4b68290c1fcf331b17396d4a25b89fcd01e047824b4f265965cf562e6cd6deba`.
It holds library content only: a 4-byte big-endian version (5) and binary
LLSD `{categories, items}`. All 490 categories and 5585 items are owned by
`ba2a564a-f0f1-4b82-9c61-b7520bfcd09f`, the only root is the `Library` folder,
and every item's parent is a Library folder. Recorder R2 reads this file from
the viewer path for the cache container.

`c10_library.txt.gz` records, for every category, `importLLSDMap` into a new
viewer category, its `exportLLSD`, the byte identity (`same`) and the fields.
For every item it records `fromLLSD` (`ret`), the byte identity of `asLLSD`
with the cached map (`same`) and the fields. Every category (490) and every
item (5585) re-serializes byte-identically (`c10.summary`).

## Random streams and digests

The generator is xorshift64* (`Rng` in the tool): `x ^= x >> 12; x ^= x << 25;
x ^= x >> 27; return x * 0x2545F4914F6CDD1D`. The seeds are `c3r`
`0x7633000000000001`, `c4r` `0x7634000000000001` and `c5r` `0x7635000000000001`
(each file's header repeats its seed). The inputs are drawn by `draw_c3`,
`draw_c4` and `draw_item`, with `draw_perm`, `draw_id`, `draw_mask` and
`draw_price`. The draw order is the code's statement order, and a port's test
transcribes those functions. A stream's digest is the SHA-256 of its lines,
each followed by `\n`. `digests.txt` lists `<stream> <cases> <lines>
<sha256>`. The sample files hold the cases with `i < 200 || i % 1000 == 0`
(`c5r`: `i < 10 || i % 2000 == 0`). `c5mf` (the full records of the C5
matrix) has a digest only.

## Runtime edges

These are the values that depend on the C runtime or on undefined behaviour.
Each is stated here, and none is chosen silently.

1. **LLSD String → Integer of `"0x10"`** (`RT(win=16;linux=0)`, five values:
   `c7.created_at.str.0x10.{0,1}`, `c8.type.str.0x10`,
   `c8.type_default.str.0x10`, `c8.version.str.0x10`). `LLSD::asInteger()` of
   a String is `(int)asReal()`, an `istringstream >> F64`. The MSVC runtime
   parses hexadecimal floating point. libstdc++ stops at `x`, so the stream
   does not reach EOF and the value is 0. This is the C++ `LLSD` class
   (llcommon), and the inventory code inherits it through `created_at`,
   `type`, `type_default` and `version`. Every other String coercion in the
   corpora gives the same value on both platforms.
2. **Uninitialised locals in the legacy readers** (`UB`). The legacy readers
   read an uninitialised local in three places:
   - `S32 date` in `LLInventoryItem::importLegacyStream`, when `creation_date`'s value does not parse (`c6.date.abc`, `c6.kw.date` whose stale value is the description, `c6.kw.date.first`);
   - `U32 mask` in `LLPermissions::importLegacyStream`, when the block's first mask or `group_owned` line does not parse (`c6.perm.mask.first_fails`, `c6.perm.group_owned.first_fails`; the four other masks inherit the garbage through `fix()`);
   - `char uuid_str[256]`, when an id keyword is the block's first value-less line (`c6.perm.id.first_empty`).

   The values each platform produced are below. They were stable across runs
   of the same binary, but they are undefined:

   | case | Windows (MSVC) | Linux (GCC 15) |
   |---|---|---|
   | `c6.date.abc` date | 0 | 0 |
   | `c6.kw.date` date | 15 | 0 |
   | `c6.kw.date.first` date | 0 | 91719616 |
   | `c6.perm.mask.first_fails` masks, item reader | `00000030` x5 | `746e6576 746e6576 746e6576 746e2576 746e6576` |
   | `c6.perm.mask.first_fails` masks, permissions reader | `00082000` x5 | `00082000` x5 |
   | `c6.perm.group_owned.first_fails` group_owned | 1 | 1 |
   | `c6.perm.id.first_empty` creator | null | null |

   The port must choose a defined value, as survey 1 §4.3 item 2 proposes.
3. **A null sale-type name in the legacy writer.** `LLSaleInfo::exportLegacyStream`
   with a sale type outside 0..3 streams `lookup()`'s `NULL`. On Windows the
   process crashes (access violation, observed with `llinventory_tool probe
   null-sale-export`). With libstdc++ the stream gets `badbit|failbit` and
   every later write is dropped, so the output ends at `\t\tsale_type\t`. The
   fixtures record `SKIPPED` for these exports, and `asLLSD` writes `not`.
4. **Signed overflow in `LLSaleInfo::accumulate`** (`c4.acc.*` with
   `INT_MAX`). This is undefined behaviour. Both compilers wrap, and the
   wrapped value is recorded. The random stream does not draw overflowing
   sums.
5. **`%d` and `%x` overflow in the legacy readers.** Both C runtimes keep the
   low 32 bits:
   - `flags 123456789` → `23456789`;
   - `fffffffff` → `ffffffff`;
   - `creation_date 2147483648` → -2147483648;
   - `4294967301` → 5;
   - `-2147483649` → 2147483647;
   - `99999999999999999999` → -1;
   - `sale_price 3000000000` → -1294967296, clamped to 0.

   The values are the same on both platforms and are recorded as plain values.
   The C standard leaves them undefined, so the port must reproduce these
   stated values.
6. Enum values outside an enumeration's range (C1 casts, `(S8)(U8)` and U8
   casts in the readers) are formally unspecified. Both compilers store the
   integer, and that is what is recorded.

## Behaviour worth knowing (recorded as it is)

- The item CRC covers the last owner and the thumbnail, and neither travels in
  an item message block. A block packed from an item with a last owner
  therefore fails the CRC check on unpack: 318 of the 319 `unpack.fresh`
  verdicts are 0, and the only verdict of 1 is `c5.perm.default`, whose last
  owner is null. No caller checks the verdict.
- `LLInventoryItem::unpackMessage` does not touch the last owner, thumbnail,
  favorite or runtime. An object reused across blocks (`llinventorymodel.cpp:5523`)
  keeps the previous item's last owner (`c9.udp.multi`, `unpack.reused`).
- All of survey 1 §1.7 and §4.3 is visible in these files. Examples are the
  orphan `|` line after metadata, `permissions 0` with a space, a blank line
  after a block swallowing the rest of the item (`c6.blank.14`,
  `c6.eof.after_perm`), `is_owner_group` never read (`c7.perm.derived_group`),
  `item_id` versus `category_id` in the category dialects, and the favorite
  branch of `LLInventoryCategory::importLLSD` returning false (`c8.favorite.*`
  `importllsd` ret).
