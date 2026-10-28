//! C1 type tables: regenerates every line of `fixtures/model/c1_types.txt`
//! from the crate, as `c1_types()` in `llinventory_tool.cpp` writes it from
//! the frozen C++, and compares them line by line.

mod common;

use common::*;
use vkstorm_inventory::category_obj::Category;
use vkstorm_inventory::flags::{self, *};
use vkstorm_inventory::perms::{self, mask_to_string, PERM_COPY, PERM_EXPORT, PERM_MODIFY, PERM_MOVE, PERM_TRANSFER};
use vkstorm_inventory::types::sale::DEFAULT_PRICE;
use vkstorm_inventory::types::{
    asset, folder, inventory, inventory_and_asset_types_match, AssetType, FolderType, IconName, InventoryType,
    SaleType, SettingsType,
};

const FIXTURE: &str = "c1_types.txt";

/// `c1_values()`: -129..=256, then `INT_MIN` and `INT_MAX`.
fn c1_values() -> impl Iterator<Item = i32> {
    (-129..=256).chain([i32::MIN, i32::MAX])
}

/// `c1_names()`.
#[rustfmt::skip]
const C1_NAMES: &[&[u8]] = &[
    b"", b"-1", b"invalid", b"ensemble", b"llassettype_bad_lookup", b"llfoldertype_bad_lookup",
    b"texture", b"sound", b"callcard", b"landmark", b"script", b"clothing", b"object", b"notecard", b"category",
    b"lsltext", b"lslbyte", b"txtr_tga", b"bodypart", b"snd_wav", b"img_tga", b"jpeg", b"animatn", b"gesture",
    b"simstate", b"link", b"link_f", b"widget", b"person", b"mesh", b"settings", b"material", b"gltf", b"glbin",
    b"root", b"snapshot", b"attach", b"wearable", b"animation", b"root_inv", b"trash", b"lstndfnd", b"favorite",
    b"current", b"outfit", b"my_otfts", b"inbox", b"outbox", b"basic_rt", b"merchant", b"stock", b"version", b"suitcase",
    b"not", b"orig", b"copy", b"cntn",
    b"calling card", b"legacy script", b"note card", b"folder", b"lsl2 script", b"lsl bytecode", b"tga texture",
    b"body part", b"targa image", b"jpeg image", b"sym link", b"sym folder link", b"settings blob", b"render material",
    b"GLTF", b"GLTF binary", b"attachment", b"root",
    b"TEXTURE", b"SOUND", b"CALLINGCARD", b"LANDMARK", b"SCRIPT", b"CLOTHING", b"OBJECT", b"NOTECARD", b"CATEGORY",
    b"LSL_TEXT", b"LSL_BYTECODE", b"TEXTURE_TGA", b"BODYPART", b"SOUND_WAV", b"IMAGE_TGA", b"IMAGE_JPEG", b"ANIMATION",
    b"GESTURE", b"SIMSTATE", b"LINK", b"FOLDER_LINK", b"MESH", b"WIDGET", b"PERSON", b"SETTINGS", b"MATERIAL", b"GLTF_BIN",
    b"UNKNOWN", b"NONE", b"texture_tga", b"Texture", b"tExture", b"Sound", b"Notecard", b"Landmark", b"NOT", b"Copy",
    b"texture ", b" texture", b"texturexx", b"landmarks", b"objectxxx", b"lsltextxx", b"unknown", b"none", b"null",
    b"0", b"6", b"255", b"texture\n", b"tex", b"glbinary", b"material ", b"\xc3\xa9", b"folder\t",
];

