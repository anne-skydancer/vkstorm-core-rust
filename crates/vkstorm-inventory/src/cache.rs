//! The inventory cache file (`LLInventoryModel::saveToFile`, `loadFromFile`
//! and `getInvCacheAddres`, `newview/llinventorymodel.cpp`).
//!
//! The file is a 4-byte big-endian version ([`CACHE_VERSION`], 5), then the
//! binary LLSD map `{categories: [...], items: [...]}`, then a newline (the
//! `std::endl` after the formatter). Each category is the viewer subclass's
//! `exportLLSD` form (`cat_id`, `owner_id`, `version`, ...), each item
//! `LLInventoryItem::asLLSD`. On disk the viewer keeps it gzipped as
//! `<name>.gz`.
//!
//! This module is the codec over the uncompressed bytes. The gzip layer is
//! llcommon's `gzip_file`/`gunzip_file` (zlib-ng, file to file, shared with
//! the rest of the viewer), and the temporary-file sequence of `cache()` and
//! `loadSkeleton` is file-system glue: both stay C++. The corpora never
//! compare `.gz` bytes, only their uncompressed content.
//!
//! Which folders and items `cache()` writes (`LLCanCache` over
//! `collectDescendentsIf`) is a walk of the C++ model and stays C++; this
//! module encodes what it is given, skipping the folders of unknown version
//! as `saveToFile` does.
//!
//! Record order is the caller's. Since upstream 28fee038e0 the model's
//! parent-to-children maps are `std::unordered_map`s, so the order
//! `collectDescendentsIf` yields (and the file's record order with it)
//! depends on the standard library's hash order. [`encode`] writes the
//! records in the order it receives them and never sorts them.

use std::collections::BTreeSet;

use vkstorm_llsd::{Format, Limits, Reader, SliceSource, Value, ValueBuilder, XmlMode};

use crate::category::{VERSION_UNKNOWN, ViewerCategory};
use crate::item::Item;
use crate::llsd::coerce::insert;
use crate::model::{ModelLookup, followed_type};
use crate::perms::{NULL_ID, Uuid};
use crate::types::AssetType;

/// `LLInventoryModel::sCurrentInvCacheVersion`.
pub const CACHE_VERSION: i32 = 5;

/// The cache file name for an owner (`getInvCacheAddres` without the cache
/// directory): `<owner>.inv.llsd` on the Second Life main grid, else
/// `<owner>.<grid>.inv.llsd`. `grid` is the grid id after the viewer's
/// `LLDir::getScrubbedFileName` and `utf8str_tolower` (C++ string
/// utilities the caller applies); `None` for the main grid. The `.gz` file
/// is this name with `.gz` appended.
pub fn cache_file_name(owner: &Uuid, grid: Option<&[u8]>) -> Vec<u8> {
    let mut name = uuid_text(owner).into_bytes();
    if let Some(g) = grid {
        name.push(b'.');
        name.extend_from_slice(g);
    }
    name.extend_from_slice(b".inv.llsd");
    name
}

/// The clear-cache marker `<agent>_DELETE_INV_GZ` (`<FS:Zi>`), in the cache
/// directory.
pub fn delete_marker_name(agent: &Uuid) -> Vec<u8> {
    let mut name = uuid_text(agent).into_bytes();
    name.extend_from_slice(b"_DELETE_INV_GZ");
    name
}

