//! `LLCategory` (`llcategory.{h,cpp}`): the object-for-sale categories and
//! their packed form.
//!
//! A category is four levels of one byte each, packed big-endian into a U32
//! (the message's `Category` field). Only the top level has names.

/// `CATEGORY_INDEX`: the index into [`CATEGORY_NAME`] of the first child of
/// each name, with one entry past the end for the child counts.
const CATEGORY_INDEX: [i32; 8] = [
    1, // ROOT
    6, // object
    7, // clothing
    7, // texture
    7, // sound
    7, // landmark
    7, // object|component
    7, // off the end (required for child count calculations)
];

/// `CATEGORY_NAME`: the heap of names, ending with `NULL`.
const CATEGORY_NAME: [Option<&str>; 8] = [
    Some("(none)"),
    Some("Object"), // (none)
    Some("Clothing"),
    Some("Texture"),
    Some("Sound"),
    Some("Landmark"),
    Some("Component"), // object
    None,
];

/// `LLCategory::CATEGORY_DEPTH`.
pub const CATEGORY_DEPTH: usize = 4;

/// `LLCategory`: four category bytes, the top level first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Category {
    data: [u8; CATEGORY_DEPTH],
}

impl Category {
    /// `LLCategory::none`, also what `LLCategory()` constructs.
    pub const NONE: Category = Category { data: [0; CATEGORY_DEPTH] };

    /// `LLCategory(U32)`: the bytes of `value`, most significant first.
    pub fn new(value: u32) -> Category {
        Category { data: value.to_be_bytes() }
    }

    /// `LLCategory::init`.
    pub fn init(&mut self, value: u32) {
        *self = Category::new(value);
    }

    /// `LLCategory::getU32`: the packed form.
    pub fn as_u32(self) -> u32 {
        u32::from_be_bytes(self.data)
    }

    /// `LLCategory::getSubCategoryCount`: the number of children of the top
    /// level category, 0 for an unknown top level (bug 4).
    pub fn sub_category_count(self) -> i32 {
        let top = usize::from(self.data[0]);
        match (CATEGORY_INDEX.get(top), CATEGORY_INDEX.get(top + 1)) {
            (Some(first), Some(end)) => end - first,
            _ => 0,
        }
    }

    /// `LLCategory::getSubCategory`: this category with its first zero level
    /// above the leaf set to `n + 1` (wrapping, so 255 sets 0); a copy of
    /// this one when the first three levels are all set.
    pub fn sub_category(self, n: u8) -> Category {
        let mut rv = self;
        if let Some(level) = rv.data[..CATEGORY_DEPTH - 1].iter_mut().find(|b| **b == 0) {
            *level = n.wrapping_add(1);
        }
        rv
    }

    /// `LLCategory::lookupName`: the name of the leaf category, `None`
    /// (`NULL`) past the table. An unknown top level gives `None` (bug 4),
    /// as the first unknown value, 7, always did.
    pub fn lookup_name(self) -> Option<&'static str> {
        let mut index = usize::from(self.data[0]);
        if index >= CATEGORY_NAME.len() {
            return None;
        }
        for &level in &self.data[1..] {
            if level == 0 {
                break;
            }
            // CATEGORY_INDEX maps every index below 8 into 0..8.
            index = CATEGORY_INDEX[index] as usize;
        }
        CATEGORY_NAME[index]
    }
}
