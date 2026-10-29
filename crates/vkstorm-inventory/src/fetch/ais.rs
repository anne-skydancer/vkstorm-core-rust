//! The AIS commands the background fetch issues, as decisions: which
//! `AISAPI` command function it calls, with which arguments. Building the
//! request (URL, method, depth clamp, body) and reading the reply (status,
//! callback ids, the 403 alert) is the AIS command layer of sub-rung R3a
//! (`ais::command::Command::build`, `ais::result::process_result`); this
//! module does not do it a second time.
//!
//! [`AisFetch`] is the fetch variants of R3a's `ais::command::Command`, field
//! for field; [`AisFetch::to_command`] is the R3a command, which the executor
//! builds with `Command::build`. The one piece of the command layer the
//! scheduler needs inside its own loop is whether a command is refused at
//! once (no capability, an empty subset): the C++ then calls the completion
//! with null inside the call that issued it, and the scheduler's state at
//! that point depends on it (the re-entrancy of s17). [`AisFetch::refusal`]
//! asks R3a's `Command::cap()` and `Command::build` for that.
//!
//! Also here: `EnqueueAISCommand`'s postponed queue (2,048 pending
//! coroutines), which R3a's command layer leaves to its caller.

use std::collections::VecDeque;

use super::Uuid;
use crate::ais::command::{CapKind, Command};
use crate::perms::NULL_ID;

/// Why a command function sends nothing (R3a's `ais::command::Refusal`).
pub use crate::ais::command::Refusal;

/// `MAX_SIMULTANEOUS_COROUTINES`
pub const MAX_SIMULTANEOUS_COROUTINES: usize = 2048;

/// The values of `AISAPI::COMMAND_TYPE` the background fetch issues (R3a's
/// `ais::command::CommandType` has them all).
pub mod command_type {
    pub const FETCHITEM: i32 = 9;
    pub const FETCHCATEGORYCHILDREN: i32 = 10;
    pub const FETCHCATEGORYSUBSET: i32 = 12;
    pub const FETCHCOF: i32 = 13;
    pub const FETCHORPHANS: i32 = 14;
    pub const FETCHCATEGORYLINKS: i32 = 15;
}

/// An `AISAPI` fetch command the scheduler calls: the fetch variants of
/// R3a's `ais::command::Command`, with its field names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AisFetch {
    /// `FetchItem(item_id, library ? LIBRARY : INVENTORY, cb)`
    FetchItem { item_id: Uuid, library: bool },
    /// `FetchCategoryChildren(category_id, type, recursive, cb, depth)`
    FetchCategoryChildren {
        category_id: Uuid,
        library: bool,
        recursive: bool,
        depth: i32,
    },
    /// `FetchCategorySubset(category_id, children, type, recursive, cb, depth)`
    FetchCategorySubset {
        category_id: Uuid,
        children: Vec<Uuid>,
        library: bool,
        recursive: bool,
        depth: i32,
    },
    /// `FetchCOF(cb)`
    FetchCof,
    /// `FetchCategoryLinks(category_id, cb)`
    FetchCategoryLinks { category_id: Uuid },
    /// `FetchOrphans(cb)`
    FetchOrphans,
}

impl AisFetch {
    /// `AISAPI::COMMAND_TYPE` of the command.
    pub fn command_type(&self) -> i32 {
        use command_type::*;
        match self {
            AisFetch::FetchItem { .. } => FETCHITEM,
            AisFetch::FetchCategoryChildren { .. } => FETCHCATEGORYCHILDREN,
            AisFetch::FetchCategorySubset { .. } => FETCHCATEGORYSUBSET,
            AisFetch::FetchCof => FETCHCOF,
            AisFetch::FetchCategoryLinks { .. } => FETCHCATEGORYLINKS,
            AisFetch::FetchOrphans => FETCHORPHANS,
        }
    }