/// `LLUUID::asString`: lower-case hyphenated.
pub fn uuid_text(id: &Uuid) -> String {
    let mut s = String::with_capacity(36);
    for (i, b) in id.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `saveToFile`'s bytes: the version, the binary LLSD of the categories
/// whose version is known (`exportLLSD`) and of every item (`asLLSD`), in
/// the order given, and the newline.
pub fn encode<'a, C, I>(categories: C, items: I) -> Vec<u8>
where
    C: IntoIterator<Item = &'a ViewerCategory>,
    I: IntoIterator<Item = &'a Item>,
{
    let cats: Vec<Value> = categories
        .into_iter()
        .filter(|c| c.version != VERSION_UNKNOWN)
        .map(|c| {
            let mut sd = Value::Undefined;
            c.export_llsd(&mut sd);
            sd
        })
        .collect();
    let items: Vec<Value> = items.into_iter().map(Item::to_llsd).collect();
    let mut body = Value::map();
    insert(&mut body, "categories", Value::Array(cats));
    insert(&mut body, "items", Value::Array(items));
    let mut out = Vec::new();
    out.extend_from_slice(&(CACHE_VERSION as u32).to_be_bytes());
    let (bytes, _) = vkstorm_llsd::format(Format::Binary, &body, Default::default());
    out.extend_from_slice(&bytes);
    out.push(b'\n');
    out
}

/// What `loadFromFile` returns for a file it could open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decoded {
    /// `is_cache_obsolete`: no complete version header, another version, or
    /// a binary LLSD parse failure. When set, nothing was read.
    pub obsolete: bool,
    /// The categories, in file order (`importLLSDMap` into a category of
    /// null owner and unknown version; it accepts any record).
    pub categories: Vec<ViewerCategory>,
    /// The items, in file order (`fromLLSD` with `is_new`), without those
    /// of null id and those whose viewer type is `AT_UNKNOWN`.
    pub items: Vec<Item>,
    /// The parents of the items whose viewer type is `AT_UNKNOWN`
    /// (`cats_to_update`, UUID order).
    pub cats_to_update: BTreeSet<Uuid>,
    /// The items dropped for a null id, in file order (the C++ logs
    /// "Ignoring inventory with null item id: {getName()}").
    pub null_id_items: Vec<Item>,
    /// The lines the item loop logs, in file order.
    pub lines: Vec<DecodeLine>,
}

/// A line `loadFromFile`'s item loop logs, in file order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeLine {
    /// `LL_DEBUGS(LOG_INV)` "Ignoring inventory with null item id: {name}"
    /// for [`Decoded::null_id_items`]`[index]`. The C++ formats the line
    /// with the item's `getName()`, which follows a link into the model
    /// (and can log "Warning: Accessing link to link") only when the debug
    /// line is enabled; so the caller does that formatting, not this crate.
    NullId { index: usize },
    /// The item filter's `getType()` followed a link to `target`, a link of
    /// the model: `getLinkedItem` looked `target` up (`getItem`) and logged
    /// `LL_WARNS(LOG_INV)` "Warning: Accessing link to link".
    LinkToLink { target: Uuid },
    /// `fromLLSD` reset the item's inventory type (`IT_NONE`, or one that
    /// does not match its asset type): `LL_DEBUGS()` "Resetting inventory
    /// type for {id}" (`llinventory/llinventory.cpp`), with the id read
    /// (null for an item the null-id line then drops).
    ResetInventoryType { id: Uuid },
}

impl Decoded {
    /// `loadFromFile`'s return value: `!is_cache_obsolete`.
    pub fn rv(&self) -> bool {
        !self.obsolete
    }
}

