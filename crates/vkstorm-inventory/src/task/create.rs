//! `LLViewerObject::createInventoryItem`: the `CreateTaskInventoryItem`
//! capability's POST body, and how its reply is judged.

use vkstorm_llsd::Value;

use crate::llsd::coerce::{as_boolean, get, insert};
use crate::perms::{Permissions, Uuid};
use crate::types::{AssetType, InventoryType};

/// The arguments of `createInventoryItem` (the callback stays with the
/// caller).
#[derive(Clone, Debug)]
pub struct CreateRequest<'a> {
    pub asset_type: AssetType,
    pub inventory_type: InventoryType,
    pub sub_type: u8,
    pub name: &'a [u8],
    pub description: &'a [u8],
    pub permissions: &'a Permissions,
    /// `params`; `None` for an undefined `LLSD`.
    pub params: Option<&'a Value>,
}

/// The POST body: `object_id`, `inventory_type`, `asset_type`, `sub_type`
/// (integers), `name`, `description`, `permissions` (`base`, `owner`,
/// `everyone`, `group`, `next_owner`, each mask as `S32`) and `params` when
/// defined.
pub fn create_body(object_id: &Uuid, req: &CreateRequest<'_>) -> Value {
    let mut body = Value::map();
    insert(&mut body, "object_id", Value::Uuid(*object_id));
    insert(&mut body, "inventory_type", Value::Integer(req.inventory_type.0));
    insert(&mut body, "asset_type", Value::Integer(req.asset_type.0));
    insert(&mut body, "sub_type", Value::Integer(i32::from(req.sub_type)));
    insert(&mut body, "name", Value::String(req.name.to_vec()));
    insert(&mut body, "description", Value::String(req.description.to_vec()));
    let p = req.permissions;
    let mut perm = Value::map();
    insert(&mut perm, "base", Value::Integer(p.base as i32));
    insert(&mut perm, "owner", Value::Integer(p.owner_mask as i32));
    insert(&mut perm, "everyone", Value::Integer(p.everyone as i32));
    insert(&mut perm, "group", Value::Integer(p.group_mask as i32));
    insert(&mut perm, "next_owner", Value::Integer(p.next as i32));
    insert(&mut body, "permissions", perm);
    if let Some(params) = req.params {
        if !matches!(params, Value::Undefined) {
            insert(&mut body, "params", params.clone());
        }
    }
    body
}

/// `createInventoryItemCoro`'s verdict: the HTTP status is a success and
/// the reply's `success` is true (`asBoolean`).
pub fn create_succeeded(http_ok: bool, result: &Value) -> bool {
    http_ok && as_boolean(get(result, "success"))
}
