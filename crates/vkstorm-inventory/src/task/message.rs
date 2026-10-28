//! The task-inventory messages the viewer sends, as flat records
//! ([`Outgoing`]), and their packing through the `vkstorm-messages`
//! builder ([`pack`]).
//!
//! The records hold every value the C++ senders add (`fetchInventoryFromServer`,
//! `updateInventory`, `removeInventory`, `moveInventory`, `saveScript`);
//! the item of `UpdateTaskInventory` and `RezScript` is the
//! [`ItemBlock`] of `LLViewerInventoryItem::packMessage` (the transaction id
//! in the asset slot, the CRC of the item). All are sent reliably to the
//! region's host.

use std::fmt;

use vkstorm_messages::{Builder, BuilderError, Template, VarType};

use crate::message::{pack_item_block, ItemBlock, ItemBlockKeys};
use crate::perms::Uuid;

/// A message of the task-inventory code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outgoing {
    /// `RequestTaskInventory` (`fetchInventoryFromServer`, UDP path).
    RequestTaskInventory { agent: Uuid, session: Uuid, local_id: u32 },
    /// `UpdateTaskInventory` (`updateInventory`).
    UpdateTaskInventory { agent: Uuid, session: Uuid, local_id: u32, key: u8, item: ItemBlock },
    /// `RemoveTaskInventory` (`removeInventory`).
    RemoveTaskInventory { agent: Uuid, session: Uuid, local_id: u32, item_id: Uuid },
    /// `MoveTaskInventory` (`moveInventory`).
    MoveTaskInventory { agent: Uuid, session: Uuid, folder_id: Uuid, local_id: u32, item_id: Uuid },
    /// `RezScript` (`saveScript`); `template` is the `NewScriptInfo` block,
    /// present for a script without an asset and with a template.
    RezScript {
        agent: Uuid,
        session: Uuid,
        group: Uuid,
        local_id: u32,
        enabled: bool,
        item: ItemBlock,
        template: Option<Uuid>,
    },
}

impl Outgoing {
    /// The message name.
    pub fn name(&self) -> &'static str {
        match self {
            Outgoing::RequestTaskInventory { .. } => "RequestTaskInventory",
            Outgoing::UpdateTaskInventory { .. } => "UpdateTaskInventory",
            Outgoing::RemoveTaskInventory { .. } => "RemoveTaskInventory",
            Outgoing::MoveTaskInventory { .. } => "MoveTaskInventory",
            Outgoing::RezScript { .. } => "RezScript",
        }
    }
}

/// Why a message could not be packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackError {
    /// The template is not the message's.
    WrongTemplate { expected: &'static str, got: String },
    /// The template lacks a block or variable the sender adds.
    Missing { block: &'static str, variable: &'static str },
    /// The builder refused a block or a value.
    Builder(BuilderError),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::WrongTemplate { expected, got } => write!(f, "template {got} is not {expected}"),
            PackError::Missing { block, variable } => write!(f, "template has no {block}.{variable}"),
            PackError::Builder(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PackError {}

impl From<BuilderError> for PackError {
    fn from(e: BuilderError) -> Self {
        PackError::Builder(e)
    }
}

struct Packer<'a> {
    builder: &'a mut Builder,
    template: &'a Template,
}

impl Packer<'_> {
    fn block(&mut self, block: &'static str) -> Result<(), PackError> {
        let key = self.template.block_by_name(block).ok_or(PackError::Missing { block, variable: "" })?.key;
        self.builder.next_block(key)?;
        Ok(())
    }

    fn add(&mut self, block: &'static str, variable: &'static str, data: &[u8], ty: VarType) -> Result<(), PackError> {
        let key = self
            .template
            .block_by_name(block)
            .and_then(|b| b.variable_by_name(variable))
            .ok_or(PackError::Missing { block, variable })?
            .key;
        self.builder.add_data(key, data, ty)?;
        Ok(())
    }

    fn id(&mut self, block: &'static str, variable: &'static str, id: &Uuid) -> Result<(), PackError> {
        self.add(block, variable, id, VarType::LLUUID)
    }

    fn u32(&mut self, block: &'static str, variable: &'static str, v: u32) -> Result<(), PackError> {
        self.add(block, variable, &v.to_le_bytes(), VarType::U32)
    }

    fn item(&mut self, block: &'static str, item: &ItemBlock) -> Result<(), PackError> {
        let b = self.template.block_by_name(block).ok_or(PackError::Missing { block, variable: "" })?;
        let keys = ItemBlockKeys::from_block(b).ok_or(PackError::Missing { block, variable: "ItemID" })?;
        pack_item_block(self.builder, &keys, item)?;
        Ok(())
    }
}

/// Starts `template` in `builder` and adds the message's blocks and
/// variables in the C++ order (`newMessageFast`, `nextBlockFast`, `add*Fast`).
/// The caller builds and sends it.
pub fn pack(builder: &mut Builder, template: &std::sync::Arc<Template>, msg: &Outgoing) -> Result<(), PackError> {
    if template.name != msg.name() {
        return Err(PackError::WrongTemplate { expected: msg.name(), got: template.name.clone() });
    }
    builder.new_message(template.clone());
    let mut p = Packer { builder, template };
    match msg {
        Outgoing::RequestTaskInventory { agent, session, local_id } => {
            p.block("AgentData")?;
            p.id("AgentData", "AgentID", agent)?;
            p.id("AgentData", "SessionID", session)?;
            p.block("InventoryData")?;
            p.u32("InventoryData", "LocalID", *local_id)?;
        }
        Outgoing::UpdateTaskInventory { agent, session, local_id, key, item } => {
            p.block("AgentData")?;
            p.id("AgentData", "AgentID", agent)?;
            p.id("AgentData", "SessionID", session)?;
            p.block("UpdateData")?;
            p.u32("UpdateData", "LocalID", *local_id)?;
            p.add("UpdateData", "Key", &[*key], VarType::U8)?;
            p.block("InventoryData")?;
            p.item("InventoryData", item)?;
        }
        Outgoing::RemoveTaskInventory { agent, session, local_id, item_id } => {
            p.block("AgentData")?;
            p.id("AgentData", "AgentID", agent)?;
            p.id("AgentData", "SessionID", session)?;
            p.block("InventoryData")?;
            p.u32("InventoryData", "LocalID", *local_id)?;
            p.id("InventoryData", "ItemID", item_id)?;
        }
        Outgoing::MoveTaskInventory { agent, session, folder_id, local_id, item_id } => {
            p.block("AgentData")?;
            p.id("AgentData", "AgentID", agent)?;
            p.id("AgentData", "SessionID", session)?;
            p.id("AgentData", "FolderID", folder_id)?;
            p.block("InventoryData")?;
            p.u32("InventoryData", "LocalID", *local_id)?;
            p.id("InventoryData", "ItemID", item_id)?;
        }
        Outgoing::RezScript { agent, session, group, local_id, enabled, item, template } => {
            p.block("AgentData")?;
            p.id("AgentData", "AgentID", agent)?;
            p.id("AgentData", "SessionID", session)?;
            p.id("AgentData", "GroupID", group)?;
            p.block("UpdateBlock")?;
            p.u32("UpdateBlock", "ObjectLocalID", *local_id)?;
            p.add("UpdateBlock", "Enabled", &[u8::from(*enabled)], VarType::Bool)?;
            p.block("InventoryBlock")?;
            p.item("InventoryBlock", item)?;
            if let Some(t) = template {
                p.block("NewScriptInfo")?;
                p.id("NewScriptInfo", "TemplateID", t)?;
            }
        }
    }
    Ok(())
}
