//! A test-only executor for the login load plan
//! ([`vkstorm_inventory::login::LoadPlan`]) and the model dumps of the C2
//! recorder.
//!
//! [`HarnessModel`] holds what `loadSkeleton` touches of `LLInventoryModel`
//! before `buildParentChildMap`: the folder and item maps and the backlink
//! multimap. It applies a plan as the C++ executor would, through the C++
//! `addCategory` (an `FT_MESH` folder is not added; `localizeName` with the
//! recorder's stub translations) and `addItem` (an item whose viewer type is
//! `AT_NONE` or below is not added; a link adds its backlink once). The
//! background folder fetch is never active in C2, so `addItem` keeps no
//! broken-link lists.
//!
//! [`Delta`] is `dump_model_delta`. [`state_from_dumps`] rebuilds, from a
//! case's cumulative dump lines, the model `validate()` saw at the end of
//! `buildParentChildMap` (which stays C++), as a [`Snapshot`].

use std::collections::{BTreeMap, BTreeSet};

use vkstorm_inventory::category::ViewerCategory;
use vkstorm_inventory::item::Item;
use vkstorm_inventory::login::LoadPlan;
use vkstorm_inventory::model::{ModelLookup, viewer_item_type};
use vkstorm_inventory::perms::{NULL_ID, Uuid};
use vkstorm_inventory::types::{AssetType, FolderType};
use vkstorm_inventory::validate::{SnapCategory, SnapItem, Snapshot};

use super::{dump_category, dump_item, parse_uuid, uuid};

/// The recorder's stub `LLTrans` table: the five dictionary keys it
/// translates (every other key translates to itself).
pub fn localize(name: &[u8]) -> Vec<u8> {
    let t: &[(&str, &str)] = &[
        ("New Folder", "Nouveau dossier"),
        ("New Script", "Nouveau script"),
        ("New Note", "Nouvelle note"),
        ("Contents", "Contenu"),
        ("New Shirt", "Nouvelle chemise"),
    ];
    for (k, v) in t {
        if name == k.as_bytes() {
            return v.as_bytes().to_vec();
        }
    }
    name.to_vec()
}

#[derive(Clone, Debug)]
pub struct HCat {
    pub cat: ViewerCategory,
    pub count: i32,
}

#[derive(Clone, Debug, Default)]
pub struct HarnessModel {
    pub root: Uuid,
    pub library_root: Uuid,
    pub library_owner: Uuid,
    pub cats: BTreeMap<Uuid, HCat>,
    pub items: BTreeMap<Uuid, Item>,
    /// (target, link), insertion order.
    pub backlinks: Vec<(Uuid, Uuid)>,
}

impl ModelLookup for HarnessModel {
    fn item_type(&self, id: &Uuid) -> Option<AssetType> {
        self.items.get(id).map(|i| i.object.asset_type)
    }
    fn category_type(&self, id: &Uuid) -> Option<AssetType> {
        self.cats.get(id).map(|c| c.cat.category.object.asset_type)
    }
    fn category_version(&self, id: &Uuid) -> Option<i32> {
        self.cats.get(id).map(|c| c.cat.version)
    }
}

impl HarnessModel {
    /// The C++ executor of a [`LoadPlan`].
    pub fn apply(&mut self, plan: LoadPlan) {
        for f in plan.folders {
            if f.folder.category.preferred_type == FolderType::MESH {
                continue;
            }
            let mut cat = f.folder;
            cat.category.object.name = localize(&cat.category.object.name);
            self.cats.insert(
                cat.category.object.id,
                HCat {
                    cat,
                    count: f.descendent_count,
                },
            );
        }
        for item in plan.items {
            if viewer_item_type(&item, self).0 <= AssetType::NONE.0 {
                continue;
            }
            if item.is_link_type() {
                let pair = (item.linked_id(), item.object.id);
                if !self.backlinks.contains(&pair) {
                    self.backlinks.push(pair);
                }
            }
            self.items.insert(item.object.id, item);
        }
        for id in plan.invalidate {
            if let Some(c) = self.cats.get_mut(&id) {
                c.cat.version = -1;
            }
        }
    }

