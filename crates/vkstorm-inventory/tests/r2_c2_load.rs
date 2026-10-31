//! R2 C2 login load and discard: replays `fixtures/cache/c2_load.txt` over
//! the 64 case inputs in `fixtures/cache/c2/<case>/` with the crate's login
//! load ([`vkstorm_inventory::login`]) and validation.
//!
//! The test plays the C++ glue of `loadSkeleton` around the crate: the
//! clear-cache marker purge (by [`login::purge_names`]), llcommon's
//! `gunzip_file` into the plain or temporary name (`r2common::gz`), the
//! file read, and the plan's executor (`r2common::fixture_model`: the
//! model's maps and backlinks, `addCategory`, `addItem`), over an in-memory
//! cache directory. Replayed per case:
//!
//! - every line up to `buildParentChildMap`: the note and the directory
//!   before, each `loadSkeleton` return value, the directory listings after
//!   the library and the agent load and of the temporary directory, and the
//!   model dumps (every folder and item field, backlinks) as the recorder's
//!   `dump_model_delta` writes them;
//! - the validation lines of `buildParentChildMap`, by the crate's
//!   `validate()` over the model as the fixture's dumps leave it at the end
//!   of `buildParentChildMap` (which stays C++);
//! - the digests of the plain files left in the cache directory, in the
//!   cases without a logout save.
//!
//! Left to the viewer test, and counted: `buildParentChildMap`'s messages
//! and model changes, the usable flag, `createCommonSystemCategories`, the
//! startup notification, the logout save, and the plain-file digests after
//! a logout save.

mod common;
mod r2common;

use std::collections::BTreeMap;

use r2common::fixture_model::{Delta, HarnessModel, files_line, state_from_dumps};
use r2common::gz::gunzip_file;
use r2common::*;
use vkstorm_inventory::cache;
use vkstorm_inventory::login::{self, CacheInput, Skeleton, SkeletonEntry};
use vkstorm_inventory::model::{ModelLookup, ModelObjects};
use vkstorm_inventory::perms::{NULL_ID, Uuid};
use vkstorm_inventory::validate::{self, ValidationInfo};
use vkstorm_llsd::{Format, Limits, Value};

use common::sha256_hex;

const AGENT: &str = "a9e00000-0000-4000-8000-000000000001";
/// The name `getTempFilename()` gives (random in the C++; listed as
/// `<uuid>.tmp`).
const TEMP_NAME: &str = "<uuid>.tmp";

struct CaseInput {
    note: String,
    second_instance: bool,
    grid: Option<Vec<u8>>,
    logout: bool,
    agent_root: Uuid,
    lib_root: Uuid,
    lib_owner: Uuid,
    agent_skel: Value,
    lib_skel: Option<Value>,
    files: BTreeMap<String, Vec<u8>>,
}

/// `LLDir::getScrubbedFileName` then `utf8str_tolower`.
fn scrubbed_grid(grid: &str) -> Vec<u8> {
    grid.bytes()
        .map(|b| {
            if b"\\/:*?\"<>|".contains(&b) {
                b'_'
            } else {
                b.to_ascii_lowercase()
            }
        })
        .collect()
}

fn read_notation(rel: &str) -> Value {
    let bytes = read_bytes(rel);
    vkstorm_llsd::parse(Format::Notation, &bytes, Limits::UNLIMITED)
        .1
        .expect("skeleton notation")
}

fn read_input(id: &str) -> CaseInput {
    let dir = format!("cache/c2/{id}");
    let text = String::from_utf8(read_bytes(&format!("{dir}/case.txt"))).unwrap();
    let mut kv = BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        kv.insert(k.to_owned(), v.to_owned());
    }
    let id_of = |k: &str| parse_uuid(&kv[k]);
    let production = kv["production_grid"] == "1";
    let lib_path = path(&format!("{dir}/skel_library.llsd"));
    let mut files = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(path(&format!("{dir}/files"))) {
        for e in entries {
            let e = e.unwrap();
            files.insert(
                e.file_name().into_string().unwrap(),
                std::fs::read(e.path()).unwrap(),
            );
        }
    }
    CaseInput {
        note: kv["note"].clone(),
        second_instance: kv["second_instance"] == "1",
        grid: if production {
            None
        } else {
            Some(scrubbed_grid(&kv["grid_id"]))
        },
        logout: kv["logout"] == "1",
        agent_root: id_of("agent_root"),
        lib_root: id_of("lib_root"),
        lib_owner: id_of("lib_owner"),
        agent_skel: read_notation(&format!("{dir}/skel_agent.llsd")),
        lib_skel: lib_path
            .exists()
            .then(|| read_notation(&format!("{dir}/skel_library.llsd"))),
        files,
    }
}