/// `c1_types<I>()` over the crate.
fn c1_lines() -> Lines {
    let mut s = Lines::default();

    for v in c1_values() {
        let cs = format!("c1.v.{v}");
        let at = AssetType(v);
        s.emit(
            &cs,
            "at",
            &[
                cstr(Some(at.lookup())),
                cstr(at.lookup_human_readable()),
                esc_str(at.desc()),
                bl(at.can_link()),
                bl(at.is_asset_fetch_by_id_allowed()),
                bl(at.is_asset_id_knowable()),
                bl(at.is_link_type()),
            ],
        );
        let it = InventoryType(v);
        s.emit(
            &cs,
            "it",
            &[
                esc_str(it.lookup()),
                esc_str(it.lookup_human_readable()),
                d(InventoryType::default_for_asset_type(at).0),
                bl(it.cannot_restrict_permissions()),
                bl(it.show_in_world_permissions()),
            ],
        );
        let ft = FolderType(v);
        s.emit(
            &cs,
            "ft",
            &[
                esc_str(ft.lookup()),
                bl(ft.is_protected()),
                bl(ft.is_automatic()),
                bl(ft.is_singleton()),
                bl(ft.is_ensemble()),
                d(ft.to_asset_type().0),
                d(FolderType::from_asset_type(at).0),
            ],
        );
        s.emit(&cs, "sale", &[cstr(SaleType(v).lookup())]);
        let st = SettingsType(v);
        s.emit(&cs, "settings", &[d(st.icon_name().0), esc_str(st.default_name())]);
    }

    for it in -1..=255 {
        let bits: String = (-1..=255)
            .map(|at| {
                if inventory_and_asset_types_match(InventoryType(it), AssetType(at)) { '1' } else { '0' }
            })
            .collect();
        s.emit(&format!("c1.match.{it}"), "at-1..255", &[bits]);
    }

    for (i, n) in C1_NAMES.iter().enumerate() {
        s.emit(
            &format!("c1.name.{i}"),
            "lookup",
            &[
                esc(n),
                d(AssetType::from_name(n).0),
                d(AssetType::from_name_cstr(Some(n)).0),
                d(AssetType::from_human_readable(n).0),
                d(AssetType::from_human_readable_cstr(Some(n)).0),
                d(AssetType::from_desc(n).0),
                d(InventoryType::from_name(n).0),
                d(FolderType::from_name(n).0),
                d(SaleType::from_name_cstr(Some(n)).0),
            ],
        );
    }
    s.emit(
        "c1.name.null",
        "lookup",
        &[
            "NULL".to_owned(),
            d(AssetType::from_name_cstr(None).0),
            d(AssetType::from_human_readable_cstr(None).0),
        ],
    );
    s.emit_llsd("c1.typenames", "asset", &AssetType::type_names());
    s.emit_llsd("c1.typenames", "folder", &FolderType::type_names());
    s.emit("c1.badlookup", "values", &[esc_str(asset::BADLOOKUP), esc_str(folder::BAD_LOOKUP)]);

    // LLSettingsType::fromInventoryFlags.
    let flag_values = (0..256u32).chain([0x100, 0x101, 0x1ff, 0x1f1102, 0x7fffffff, 0x80000002, 0xffffffff]);
    for f in flag_values {
        s.emit(
            &format!("c1.flags.{}", x8(f)),
            "settings_from_flags",
            &[d(SettingsType::from_inventory_flags(f).0)],
        );
    }

    // LLCategory over the defined domain (top level 0..6).
    for top in 0..=6u32 {
        for b1 in [0u32, 1, 2, 255] {
            for b2 in [0u32, 1] {
                for b3 in [0u32, 1] {
                    let value = (top << 24) | (b1 << 16) | (b2 << 8) | b3;
                    let c = Category::new(value);
                    let sub = |n: u8| x8(c.sub_category(n).as_u32());
                    s.emit(
                        &format!("c1.cat.{}", x8(value)),
                        "category",
                        &[
                            x8(c.as_u32()),
                            d(c.sub_category_count()),
                            cstr(c.lookup_name()),
                            sub(0),
                            sub(1),
                            sub(5),
                            sub(255),
                        ],
                    );
                }
            }
        }
    }
    let mut r = Rng::new(0x7631_0000_0000_0001);
    for _ in 0..64 {
        let value = r.next() as u32;
        s.emit(&format!("c1.catu32.{}", x8(value)), "category", &[x8(Category::new(value).as_u32())]);
    }
    let none = Category::default();
    s.emit(
        "c1.cat.default",
        "category",
        &[x8(none.as_u32()), d(none.sub_category_count()), cstr(none.lookup_name())],
    );
    s.emit("c1.cat.none", "category", &[x8(Category::NONE.as_u32())]);

    // Transaction flags.
    for a in 0..2 {
        for b in 0..2 {
            s.emit(
                &format!("c1.tf.pack.{a}{b}"),
                "flags",
                &[d(pack_transaction_flags(a != 0, b != 0))],
            );
        }
    }
    let bits = |f: fn(u8) -> bool| -> String { (0..=255u8).map(|v| if f(v) { '1' } else { '0' }).collect() };
    s.emit("c1.tf.is", "src0..255", &[bits(is_tf_source_group)]);
    s.emit("c1.tf.is", "dst0..255", &[bits(is_tf_dest_group)]);
    s.emit("c1.tf.is", "own0..255", &[bits(is_tf_owner_group)]);

    let (a, b) = (id_a(), id_b());
    let tm: [(i32, [u8; 16], [u8; 16], &[u8], i32, &[u8]); 12] = [
        (0, a, b, b"Bob", TRANS_OBJECT_SALE, b""),
        (10, NUL, b, b"Bob", TRANS_OBJECT_SALE, b"desc"),
        (10, a, NUL, b"", TRANS_GROUP_CREATE, b""),
        (10, a, NUL, b"", TRANS_GROUP_JOIN, b""),
        (10, a, NUL, b"", TRANS_UPLOAD_CHARGE, b""),
        (10, a, NUL, b"", 0, b"x"),
        (25, a, b, b"Bob Resident", TRANS_OBJECT_SALE, b"Thing"),
        (25, a, b, b"Bob Resident", TRANS_OBJECT_SALE, b""),
        (25, a, b, b"Bob Resident", TRANS_LAND_SALE, b""),
        (25, a, b, b"Bob Resident", TRANS_LAND_PASS_SALE, b""),
        (25, a, b, b"Bob Resident", TRANS_GROUP_LAND_DEED, b""),
        (-5, a, b, b"Bob", 0, b"neg"),
    ];
    for (i, (amount, ta, tb, name, ty, desc)) in tm.iter().enumerate() {
        // The recorder passes (a, b) to both: a is the source of the first
        // message and the destination of the second.
        let src = build_transfer_message_to_source(*amount, ta, tb, name, *ty, desc, DEFAULT_CURRENCY);
        let dst = build_transfer_message_to_destination(*amount, ta, tb, name, *ty, desc, DEFAULT_CURRENCY);
        s.emit(&format!("c1.tm.{i}"), "messages", &[esc(&src), esc(&dst)]);
    }

    // mask_to_string over the V M C T X alphabet and the edges.
    for m in c1_masks() {
        s.emit(
            &format!("c1.mask.{}", x8(m)),
            "mask_to_string",
            &[esc_str(&mask_to_string(m, false)), esc_str(&mask_to_string(m, true))],
        );
    }

    // Constants.
    s.emit(
        "c1.const",
        "perm",
        &[
            x8(PERM_TRANSFER),
            x8(PERM_MODIFY),
            x8(PERM_COPY),
            x8(PERM_EXPORT),
            x8(PERM_MOVE),
            x8(perms::PERM_RESERVED),
            x8(perms::PERM_NONE),
            x8(perms::PERM_ALL),
            x8(perms::PERM_ITEM_UNRESTRICTED),
            d(perms::PERM_BASE),
            d(perms::PERM_OWNER),
            d(perms::PERM_GROUP),
            d(perms::PERM_EVERYONE),
            d(perms::PERM_NEXT_OWNER),
        ],
    );
    s.emit(
        "c1.const",
        "counts",
        &[
            d(AssetType::COUNT.0),
            d(AssetType::UNKNOWN.0),
            d(AssetType::NONE.0),
            d(InventoryType::COUNT.0),
            d(InventoryType::UNKNOWN.0),
            d(InventoryType::NONE.0),
            d(FolderType::COUNT.0),
            d(FolderType::NONE.0),
            d(SaleType::COUNT.0),
            d(IconName::COUNT.0),
            d(IconName::SCRIPT_LUAU.0),
            d(IconName::INVALID.0),
            d(IconName::UNKNOWN.0),
            d(IconName::NONE.0),
        ],
    );
    s.emit(
        "c1.const",
        "misc",
        &[
            d(DEFAULT_PRICE),
            d(flags::TASK_INVENTORY_ITEM_KEY),
            d(flags::TASK_INVENTORY_ASSET_KEY),
            d(flags::MAX_INVENTORY_BUFFER_SIZE),
            d(inventory::SST_LSL),
            d(inventory::SST_LUA),
        ],
    );
    s.emit(
        "c1.const",
        "transaction_flags",
        &[
            d(TRANSACTION_FLAGS_NONE),
            d(TRANSACTION_FLAG_SOURCE_GROUP),
            d(TRANSACTION_FLAG_DEST_GROUP),
            d(TRANSACTION_FLAG_OWNER_GROUP),
            d(TRANSACTION_FLAG_SIMULTANEOUS_CONTRIBUTION),
            d(TRANSACTION_FLAG_SIMULTANEOUS_CONTRIBUTION_REMOVAL),
        ],
    );
    s.emit(
        "c1.const",
        "item_flags",
        &[
            x8(II_FLAGS_SHARED_SINGLE_REFERENCE),
            x8(II_FLAGS_LANDMARK_VISITED),
            x8(II_FLAGS_OBJECT_SLAM_PERM),
            x8(II_FLAGS_OBJECT_SLAM_SALE),
            x8(II_FLAGS_OBJECT_PERM_OVERWRITE_BASE),
            x8(II_FLAGS_OBJECT_PERM_OVERWRITE_OWNER),
            x8(II_FLAGS_OBJECT_PERM_OVERWRITE_GROUP),
            x8(II_FLAGS_OBJECT_PERM_OVERWRITE_EVERYONE),
            x8(II_FLAGS_OBJECT_PERM_OVERWRITE_NEXT_OWNER),
            x8(II_FLAGS_OBJECT_HAS_MULTIPLE_ITEMS),
            x8(II_FLAGS_SUBTYPE_MASK),
            x8(II_FLAGS_PERM_OVERWRITE_MASK),
        ],
    );
    s
}