    /// `dump_model(out, true)` before `buildParentChildMap` (no child
    /// vectors, no locks, the constructor's pending mask).
    pub fn dump(&self) -> Vec<String> {
        let mut out = vec![format!(
            "model root={} library_root={} library_owner={} usable=0 cats={} items={} cat_trees=0 item_trees=0",
            uuid(&self.root),
            uuid(&self.library_root),
            uuid(&self.library_owner),
            self.cats.len(),
            self.items.len()
        )];
        out.extend(self.cats.values().map(|c| dump_category(&c.cat, c.count)));
        out.extend(self.items.values().map(dump_item));
        let mut backlinks = self.backlinks.clone();
        backlinks.sort_by(|x, y| x.0.cmp(&y.0));
        for (t, l) in backlinks {
            out.push(format!("backlink {} {}", uuid(&t), uuid(&l)));
        }
        out.push("locks cat_entries=0 item_entries=0".into());
        out.push("pending mask=0xffffffff changed=[] added=[] backlog_mask=0x0 backlog_changed=[] backlog_added=[]".into());
        out
    }
}

impl HarnessModel {
    /// The model `validate()` reads after `buildParentChildMap` of this
    /// model, for a model where nothing is lost: every folder in its
    /// parent's vector in folder-id order (the roots under null), every item
    /// in its parent's in item-id order, an item vector per folder.
    pub fn built_snapshot(&self) -> Snapshot {
        let mut s = Snapshot {
            root: self.root,
            library_root: self.library_root,
            ..Snapshot::default()
        };
        s.category_tree.insert(NULL_ID, Vec::new());
        for (k, (id, c)) in self.cats.iter().enumerate() {
            let o = &c.cat.category.object;
            s.categories.push(SnapCategory {
                id: o.id,
                parent: o.parent,
                preferred_type: c.cat.category.preferred_type,
                version: c.cat.version,
                descendent_count: c.count,
            });
            s.category_map.insert(*id, Some(k));
            s.category_tree.insert(*id, Vec::new());
            s.item_tree.insert(*id, Vec::new());
        }
        for k in 0..s.categories.len() {
            let c = &s.categories[k];
            assert!(
                c.parent != NULL_ID || c.preferred_type == FolderType::ROOT_INVENTORY,
                "lost folder"
            );
            let parent = c.parent;
            s.category_tree
                .get_mut(&parent)
                .expect("lost folder")
                .push(Some(k));
        }
        for (k, (id, it)) in self.items.iter().enumerate() {
            s.items.push(SnapItem {
                id: *id,
                parent: it.object.parent,
                asset_type: it.object.asset_type,
                asset: it.asset,
            });
            s.item_map.insert(*id, k);
            s.item_tree
                .get_mut(&it.object.parent)
                .expect("lost item")
                .push(Some(k));
        }
        s.backlinks = self.backlinks.clone();
        s
    }
}

/// The key of a keyed dump line (`dump_model_delta`), if it has one.
pub fn line_key(l: &str) -> Option<String> {
    if l.starts_with("cat ")
        || l.starts_with("item ")
        || l.starts_with("children ")
        || l.starts_with("broken ")
    {
        let a = l.find(' ').unwrap() + 1;
        let end = l[a..].find(' ').map(|p| p + a).unwrap_or(l.len());
        Some(l[..end].to_owned())
    } else if l.starts_with("backlink ") {
        Some(l.to_owned())
    } else {
        None
    }
}

/// `dump_model_delta`: the keyed lines new or changed since the last dump,
/// `gone <key>` for those that disappeared, every other line.
#[derive(Default)]
pub struct Delta {
    seen: BTreeMap<String, String>,
}

impl Delta {
    pub fn dump(&mut self, lines: Vec<String>) -> Vec<String> {
        let mut out = Vec::new();
        let mut now = BTreeMap::new();
        for l in lines {
            match line_key(&l) {
                None => out.push(l),
                Some(k) => {
                    if self.seen.get(&k) != Some(&l) {
                        out.push(l.clone());
                    }
                    now.insert(k, l);
                }
            }
        }
        for k in self.seen.keys() {
            if !now.contains_key(k) {
                out.push(format!("gone {k}"));
            }
        }
        self.seen = now;
        out
    }
}

fn field<'a>(l: &'a str, name: &str) -> &'a str {
    let p = l
        .rfind(&format!(" {name}="))
        .unwrap_or_else(|| panic!("no {name}= in {l}"))
        + name.len()
        + 2;
    let end = l[p..].find(' ').map(|e| e + p).unwrap_or(l.len());
    &l[p..end]
}

fn key_or_id(l: &str) -> Uuid {
    match l.rfind(" key=") {
        Some(p) => parse_uuid(&l[p + 5..]),
        None => parse_uuid(&l[l.find(' ').unwrap() + 1..][..36]),
    }
}