/// The directories and the model of a login.
struct Login<'a> {
    input: &'a CaseInput,
    cache_dir: BTreeMap<String, Vec<u8>>,
    temp_dir: BTreeMap<String, Vec<u8>>,
    model: HarnessModel,
}

impl Login<'_> {
    fn name(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    /// `loadSkeleton(skeleton, owner)` with the crate deciding.
    fn load_skeleton(&mut self, skeleton: &Value, owner: &Uuid) -> bool {
        self.load_entries(&SkeletonEntry::list_from_llsd(skeleton), owner)
    }

    fn load_entries(&mut self, entries: &[SkeletonEntry], owner: &Uuid) -> bool {
        let agent = parse_uuid(AGENT);
        let grid = self.input.grid.as_deref();
        let purge = login::purge_names(&agent, &self.model.library_owner, grid);
        if self
            .cache_dir
            .contains_key(&Self::name(purge.marker.clone()))
        {
            for n in purge.files {
                self.cache_dir.remove(&Self::name(n));
            }
            self.cache_dir.remove(&Self::name(purge.marker));
        }
        let skel = Skeleton::import(owner, entries);
        if !skel.wants_cache() {
            let plan = skel.load(&CacheInput::default(), false, &self.model);
            let rv = plan.rv;
            self.model.apply(plan);
            return rv;
        }
        let plain = Self::name(cache::cache_file_name(owner, grid));
        let gz = format!("{plain}.gz");
        let second = self.input.second_instance;
        let gz_present = self.cache_dir.contains_key(&gz);
        let mut gunzip_ok = false;
        if gz_present {
            if let Some(bytes) = gunzip_file(&self.cache_dir[&gz]) {
                gunzip_ok = true;
                if second {
                    self.temp_dir.insert(TEMP_NAME.into(), bytes);
                } else {
                    self.cache_dir.insert(plain.clone(), bytes);
                }
            }
        }
        let file = if gz_present && !gunzip_ok {
            None
        } else if second {
            self.temp_dir.get(TEMP_NAME).cloned()
        } else {
            self.cache_dir.get(&plain).cloned()
        };
        let input = CacheInput {
            gz_present,
            gunzip_ok,
            file: file.as_deref(),
            second_instance: second,
        };
        // The C ABI path: the ids the load reads, resolved before the call
        // into ModelObjects, give the same plan as the live model.
        let mut resolved = ModelObjects::default();
        for id in file
            .as_deref()
            .map(|b| login::lookup_ids(b, false))
            .unwrap_or_default()
        {
            if let Some(t) = self.model.item_type(&id) {
                resolved.items.insert(id, t);
            }
            if let (Some(t), Some(v)) = (
                self.model.category_type(&id),
                self.model.category_version(&id),
            ) {
                resolved.categories.insert(id, (t, v));
            }
        }
        let plan = skel.clone().load(&input, false, &self.model);
        assert_eq!(
            plan,
            skel.load(&input, false, &resolved),
            "plan over pre-resolved lookups"
        );
        let (rv, remove_plain, remove_gz) = (plan.rv, plan.remove_plain, plan.remove_gz);
        self.model.apply(plan);
        if remove_plain {
            if second {
                self.temp_dir.remove(TEMP_NAME);
            } else {
                self.cache_dir.remove(&plain);
            }
        }
        if remove_gz {
            self.cache_dir.remove(&gz);
        }
        rv
    }
}

/// The lines of a case up to `buildParentChildMap`.
fn load_lines(input: &CaseInput) -> (Vec<String>, Login<'_>) {
    let mut l = Login {
        input,
        cache_dir: input.files.clone(),
        temp_dir: BTreeMap::new(),
        model: HarnessModel::default(),
    };
    let mut out = vec![
        format!("note {}", input.note),
        files_line("before", &l.cache_dir),
    ];
    if input.agent_root != NULL_ID {
        l.model.root = input.agent_root;
    }
    if input.lib_root != NULL_ID {
        l.model.library_root = input.lib_root;
    }
    if input.lib_owner != NULL_ID {
        l.model.library_owner = input.lib_owner;
    }
    let mut delta = Delta::default();
    if let Some(skel) = &input.lib_skel {
        if l.model.library_owner != NULL_ID {
            let owner = l.model.library_owner;
            let rv = l.load_skeleton(skel, &owner);
            out.push(format!("step load_library rv={}", u8::from(rv)));
            out.push(files_line("after_library", &l.cache_dir));
            out.extend(delta.dump(l.model.dump()));
        }
    }
    let rv = l.load_skeleton(&input.agent_skel, &parse_uuid(AGENT));
    out.push(format!("step load_agent rv={}", u8::from(rv)));
    out.push(files_line("after_agent", &l.cache_dir));
    out.push(files_line("temp", &l.temp_dir));
    out.extend(delta.dump(l.model.dump()));
    (out, l)
}

