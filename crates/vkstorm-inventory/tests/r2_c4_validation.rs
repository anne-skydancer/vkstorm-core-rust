//! R2 C4 validation: regenerates `fixtures/cache/c4_validation.txt` and
//! both `c4_validation_qa.<library>.txt` files from the crate's
//! [`vkstorm_inventory::validate`], over the model states of
//! `llinventorymodel_parity_c4.h` rebuilt by the test (`r2common::snapshot`:
//! the standard world, `populate`, and each case's change).
//!
//! Per case: every validation line (`asLLSD` as binary-LLSD hex and
//! notation, the counts, each warning, `mLog`) is the crate's; the model
//! summary lines (header, child vectors, backlinks, set locks) are the
//! test's rebuilt state, compared to show it is the state the C++
//! validated. The lock maps' entry counts and the pending batch are C++
//! container internals `validate()` does not read: counted as left.
//!
//! The QA cases (`QAModeFakeSystemFolderIssues`) draw from one engine per
//! process, in case order, two draws per checked folder type per call; each
//! case calls `validate()` twice (in `populate`'s login and in the case).
//! Both libraries' records are replayed: [`QaLibrary::Msvc`] (`mt19937`)
//! and [`QaLibrary::Libstdcxx`] (`minstd_rand0`).

mod common;
mod r2common;

use r2common::snapshot::*;
use r2common::*;
use vkstorm_inventory::validate::{QaLibrary, QaRandom};

fn changed(id: &str) -> (World, Box<dyn Fn(&mut TestModel)>) {
    let w = standard_world();
    let none: Box<dyn Fn(&mut TestModel)> = Box::new(|_| {});
    let f9999 = pid(TAG_FOLDER, 9999);
    let change: Box<dyn Fn(&mut TestModel)> = match id {
        "clean" => none,
        "no_root_id" => Box::new(|m| m.snap.root = [0; 16]),
        "no_library_root_id" => Box::new(|m| m.snap.library_root = [0; 16]),
        "loop_two_folders" => Box::new(|m| m.cat_mut(&f(42)).parent = f(43)),
        "loop_self" => Box::new(|m| m.cat_mut(&f(45)).parent = f(45)),
        "folder_parent_missing" => Box::new(move |m| m.cat_mut(&f(43)).parent = f9999),
        "folder_not_in_parent_vector" => Box::new(|m| m.remove_from_vectors(&f(42), &f(43))),
        "folder_null_parent_not_root" => Box::new(|m| m.cat_mut(&f(43)).parent = [0; 16]),
        "item_null_parent" => Box::new(|m| m.item_mut(&i(13)).parent = [0; 16]),
        "item_wrong_parent" => Box::new(|m| m.item_mut(&i(13)).parent = f(SF_OBJECTS)),
        "item_not_in_parent_vector" => Box::new(|m| m.remove_from_vectors(&f(SF_NOTECARDS), &i(13))),
        "item_in_vector_not_in_map" => Box::new(|m| {
            m.snap.item_map.remove(&i(13));
        }),
        "item_parent_missing" => Box::new(move |m| m.item_mut(&i(13)).parent = f9999),
        "item_under_unrecognised_top" => Box::new(|m| {
            m.add_category(f(800), f(SF_ROOT), -1);
            m.update_item_new(&WItem { id: i(800), parent: f(800), atype: vkstorm_inventory::types::AssetType::NOTECARD, asset: a(800) });
            m.cat_mut(&f(800)).parent = [0; 16];
        }),
        "duplicate_trash_fatal" => return (w.with_folder(pid(TAG_FOLDER, 0x900), f(SF_ROOT), 14, 1), none),
        "duplicate_cof_fatal" => return (w.with_folder(pid(TAG_FOLDER, 0x901), f(SF_ROOT), 46, 1), none),
        "duplicate_textures_automatic" => return (w.with_folder(pid(TAG_FOLDER, 0x902), f(SF_ROOT), 0, 1), none),
        "duplicate_settings_warning" => return (w.with_folder(pid(TAG_FOLDER, 0x903), f(SF_ROOT), 56, 1), none),
        "duplicate_material_warning" => return (w.with_folder(pid(TAG_FOLDER, 0x904), f(SF_ROOT), 57, 1), none),
        "system_folder_elsewhere" => return (w.with_folder(pid(TAG_FOLDER, 0x905), f(SF_OBJECTS), 14, 1), none),
        "suitcase_ignored" => return (w.with_folder(pid(TAG_FOLDER, 0x906), f(SF_ROOT), 100, 1), none),
        "missing_textures" => return (w.without_folder(f(SF_TEXTURES)), none),
        "missing_trash" => return (w.without_folder(f(SF_TRASH)), none),
        "missing_cof" => return (w.without_folder(f(SF_COF)), none),
        "missing_inbox" => return (w.without_folder(f(SF_INBOX)), none),
        "missing_settings" => return (w.without_folder(f(SF_SETTINGS)), none),
        "agent_count_mismatch" => Box::new(|m| m.cat_mut(&f(SF_OBJECTS)).descendent_count = 99),
        "library_count_mismatch_exempt" => Box::new(|m| m.cat_mut(&lf(3)).descendent_count = 99),
        "unknown_counts_and_versions" => Box::new(|m| {
            m.cat_mut(&f(42)).descendent_count = -1;
            m.cat_mut(&f(43)).version = -1;
        }),
        "locks" => Box::new(|m| {
            m.snap.category_locks.insert(f(42));
            m.snap.item_locks.insert(f(42));
        }),
        "category_tree_extra_entry" => Box::new(|m| {
            m.snap.category_tree.insert(pid(TAG_FOLDER, 0x999), Vec::new());
        }),
        "category_missing_child_arrays" => Box::new(|m| {
            m.snap.item_tree.remove(&f(45));
        }),
        "category_key_mismatch" => Box::new(|m| {
            let k = m.cat_index(&f(45));
            m.snap.category_map.insert(pid(TAG_FOLDER, 0x998), Some(k));
        }),
        "item_key_mismatch" => Box::new(|m| {
            let k = m.snap.item_map[&i(18)];
            m.snap.item_map.insert(pid(TAG_ITEM, 0x998), k);
            m.snap.last_item = Some(k);
        }),
        "link_missing_backlink" => Box::new(|m| m.snap.backlinks.clear()),
        "links_broken_and_to_links" => Box::new(|m| {
            m.update_item_new(&link(i(810), f(SF_COF), pid(TAG_ITEM, 0x9999), false));
            m.update_item_new(&link(i(811), f(SF_COF), pid(TAG_FOLDER, 0x9999), true));
            m.update_item_new(&link(i(812), f(SF_COF), i(33), false));
            m.snap.backlinks.push((i(33), i(10)));
            m.snap.backlinks.push((i(30), i(812)));
        }),
        "two_roots_named_my_inventory" => return (w.with_folder(pid(TAG_FOLDER, 0x907), f(SF_ROOT), 8, 1), none),
        "three_roots_named_my_inventory" => {
            return (
                w.with_folder(pid(TAG_FOLDER, 0x908), f(SF_ROOT), 8, 1).with_folder(pid(TAG_FOLDER, 0x909), f(SF_ROOT), 8, 1),
                none,
            );
        }
        "three_roots_different_parents" => {
            return (
                w.with_folder(pid(TAG_FOLDER, 0x90a), f(SF_ROOT), 8, 1).with_folder(pid(TAG_FOLDER, 0x90b), f(SF_OBJECTS), 8, 1),
                none,
            );
        }
        "item_vector_20005_wrong_parent_last" => {
            let mut w = w.with_folder(f(850), f(SF_ROOT), -1, 1);
            for n in 0..20005u64 {
                w.items.push(WItem {
                    id: pid(0xa800_0000, n),
                    parent: f(850),
                    atype: vkstorm_inventory::types::AssetType::NOTECARD,
                    asset: a(9000 + n),
                });
            }
            return (
                w,
                Box::new(|m| {
                    let last = *m.snap.item_tree[&f(850)].last().unwrap();
                    m.snap.items[last.unwrap()].parent = f(SF_OBJECTS);
                }),
            );
        }
        "qa_fake_1" | "qa_fake_2" | "qa_fake_3" | "qa_off_again" => none,
        other => panic!("unknown C4 case {other}"),
    };
    (w, change)
}

