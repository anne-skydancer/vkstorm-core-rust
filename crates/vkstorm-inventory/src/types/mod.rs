//! The type tables, as raw integer newtypes with their names and lookups.

pub mod asset;
pub mod folder;
pub mod inventory;
pub mod sale;
pub mod settings;

pub use asset::AssetType;
pub use folder::FolderType;
pub use inventory::{inventory_and_asset_types_match, IconName, InventoryType};
pub use sale::SaleType;
pub use settings::SettingsType;
