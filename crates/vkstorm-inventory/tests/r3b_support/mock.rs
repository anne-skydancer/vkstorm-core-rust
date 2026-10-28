//! The replies of `tests/vksinventory_mock_server.py`, rebuilt as LLSD values
//! for the scenarios recorded without reply bodies (LOCKSTEP, PHASED, CONC,
//! large). Only what the viewer reads is produced: ids, parents, types,
//! owners, versions, the `_embedded` structure and the scripted statuses
//! (the README's "Mock fidelity"); names, assets and masks are left out, so
//! the bytes are not the recorded ones (whose length and digest stay inputs).
//! `r3b_seq.rs` checks the generator: every SEQ scenario also replays on
//! generated replies, each status equal to the recorded one.

use std::collections::BTreeMap;

use vkstorm_inventory::fetch::{NULL_ID, Uuid, uuid_string};
use vkstorm_inventory::llsd::coerce::{as_uuid, get};
use vkstorm_llsd::Value;

use super::{FT_CURRENT_OUTFIT, World, parse_uuid};

#[derive(Clone, Debug)]
struct MFolder {
    parent: Uuid,
    ftype: i32,
    version: i32,
    owner: Uuid,
}

#[derive(Clone, Debug)]
struct MItem {
    parent: Uuid,
    owner: Uuid,
    linked: Option<Uuid>,
}

/// The mock's state: its world, the scripted modes and the counters.
#[derive(Clone, Debug, Default)]
pub struct Mock {
    agent: Uuid,
    folders: BTreeMap<Uuid, MFolder>,
    items: BTreeMap<Uuid, MItem>,
    child_f: BTreeMap<Uuid, Vec<Uuid>>,
    child_i: BTreeMap<Uuid, Vec<Uuid>>,
    counts: BTreeMap<String, u32>,
    ais_requests: u32,
    subset_requests: u32,
    pub fail_first: u32,
    pub fail_status: i32,
    pub fail_subset: u32,
    pub legacy_error: u32,
}

fn m(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(pairs.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())
}

fn idmap(pairs: Vec<(Uuid, Value)>) -> Value {
    Value::Map(pairs.into_iter().map(|(k, v)| (uuid_string(&k).into_bytes(), v)).collect())
}

fn error_body(code: i32) -> Value {
    m(vec![("error_code", Value::Integer(code)), ("error_description", Value::String(b"scripted".to_vec()))])
}

impl Mock {
    /// `POST /control/world` with the scenario's world.
    pub fn new(w: &World) -> Self {
        let mut s = Mock { agent: w.agent, fail_status: 404, ..Default::default() };
        // a scripted folder's tag comes from its id (README "Mock fidelity")
        for f in w.folders.iter().filter(|f| !w.not_on_server.contains(&f.id)) {
            s.folders.insert(f.id, MFolder { parent: f.parent, ftype: f.ftype, version: f.version, owner: f.owner });
            s.child_f.entry(f.parent).or_default().push(f.id);
        }
        for i in &w.items {
            s.items.insert(i.id, MItem { parent: i.parent, owner: i.owner, linked: i.linked });
            s.child_i.entry(i.parent).or_default().push(i.id);
        }
        s
    }

    /// `POST /control/mode`: the counters restart.
    pub fn set_mode(&mut self, fail_first: u32, fail_status: i32, fail_subset: u32, legacy_error: u32) {
        self.fail_first = fail_first;
        self.fail_status = fail_status;
        self.fail_subset = fail_subset;
        self.legacy_error = legacy_error;
        self.counts.clear();
        self.ais_requests = 0;
        self.subset_requests = 0;
    }

    fn count(&mut self, key: String) -> u32 {
        let c = self.counts.entry(key).or_default();
        let n = *c;
        *c += 1;
        n
    }

    fn cof(&self) -> Option<Uuid> {
        self.folders.iter().find(|(_, f)| f.ftype == FT_CURRENT_OUTFIT).map(|(id, _)| *id)
    }

    fn item_value(&self, id: &Uuid) -> Value {
        let it = &self.items[id];
        let mut p = vec![
            ("item_id", Value::Uuid(*id)),
            ("parent_id", Value::Uuid(it.parent)),
            ("agent_id", Value::Uuid(it.owner)),
        ];
        match it.linked {
            Some(l) => {
                p.push(("linked_id", Value::Uuid(l)));
                if let Some(t) = self.items.get(&l) {
                    if t.linked.is_none() {
                        p.push(("_embedded", m(vec![("item", self.item_value(&l))])));
                    }
                } else if self.folders.contains_key(&l) {
                    p.push(("_embedded", m(vec![("category", self.cat_value(&l, -1, None, false))])));
                }
            }
            None => p.push(("permissions", m(vec![("owner_id", Value::Uuid(it.owner))]))),
        }
        m(p)
    }

