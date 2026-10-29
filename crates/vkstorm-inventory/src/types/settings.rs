//! Settings types: `LLSettingsType` (`llinventorysettings.{h,cpp}`), the
//! subtype of a settings item, kept in the low byte of its flags.
//!
//! The C++ keeps the table in `LLSettingsDictionary`, a `std::map` keyed by
//! the type; [`ENTRIES`] is that map in its ascending order. Its labels and
//! default names go through the viewer's translation bridge when the
//! dictionary is built; [`SettingsType::default_name`] is the untranslated
//! name, which is what a bridge that translates nothing gives, and
//! [`SettingsType::default_name_translated`] applies a bridge.

use super::inventory::IconName;
use crate::flags::II_FLAGS_SUBTYPE_MASK;

/// A settings type: `LLSettingsType::type_e`, as the raw integer the viewer
/// stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SettingsType(pub i32);

impl SettingsType {
    pub const SKY: SettingsType = SettingsType(0);
    pub const WATER: SettingsType = SettingsType(1);
    pub const DAYCYCLE: SettingsType = SettingsType(2);
    pub const INVALID: SettingsType = SettingsType(255);
    pub const NONE: SettingsType = SettingsType(-1);
}

/// One `SettingsEntry` of `LLSettingsDictionary`.
#[derive(Clone, Copy, Debug)]
pub struct SettingsEntry {
    pub settings_type: SettingsType,
    /// The dictionary name, also the untranslated label (`mLabel`).
    pub name: &'static str,
    /// The untranslated name of a new setting (`mDefaultNewName`).
    pub default_new_name: &'static str,
    pub icon_name: IconName,
}

const fn e(
    v: i32,
    name: &'static str,
    default_new_name: &'static str,
    icon_name: IconName,
) -> SettingsEntry {
    SettingsEntry {
        settings_type: SettingsType(v),
        name,
        default_new_name,
        icon_name,
    }
}

/// `LLSettingsDictionary::initSingleton()`, in map (ascending value) order.
#[rustfmt::skip]
pub static ENTRIES: [SettingsEntry; 5] = [
    e(-1,  "none",    "New Settings", IconName::SETTINGS),
    e(0,   "sky",     "New Sky",      IconName::SETTINGS_SKY),
    e(1,   "water",   "New Water",    IconName::SETTINGS_WATER),
    e(2,   "day",     "New Day",      IconName::SETTINGS_DAY),
    e(255, "invalid", "New Settings", IconName::SETTINGS),
];

impl SettingsType {
    /// The dictionary entry of this value (`LLSettingsDictionary::lookup`).
    pub fn entry(self) -> Option<&'static SettingsEntry> {
        ENTRIES.iter().find(|e| e.settings_type == self)
    }

    /// The entry of this value, or `ST_INVALID`'s, as `getIconName` and
    /// `getDefaultName` fall back.
    fn entry_or_invalid(self) -> &'static SettingsEntry {
        const INVALID_ENTRY: &SettingsEntry = &ENTRIES[ENTRIES.len() - 1];
        self.entry().unwrap_or(INVALID_ENTRY)
    }

    /// `LLSettingsType::fromInventoryFlags`: the low byte of the item flags
    /// (`II_FLAGS_SUBTYPE_MASK`), so always `0..=255`.
    pub fn from_inventory_flags(flags: u32) -> SettingsType {
        SettingsType((flags & II_FLAGS_SUBTYPE_MASK) as i32)
    }

    /// `LLSettingsType::getIconName`: the entry's icon, or `ST_INVALID`'s
    /// (`ICONNAME_SETTINGS`) for a value without an entry.
    pub fn icon_name(self) -> IconName {
        self.entry_or_invalid().icon_name
    }

    /// `LLSettingsType::getDefaultName` with a translation bridge that
    /// translates nothing: the entry's default new name, or `ST_INVALID`'s
    /// (`New Settings`) for a value without an entry.
    pub fn default_name(self) -> &'static str {
        self.entry_or_invalid().default_new_name
    }

    /// `LLSettingsType::getDefaultName` with a translation bridge:
    /// `translate` receives the untranslated default name, and a non-empty
    /// result replaces it (`SettingsEntry`'s constructor).
    pub fn default_name_translated(self, translate: impl FnOnce(&str) -> String) -> String {
        let name = self.default_name();
        let translated = translate(name);
        if translated.is_empty() {
            name.to_owned()
        } else {
            translated
        }
    }
}