/// Replays one C4 file; `qa` is the engine of the QA file.
fn replay(file: &str, mut qa: Option<QaRandom>) -> Tally {
    let mut tally = Tally::default();
    for case in read_cases(file) {
        let (w, change) = changed(&case.id);
        let qa_on = qa.is_some() && case.id.starts_with("qa_fake");
        let mut m = populate(&w, if qa_on { qa.as_mut() } else { None });
        change(&mut m);
        let v = m.validate(if qa_on { qa.as_mut() } else { None });
        let mut out = dump_validation(&v.info);
        let n_validation = out.len();
        out.extend(m.summary().iter().map(|l| long_line(l)));
        let expected: Vec<String> = case
            .lines
            .iter()
            .filter(|l| !l.starts_with("locks cat_entries=") && !l.starts_with("pending "))
            .cloned()
            .collect();
        assert_case(file, &case.id, &expected, &out);
        tally.replayed("validation", n_validation);
        tally.replayed("model summary (rebuilt input state)", out.len() - n_validation);
        tally.left("lock entry counts, pending batch", case.lines.len() - expected.len());
    }
    tally.print(file);
    tally
}

#[test]
fn c4_validation() {
    let t = replay("cache/c4_validation.txt", None);
    assert_eq!(t.kinds["validation"], (205, 0));
    assert_eq!((t.replayed, t.left), (1595, 80));
}

#[test]
fn c4_validation_qa_msvc() {
    let t = replay("cache/c4_validation_qa.msvc.txt", Some(QaRandom::new(QaLibrary::Msvc)));
    assert_eq!((t.replayed, t.left), (92, 8));
}

#[test]
fn c4_validation_qa_libstdcxx() {
    let t = replay("cache/c4_validation_qa.libstdcxx.txt", Some(QaRandom::new(QaLibrary::Libstdcxx)));
    assert_eq!((t.replayed, t.left), (92, 8));
}

/// The default engine is the platform's own, and replays the recording of
/// the platform's C++ standard library (both engines are also replayed
/// explicitly above, on every platform).
#[test]
fn c4_validation_qa_native() {
    let (file, library) = if cfg!(target_env = "msvc") {
        ("cache/c4_validation_qa.msvc.txt", QaLibrary::Msvc)
    } else {
        ("cache/c4_validation_qa.libstdcxx.txt", QaLibrary::Libstdcxx)
    };
    assert_eq!(QaRandom::default().library(), library);
    let t = replay(file, Some(QaRandom::default()));
    assert_eq!((t.replayed, t.left), (92, 8));
}