    fn cat_value(&self, id: &Uuid, depth: i32, subset: Option<&[Uuid]>, links_only: bool) -> Value {
        let f = &self.folders[id];
        let mut p = vec![
            ("category_id", Value::Uuid(*id)),
            ("parent_id", Value::Uuid(f.parent)),
            ("type_default", Value::Integer(f.ftype)),
            ("agent_id", Value::Uuid(f.owner)),
            ("version", Value::Integer(f.version)),
        ];
        if depth >= 0 {
            let kids = self.child_i.get(id).cloned().unwrap_or_default();
            let is_link = |i: &Uuid| self.items[i].linked.is_some();
            let mut emb = Vec::new();
            if links_only {
                let links = kids.iter().filter(|i| is_link(i)).map(|i| (*i, self.item_value(i))).collect();
                emb.push(("links", idmap(links)));
            } else {
                let cats = self
                    .child_f
                    .get(id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| subset.is_none_or(|s| s.contains(c)))
                    .map(|c| (c, self.cat_value(&c, depth - 1, None, false)))
                    .collect();
                emb.push(("categories", idmap(cats)));
                if subset.is_none() {
                    let links = kids.iter().filter(|i| is_link(i)).map(|i| (*i, self.item_value(i))).collect();
                    let items = kids.iter().filter(|i| !is_link(i)).map(|i| (*i, self.item_value(i))).collect();
                    emb.push(("links", idmap(links)));
                    emb.push(("items", idmap(items)));
                }
            }
            p.push(("_embedded", m(emb)));
        }
        m(p)
    }

    /// An AIS request (`/cap/inv/...`, `/cap/lib/...`): status and reply.
    pub fn ais(&mut self, method: &str, target: &str) -> (i32, Option<Value>) {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let rest = &parts[2..];
        let kind = rest.first().copied().unwrap_or("");
        let mut oid = rest.get(1).copied().unwrap_or("").to_owned();
        let sub = rest.get(2).copied().unwrap_or("");
        if kind == "category" && oid == "current" {
            oid = self.cof().map(|c| uuid_string(&c)).unwrap_or_else(|| uuid_string(&NULL_ID));
        }
        let mut q: BTreeMap<&str, &str> = BTreeMap::new();
        for kv in query.split(['&', ',']) {
            if let Some((k, v)) = kv.split_once('=') {
                q.insert(k, v);
            }
        }
        // the children list is comma separated: take it whole
        let children: Option<Vec<Uuid>> = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("children="))
            .map(|c| c.split(',').map(parse_uuid).collect());
        let depth: Option<i32> = q.get("depth").map(|d| d.parse().unwrap_or(0));
        let has_children = children.is_some();
        self.ais_requests += 1;