/// `loadFromFile` over the bytes of a file it opened.
///
/// The version is read as 4 bytes, big-endian; fewer than 4 bytes or another
/// version is obsolete. The rest is parsed as one binary LLSD value without
/// a size limit (bytes after it, such as the newline, are ignored); a parse
/// failure is obsolete, while a parse that finds nothing leaves an undefined
/// document and loads nothing. `categories` and `items` are read only when
/// they are arrays of a map document.
///
/// The item filter uses the viewer item's `getType()`, which follows a link
/// into the model (`lookup`): an `AT_LINK` to a non-link item has that
/// item's type, an `AT_LINK_FOLDER` to a folder the folder's type.
/// `in_open_sim` is passed to `fromLLSD` (its OpenSim permissions branch).
pub fn decode(bytes: &[u8], in_open_sim: bool, lookup: &dyn ModelLookup) -> Decoded {
    let mut out = Decoded {
        obsolete: true,
        ..Decoded::default()
    };
    if bytes.len() < 4 {
        return out;
    }
    let version = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i32;
    if version != CACHE_VERSION {
        return out;
    }
    let body = &bytes[4..];
    let mut src = SliceSource::new(body);
    let mut r = Reader::new(&mut src);
    let mut builder = ValueBuilder::new(vkstorm_llsd::key_policy(Format::Binary));
    let n = vkstorm_llsd::parse_into(
        Format::Binary,
        &mut r,
        &mut builder,
        Limits::UNLIMITED,
        XmlMode::Parse,
    );
    if n == vkstorm_llsd::PARSE_FAILURE {
        return out;
    }
    out.obsolete = false;
    let inventory = if n > 0 {
        builder.finish().unwrap_or(Value::Undefined)
    } else {
        Value::Undefined
    };
    let Value::Map(map) = &inventory else {
        return out;
    };
    if let Some(Value::Array(cats)) = map.get(b"categories".as_slice()) {
        for sd in cats {
            let mut cat = ViewerCategory::default();
            if cat.import_llsd_map(sd) {
                out.categories.push(cat);
            }
        }
    }
    if let Some(Value::Array(items)) = map.get(b"items".as_slice()) {
        for sd in items {
            let mut item = Item::default();
            let Some(reset) = item.from_llsd_logged(sd, true, in_open_sim) else {
                continue;
            };
            if reset {
                out.lines
                    .push(DecodeLine::ResetInventoryType { id: item.object.id });
            }
            if item.object.id == NULL_ID {
                out.lines.push(DecodeLine::NullId {
                    index: out.null_id_items.len(),
                });
                out.null_id_items.push(item);
                continue;
            }
            let (viewer_type, link_to_link) = followed_type(&item, lookup);
            if link_to_link {
                out.lines
                    .push(DecodeLine::LinkToLink { target: item.asset });
            }
            if viewer_type == AssetType::UNKNOWN {
                out.cats_to_update.insert(item.object.parent);
            } else {
                out.items.push(item);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EmptyModel;

    #[test]
    fn round_trip_and_header_rules() {
        let mut cat = ViewerCategory {
            version: 3,
            ..ViewerCategory::default()
        };
        cat.category.object.id = [1; 16];
        let unknown = ViewerCategory::default();
        let mut item = Item::default();
        item.object.id = [2; 16];
        item.object.parent = [1; 16];
        item.object.asset_type = AssetType::NOTECARD;
        let bytes = encode([&cat, &unknown], [&item]);
        assert_eq!(&bytes[..4], &[0, 0, 0, 5]);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let d = decode(&bytes, false, &EmptyModel);
        assert!(d.rv());
        assert_eq!(
            d.categories.len(),
            1,
            "a folder of unknown version is not saved"
        );
        // fromLLSD normalises the item (inventory type, masks); from then on
        // the bytes round-trip.
        // The first read reset the item's inventory type (it had none), so
        // it logged that; the second has nothing to log.
        assert_eq!(
            d.lines,
            vec![DecodeLine::ResetInventoryType { id: [2; 16] }]
        );
        let again = encode(&d.categories, &d.items);
        let d2 = decode(&again, false, &EmptyModel);
        assert!(d2.lines.is_empty());
        assert_eq!(
            d2,
            Decoded {
                lines: Vec::new(),
                ..d.clone()
            }
        );
        assert_eq!(encode(&d2.categories, &d2.items), again);
        // Another version, a short header, a cut body.
        let mut v4 = bytes.clone();
        v4[3] = 4;
        assert!(decode(&v4, false, &EmptyModel).obsolete);
        assert!(decode(&bytes[..3], false, &EmptyModel).obsolete);
        assert!(decode(&bytes[..bytes.len() / 2], false, &EmptyModel).obsolete);
        // A header alone: nothing parsed, not obsolete, nothing loaded.
        let d = decode(&bytes[..4], false, &EmptyModel);
        assert!(!d.obsolete && d.categories.is_empty() && d.items.is_empty());
    }

    #[test]
    fn names() {
        let id = [0xab; 16];
        assert_eq!(
            cache_file_name(&id, None),
            b"abababab-abab-abab-abab-abababababab.inv.llsd".to_vec()
        );
        assert_eq!(
            cache_file_name(&id, Some(b"aditi")),
            b"abababab-abab-abab-abab-abababababab.aditi.inv.llsd".to_vec()
        );
        assert_eq!(
            delete_marker_name(&id),
            b"abababab-abab-abab-abab-abababababab_DELETE_INV_GZ".to_vec()
        );
    }
}