fn id_list(s: &str) -> Option<Vec<Option<Uuid>>> {
    if s == "none" {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    if inner.is_empty() {
        return Some(Vec::new());
    }
    Some(
        inner
            .split(',')
            .map(|e| {
                if e == "null" {
                    None
                } else {
                    Some(parse_uuid(e))
                }
            })
            .collect(),
    )
}

/// Rebuilds the model of the latest of `dumps` (each a `dump_model_delta`
/// output, in order) as `validate()` reads it. A parent whose child vectors
/// are both present and empty is not dumped: every folder without a
/// `children` line, and the null parent, then has empty vectors (as
/// `buildParentChildMap` makes them), checked against the header's counts.
pub fn state_from_dumps(dumps: &[&[String]]) -> Snapshot {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut header = String::new();
    let mut locks: Vec<String> = Vec::new();
    for dump in dumps {
        locks.clear();
        for l in dump.iter() {
            if let Some(k) = l.strip_prefix("gone ") {
                seen.remove(k);
            } else if let Some(k) = line_key(l) {
                seen.insert(k, l.clone());
            } else if l.starts_with("model ") {
                header = l.clone();
            } else if l.starts_with("lock ") {
                locks.push(l.clone());
            }
        }
    }
    let mut s = Snapshot {
        root: parse_uuid(field(&header, "root")),
        library_root: parse_uuid(field(&header, "library_root")),
        ..Snapshot::default()
    };
    let mut cat_index: BTreeMap<Uuid, usize> = BTreeMap::new();
    let mut item_index: BTreeMap<Uuid, usize> = BTreeMap::new();
    for l in seen.values() {
        if l.starts_with("cat ") {
            let id = parse_uuid(&l[4..40]);
            let k = s.categories.len();
            s.categories.push(SnapCategory {
                id,
                parent: parse_uuid(field(l, "parent")),
                preferred_type: FolderType(field(l, "pref").parse().unwrap()),
                version: field(l, "version").parse().unwrap(),
                descendent_count: field(l, "count").parse().unwrap(),
            });
            cat_index.insert(id, k);
            s.category_map.insert(key_or_id(l), Some(k));
        } else if l.starts_with("item ") {
            let id = parse_uuid(&l[5..41]);
            let k = s.items.len();
            s.items.push(SnapItem {
                id,
                parent: parse_uuid(field(l, "parent")),
                asset_type: AssetType(field(l, "type").parse().unwrap()),
                asset: parse_uuid(field(l, "asset")),
            });
            item_index.insert(id, k);
            s.item_map.insert(key_or_id(l), k);
        }
    }
    let mut mentioned: BTreeSet<Uuid> = BTreeSet::new();
    for l in seen.values().filter(|l| l.starts_with("children ")) {
        let parent = parse_uuid(&l[9..45]);
        mentioned.insert(parent);
        if let Some(v) = id_list(field(l, "cats")) {
            s.category_tree.insert(
                parent,
                v.into_iter().map(|e| e.map(|id| cat_index[&id])).collect(),
            );
        }
        if let Some(v) = id_list(field(l, "items")) {
            s.item_tree.insert(
                parent,
                v.into_iter().map(|e| e.map(|id| item_index[&id])).collect(),
            );
        }
    }
    let cat_trees: usize = field(&header, "cat_trees").parse().unwrap();
    let item_trees: usize = field(&header, "item_trees").parse().unwrap();
    if cat_trees > 0 {
        let keys: Vec<Uuid> = s
            .category_map
            .keys()
            .copied()
            .chain(std::iter::once(NULL_ID))
            .collect();
        for k in keys {
            if !mentioned.contains(&k) {
                s.category_tree.entry(k).or_default();
                if k != NULL_ID {
                    s.item_tree.entry(k).or_default();
                }
            }
        }
    }
    assert_eq!(
        (s.category_tree.len(), s.item_tree.len()),
        (cat_trees, item_trees),
        "rebuilt child vectors: {header}"
    );
    for l in seen.values().filter(|l| l.starts_with("backlink ")) {
        s.backlinks
            .push((parse_uuid(&l[9..45]), parse_uuid(&l[46..82])));
    }
    for l in &locks {
        if let Some(id) = l.strip_prefix("lock cat ") {
            s.category_locks.insert(parse_uuid(id));
        } else if let Some(id) = l.strip_prefix("lock item ") {
            s.item_locks.insert(parse_uuid(id));
        }
    }
    s
}

/// `files <label> [name size; ...]` over a directory's files.
pub fn files_line(label: &str, dir: &BTreeMap<String, Vec<u8>>) -> String {
    let mut names: Vec<String> = dir
        .iter()
        .map(|(n, b)| {
            let gz = n.len() > 3 && n.ends_with(".gz");
            format!(
                "{n} {}",
                if gz {
                    "gz".to_owned()
                } else {
                    b.len().to_string()
                }
            )
        })
        .collect();
    names.sort();
    format!("files {label} [{}]", names.join("; "))
}