#[test]
fn c2_load() {
    let file = &r2common::cache_lib("c2_load.txt");
    let ids: Vec<String> = String::from_utf8(read_bytes("cache/c2/cases.txt"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(ids.len(), 64);
    let cases = read_cases(file);
    assert_eq!(cases.iter().map(|c| c.id.clone()).collect::<Vec<_>>(), ids);
    let mut tally = Tally::default();
    for case in &cases {
        let input = read_input(&case.id);
        let (out, login) = load_lines(&input);
        let build = case
            .lines
            .iter()
            .position(|l| l == "step build_parent_child_map")
            .unwrap();
        assert_case(file, &case.id, &case.lines[..build], &out);
        tally.replayed("login load (loadSkeleton)", out.len());

        // validate() at the end of buildParentChildMap.
        let first_v = case
            .lines
            .iter()
            .position(|l| l.starts_with("validation "))
            .unwrap();
        let expected_v: Vec<String> = case.lines[first_v..]
            .iter()
            .take_while(|l| l.starts_with("validation "))
            .cloned()
            .collect();
        let state = state_from_dumps(&[&case.lines[..first_v]]);
        let info = if state.root != NULL_ID && state.category_tree.contains_key(&state.root) {
            let mut info =
                validate::validate(&r2common::key_ordered(&state), Default::default()).info;
            info.initialized = true;
            info
        } else {
            ValidationInfo::default()
        };
        assert_case(
            file,
            &format!("{} (validation)", case.id),
            &expected_v,
            &dump_validation(&info),
        );
        tally.replayed("buildParentChildMap validation", expected_v.len());
        tally.left(
            "buildParentChildMap messages, model changes, usable",
            first_v + expected_v.len() + 1 - build - expected_v.len(),
        );

        // After the login.
        let rest_start = first_v + expected_v.len() + 1;
        let content_start = case
            .lines
            .iter()
            .position(|l| l.starts_with("content "))
            .unwrap_or(case.lines.len());
        tally.left(
            "system folders, startup notify, logout save",
            content_start - rest_start,
        );
        let content = &case.lines[content_start..];
        if input.logout {
            tally.left("plain files after a logout save", content.len());
        } else {
            let produced: Vec<String> = login
                .cache_dir
                .iter()
                .filter(|(n, _)| !n.ends_with(".gz"))
                .map(|(n, b)| format!("content {n} sha256={}", sha256_hex(b)))
                .collect();
            assert_case(file, &format!("{} (content)", case.id), content, &produced);
            tally.replayed("plain files left", produced.len());
        }
    }
    tally.print(file);
    assert_eq!((tally.replayed, tally.left), (5033, 1907));
}

/// `library_real.txt`'s directory listings: the public Library cache loaded
/// as the library skeleton (one entry per cached folder, as the recorder
/// makes it), then the standard agent skeleton without a cache.
#[test]
fn library_real_listings() {
    let file = &r2common::cache_lib("library_real.txt");
    let case = &read_cases(file)[0];
    let name = "ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz";
    let gz = read_bytes(&format!("model/library/{name}"));
    let owner = parse_uuid(&name[..36]);
    let plain = r2common::gz::gunzip_strict(&gz);
    let decoded = cache::decode(&plain, false, &vkstorm_inventory::model::EmptyModel);
    let mut skel = Vec::new();
    for c in &decoded.categories {
        let o = &c.category.object;
        skel.push(SkeletonEntry {
            name: Some(o.name.clone()),
            folder_id: Some(o.id),
            parent_id: Some(o.parent),
            version: Some(c.version),
            type_default: Some(c.category.preferred_type.0),
        });
    }
    let input = CaseInput {
        note: String::new(),
        second_instance: false,
        grid: None,
        logout: false,
        agent_root: parse_uuid("f0000000-0000-4000-8000-000000000001"),
        lib_root: NULL_ID,
        lib_owner: owner,
        agent_skel: read_notation("cache/c2/valid/skel_agent.llsd"),
        lib_skel: None,
        files: [(name.to_owned(), gz)].into_iter().collect(),
    };
    let mut l = Login {
        input: &input,
        cache_dir: input.files.clone(),
        temp_dir: BTreeMap::new(),
        model: HarnessModel::default(),
    };
    l.model.library_owner = owner;
    assert!(l.load_entries(&skel, &owner));
    let after_library = files_line("after_library", &l.cache_dir);
    assert!(l.load_skeleton(&input.agent_skel, &parse_uuid(AGENT)));
    let after_agent = files_line("after_agent", &l.cache_dir);
    let temp = files_line("temp", &l.temp_dir);
    let expected: Vec<String> = case
        .lines
        .iter()
        .filter(|l| l.starts_with("files "))
        .cloned()
        .collect();
    assert_case(
        file,
        &case.id,
        &expected,
        &[after_library, after_agent, temp],
    );
}