/// The `mask_to_string` inputs: the 32 subsets of {V, M, C, T, X}, then the
/// mask edges.
fn c1_masks() -> Vec<u32> {
    let alphabet = [PERM_MOVE, PERM_MODIFY, PERM_COPY, PERM_TRANSFER, PERM_EXPORT];
    let subsets = (0..32u32).map(|i| {
        alphabet
            .iter()
            .enumerate()
            .filter(|(k, _)| i & (1 << k) != 0)
            .fold(0, |m, (_, bit)| m | bit)
    });
    subsets.chain(MASK_EDGES).collect()
}

/// The fixture lines this build must give. The recording is the Release
/// viewer's, without `OPENSIM`, so both `mask_to_string` forms have 4
/// columns. With the crate's `opensim` feature the `isOpenSim` form has the
/// export column as well: that column is derived here from the mask, and
/// the recorded four columns must still match.
fn expected_lines() -> Vec<String> {
    let lines = read_fixture(FIXTURE);
    if !cfg!(feature = "opensim") {
        return lines;
    }
    lines
        .into_iter()
        .map(|line| {
            let cols = split_tabs(&line);
            match cols.as_slice() {
                [case, "mask_to_string", plain, opensim] => {
                    let mask = u32::from_str_radix(case.trim_start_matches("c1.mask."), 16).expect("mask case id");
                    let x = if mask & PERM_EXPORT != 0 { 'X' } else { ' ' };
                    format!("{case}\tmask_to_string\t{plain}\t{opensim}{x}")
                }
                _ => line,
            }
        })
        .collect()
}

#[test]
fn c1_types_replay() {
    let expected = expected_lines();
    assert_eq!(expected.len(), 2848, "{FIXTURE}: data line count");
    let actual = c1_lines();
    assert_replay(FIXTURE, &expected, &actual.0);
}