        if self.fail_first > 0 && self.ais_requests <= self.fail_first {
            return (self.fail_status, Some(Value::map()));
        }
        if has_children && method == "GET" {
            self.subset_requests += 1;
            if self.subset_requests <= self.fail_subset {
                return (404, Some(error_body(404)));
            }
        }
        let p = &oid[..oid.len().min(2)];
        let is_children = method == "GET" && kind == "category" && sub == "children";
        let pre = |id: &Uuid| uuid_string(id)[..2].to_owned();
        if is_children && has_children && depth.unwrap_or(0) > 0 {
            if children.as_ref().unwrap().iter().any(|c| matches!(pre(c).as_str(), "f3" | "f4")) {
                return (403, Some(error_body(403)));
            }
        }
        if is_children && has_children && children.as_ref().unwrap().iter().any(|c| pre(c) == "4b") && self.count("4b-subset".into()) < 1 {
            return (404, Some(error_body(404)));
        }
        if is_children && p == "4c" {
            return (404, Some(error_body(404)));
        }
        if is_children && p == "f4" && !has_children {
            return (403, Some(error_body(403)));
        }
        if is_children && p == "f3" && depth.unwrap_or(0) > 0 && !has_children {
            return (403, Some(error_body(403)));
        }
        if p == "52" && method == "GET" && self.count(format!("52:{oid}")) < 2 {
            return (503, None);
        }
        if is_children && p == "5e" && depth.unwrap_or(0) > 0 && !has_children {
            return (503, None);
        }
        if p == "53" && method != "GET" && self.count(format!("53:{oid}")) < 1 {
            return (503, None);
        }
        if method != "GET" {
            // writes are replayed from recorded replies only
            return (400, Some(error_body(400)));
        }
        let id = parse_uuid(&oid);
        match kind {
            "orphans" => {
                let orphans = self.child_i.get(&NULL_ID).cloned().unwrap_or_default();
                let items = orphans.iter().map(|i| (*i, self.item_value(i))).collect();
                (200, Some(m(vec![("_embedded", m(vec![("items", idmap(items))]))])))
            }
            "item" => match self.items.contains_key(&id) {
                true => (200, Some(self.item_value(&id))),
                false => (404, Some(error_body(404))),
            },
            "category" => {
                if !self.folders.contains_key(&id) {
                    return (404, Some(error_body(404)));
                }
                match sub {
                    "children" => (200, Some(self.cat_value(&id, depth.unwrap_or(0), children.as_deref(), false))),
                    "links" => (200, Some(self.cat_value(&id, 0, None, true))),
                    "categories" => (200, Some(self.cat_value(&id, depth.unwrap_or(0), None, false))),
                    _ => (404, Some(error_body(404))),
                }
            }
            _ => (404, Some(error_body(404))),
        }
    }

    /// `FetchInventoryDescendents2` / `FetchLibDescendents2`.
    pub fn legacy_folders(&mut self, body: &Value) -> (i32, Option<Value>) {
        let folders = match get(body, "folders") {
            Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        let ids: Vec<Uuid> = folders.iter().map(|f| as_uuid(get(f, "folder_id"))).collect();
        if ids.iter().any(|i| uuid_string(i).starts_with("f4")) {
            return (403, None);
        }
        for i in &ids {
            if uuid_string(i).starts_with("e0") && self.count(format!("e0:{}", uuid_string(i))) < self.legacy_error {
                return (200, Some(m(vec![("error", m(vec![("identifier", Value::String(b"Development".to_vec()))]))])));
            }
        }
        let mut out = Vec::new();
        let mut bad = Vec::new();
        for i in &ids {
            if *i == NULL_ID {
                let kids = self.child_i.get(&NULL_ID).cloned().unwrap_or_default();
                out.push(m(vec![
                    ("agent_id", Value::Uuid(self.agent)),
                    ("owner_id", Value::Uuid(self.agent)),
                    ("folder_id", Value::Uuid(NULL_ID)),
                    ("version", Value::Integer(0)),
                    ("descendents", Value::Integer(kids.len() as i32)),
                    ("categories", Value::Array(vec![])),
                    ("items", Value::Array(kids.iter().map(|k| self.item_value(k)).collect())),
                ]));
                continue;
            }
            let Some(f) = self.folders.get(i).cloned() else {
                bad.push(m(vec![("folder_id", Value::Uuid(*i)), ("error", Value::String(b"Unknown".to_vec()))]));
                continue;
            };
            let cats = self.child_f.get(i).cloned().unwrap_or_default();
            let kids = self.child_i.get(i).cloned().unwrap_or_default();
            out.push(m(vec![
                ("agent_id", Value::Uuid(self.agent)),
                ("owner_id", Value::Uuid(f.owner)),
                ("folder_id", Value::Uuid(*i)),
                ("version", Value::Integer(f.version)),
                ("descendents", Value::Integer((cats.len() + kids.len()) as i32)),
                (
                    "categories",
                    Value::Array(
                        cats.iter()
                            .map(|c| {
                                let cf = &self.folders[c];
                                m(vec![
                                    ("category_id", Value::Uuid(*c)),
                                    ("parent_id", Value::Uuid(cf.parent)),
                                    ("type_default", Value::Integer(cf.ftype)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("items", Value::Array(kids.iter().map(|k| self.item_value(k)).collect())),
            ]));
        }
        let mut p = vec![("folders", Value::Array(out))];
        if !bad.is_empty() {
            p.push(("bad_folders", Value::Array(bad)));
        }
        (200, Some(m(p)))
    }

    /// `FetchInventory2` / `FetchLib2`.
    pub fn legacy_items(&mut self, body: &Value) -> (i32, Option<Value>) {
        let items = match get(body, "items") {
            Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        let out: Vec<Value> = items
            .iter()
            .map(|e| as_uuid(get(e, "item_id")))
            .filter(|i| self.items.contains_key(i))
            .map(|i| self.item_value(&i))
            .collect();
        (200, Some(m(vec![("agent_id", Value::Uuid(self.agent)), ("items", Value::Array(out))])))
    }
}
