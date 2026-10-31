//! The `RequestTaskInventory` capability reply (`loadTaskInvLLSD`) and the
//! name localisation of `LLViewerInventoryItem::unpackMessage(LLSD)`
//! (`LLLocalizedInventoryItemsDictionary`).
//!
//! A reply is a map; `contents` is an array of `LLInventoryItem::asLLSD`
//! maps. Each element that is a map with `item_id` becomes an
//! `LLViewerInventoryItem` through `fromLLSD` (as a new item), its
//! `running` and `faulted` keys (`asBoolean`), and the localisation of its
//! name; every other element is skipped. A `contents` that is not an array
//! has no elements (`llsd::inArray`).

use std::collections::HashMap;

use vkstorm_llsd::Value;

use super::{Entry, ViewerItem};
use crate::llsd::coerce::{as_boolean, get, has};

/// The keys of `LLLocalizedInventoryItemsDictionary` and the `LLTrans`
/// string each is localised with, in the C++ order. Two keys differ from
/// their string: `"Male  - Get lost"` and `"Female  - Get lost"` (two
/// spaces; EXT-8319).
pub const LOCALIZED_NAME_KEYS: &[(&str, &str)] = &[
    ("New Shape", "New Shape"),
    ("New Skin", "New Skin"),
    ("New Hair", "New Hair"),
    ("New Eyes", "New Eyes"),
    ("New Shirt", "New Shirt"),
    ("New Pants", "New Pants"),
    ("New Shoes", "New Shoes"),
    ("New Socks", "New Socks"),
    ("New Jacket", "New Jacket"),
    ("New Gloves", "New Gloves"),
    ("New Undershirt", "New Undershirt"),
    ("New Underpants", "New Underpants"),
    ("New Skirt", "New Skirt"),
    ("New Alpha", "New Alpha"),
    ("New Tattoo", "New Tattoo"),
    ("New Universal", "New Universal"),
    ("New Physics", "New Physics"),
    ("Invalid Wearable", "Invalid Wearable"),
    ("New Gesture", "New Gesture"),
    ("New Material", "New Material"),
    ("New Script", "New Script"),
    ("New Folder", "New Folder"),
    ("New Note", "New Note"),
    ("Contents", "Contents"),
    ("Gesture", "Gesture"),
    ("Male Gestures", "Male Gestures"),
    ("Female Gestures", "Female Gestures"),
    ("Other Gestures", "Other Gestures"),
    ("Speech Gestures", "Speech Gestures"),
    ("Common Gestures", "Common Gestures"),
    ("Male - Excuse me", "Male - Excuse me"),
    ("Male  - Get lost", "Male - Get lost"),
    ("Male - Blow kiss", "Male - Blow kiss"),
    ("Male - Boo", "Male - Boo"),
    ("Male - Bored", "Male - Bored"),
    ("Male - Hey", "Male - Hey"),
    ("Male - Laugh", "Male - Laugh"),
    ("Male - Repulsed", "Male - Repulsed"),
    ("Male - Shrug", "Male - Shrug"),
    ("Male - Stick tougue out", "Male - Stick tougue out"),
    ("Male - Wow", "Male - Wow"),
    ("Female - Chuckle", "Female - Chuckle"),
    ("Female - Cry", "Female - Cry"),
    ("Female - Embarrassed", "Female - Embarrassed"),
    ("Female - Excuse me", "Female - Excuse me"),
    ("Female  - Get lost", "Female - Get lost"),
    ("Female - Blow kiss", "Female - Blow kiss"),
    ("Female - Boo", "Female - Boo"),
    ("Female - Bored", "Female - Bored"),
    ("Female - Hey", "Female - Hey"),
    ("Female - Hey baby", "Female - Hey baby"),
    ("Female - Laugh", "Female - Laugh"),
    ("Female - Looking good", "Female - Looking good"),
    ("Female - Over here", "Female - Over here"),
    ("Female - Please", "Female - Please"),
    ("Female - Repulsed", "Female - Repulsed"),
    ("Female - Shrug", "Female - Shrug"),
    ("Female - Stick tougue out", "Female - Stick tougue out"),
    ("Female - Wow", "Female - Wow"),
    ("/bow", "/bow"),
    ("/clap", "/clap"),
    ("/count", "/count"),
    ("/extinguish", "/extinguish"),
    ("/kmb", "/kmb"),
    ("/muscle", "/muscle"),
    ("/no", "/no"),
    ("/no!", "/no!"),
    ("/paper", "/paper"),
    ("/pointme", "/pointme"),
    ("/pointyou", "/pointyou"),
    ("/rock", "/rock"),
    ("/scissor", "/scissor"),
    ("/smoke", "/smoke"),
    ("/stretch", "/stretch"),
    ("/whistle", "/whistle"),
    ("/yes", "/yes"),
    ("/yes!", "/yes!"),
    ("afk", "afk"),
    ("dance1", "dance1"),
    ("dance2", "dance2"),
    ("dance3", "dance3"),
    ("dance4", "dance4"),
    ("dance5", "dance5"),
    ("dance6", "dance6"),
    ("dance7", "dance7"),
    ("dance8", "dance8"),
];

/// `LLLocalizedInventoryItemsDictionary`: exact names and their
/// localisation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalizedNames {
    names: HashMap<Vec<u8>, Vec<u8>>,
}

impl LocalizedNames {
    /// The dictionary's constructor, with `translate` as `LLTrans::getString`.
    pub fn new(mut translate: impl FnMut(&str) -> Vec<u8>) -> LocalizedNames {
        let names = LOCALIZED_NAME_KEYS
            .iter()
            .map(|(key, desc)| (key.as_bytes().to_vec(), translate(desc)))
            .collect();
        LocalizedNames { names }
    }

    /// An empty dictionary: no name is localised.
    pub fn none() -> LocalizedNames {
        LocalizedNames::default()
    }

    /// `localizeInventoryObjectName`: replaces a name found in the
    /// dictionary; returns whether it was found.
    pub fn localize(&self, name: &mut Vec<u8>) -> bool {
        match self.names.get(name.as_slice()) {
            Some(v) => {
                name.clone_from(v);
                true
            }
            None => false,
        }
    }
}

/// `LLViewerInventoryItem::unpackMessage(const LLSD&)`: `fromLLSD` (as a new
/// item), `running`, `faulted`, the name localised, complete.
pub fn unpack_viewer_item(sd: &Value, in_open_sim: bool, names: &LocalizedNames) -> ViewerItem {
    let mut item = ViewerItem::default();
    item.item.from_llsd(sd, true, in_open_sim);
    item.running = as_boolean(get(sd, "running"));
    item.faulted = as_boolean(get(sd, "faulted"));
    names.localize(&mut item.item.object.name);
    item.complete = true;
    item
}

/// The entries `loadTaskInvLLSD` reads from a reply with `contents`, in
/// reply order (each is pushed to the front of the list after the
/// synthesised `Contents` category); `None` when the reply has no
/// `contents` key.
pub fn read_reply_contents(
    reply: &Value,
    in_open_sim: bool,
    names: &LocalizedNames,
) -> Option<Vec<Entry>> {
    if !has(reply, "contents") {
        return None;
    }
    let mut out = Vec::new();
    if let Value::Array(elements) = get(reply, "contents") {
        for e in elements {
            if has(e, "item_id") {
                out.push(Entry::Item(unpack_viewer_item(e, in_open_sim, names)));
            }
            // else LL_WARNS_ONCE "Unknown inventory entry".
        }
    }
    Some(out)
}