    /// The R3a command function call this decision stands for.
    pub fn to_command(&self) -> Command {
        match self.clone() {
            AisFetch::FetchItem { item_id, library } => Command::FetchItem { item_id, library },
            AisFetch::FetchCategoryChildren {
                category_id,
                library,
                recursive,
                depth,
            } => Command::FetchCategoryChildren {
                category_id,
                library,
                recursive,
                depth,
            },
            AisFetch::FetchCategorySubset {
                category_id,
                children,
                library,
                recursive,
                depth,
            } => Command::FetchCategorySubset {
                category_id,
                children,
                library,
                recursive,
                depth,
            },
            AisFetch::FetchCof => Command::FetchCof,
            AisFetch::FetchCategoryLinks { category_id } => {
                Command::FetchCategoryLinks { category_id }
            }
            AisFetch::FetchOrphans => Command::FetchOrphans,
        }
    }

    /// Whether the command looks up `LibraryAPIv3` (else `InventoryAPIv3`):
    /// R3a's `Command::cap()`.
    pub fn uses_library_cap(&self) -> bool {
        self.to_command().cap() == CapKind::Library
    }

    /// Whether the command function refuses at once, given which of the
    /// region's two AIS capabilities are present: R3a's `Command::build`
    /// with the capability present or empty (the capability first, then the
    /// empty subset). No fetch command uses a transaction id.
    pub fn refusal(&self, inventory_cap: bool, library_cap: bool) -> Option<Refusal> {
        let present = if self.uses_library_cap() {
            library_cap
        } else {
            inventory_cap
        };
        self.to_command()
            .build(if present { "cap" } else { "" }, &NULL_ID)
            .err()
    }
}

impl From<&AisFetch> for Command {
    fn from(f: &AisFetch) -> Command {
        f.to_command()
    }
}

/// `EnqueueAISCommand` and `onIdle`: commands go to the coprocedure pool
/// while it holds fewer than 2,048 pending; the rest wait here in order.
/// `pending_in_pool` is `LLCoprocedureManager::countPending("AIS")`.
#[derive(Clone, Debug)]
pub struct PostponedQueue<T> {
    queue: VecDeque<T>,
}

impl<T> Default for PostponedQueue<T> {
    fn default() -> Self {
        PostponedQueue {
            queue: VecDeque::new(),
        }
    }
}

impl<T> PostponedQueue<T> {
    pub fn new() -> Self {
        PostponedQueue {
            queue: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// `EnqueueAISCommand`: `Some(cmd)` to enqueue in the pool now, `None`
    /// when it was postponed. The first postponed command registers
    /// `onIdle` (the caller does when this returns `None` and [`len`] is 1).
    ///
    /// [`len`]: PostponedQueue::len
    pub fn enqueue(&mut self, cmd: T, pending_in_pool: usize) -> Option<T> {
        if pending_in_pool < MAX_SIMULTANEOUS_COROUTINES {
            Some(cmd)
        } else {
            self.queue.push_back(cmd);
            None
        }
    }

    /// `onIdle`: the postponed commands to enqueue now, in order. When the
    /// queue is empty afterwards the C++ removes the idle callback.
    pub fn on_idle(&mut self, mut pending_in_pool: usize) -> Vec<T> {
        let mut out = Vec::new();
        while pending_in_pool < MAX_SIMULTANEOUS_COROUTINES {
            match self.queue.pop_front() {
                Some(c) => {
                    out.push(c);
                    pending_in_pool += 1;
                }
                None => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_checks_the_capability_first() {
        let e = AisFetch::FetchCategorySubset {
            category_id: [1; 16],
            children: vec![],
            library: true,
            recursive: true,
            depth: 0,
        };
        assert_eq!(e.refusal(true, false), Some(Refusal::NoCapability));
        assert_eq!(e.refusal(false, true), Some(Refusal::EmptyRequest));
        assert_eq!(AisFetch::FetchCof.refusal(true, false), None);
        assert_eq!(
            AisFetch::FetchOrphans.refusal(false, true),
            Some(Refusal::NoCapability)
        );
    }

    #[test]
    fn postponed_queue_drains_in_order() {
        let mut q = PostponedQueue::new();
        assert_eq!(q.enqueue(1, 2047), Some(1));
        assert_eq!(q.enqueue(2, 2048), None);
        assert_eq!(q.enqueue(3, 2049), None);
        assert_eq!(q.on_idle(2048), Vec::<i32>::new());
        assert_eq!(q.on_idle(2047), vec![2]);
        assert_eq!(q.on_idle(0), vec![3]);
        assert!(q.is_empty());
    }
}
