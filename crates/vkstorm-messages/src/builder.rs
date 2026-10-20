//! `LLTemplateMessageBuilder`: accumulates the blocks and variables of one
//! message and packs them (`lltemplatemessagebuilder.cpp`).
//!
//! The wire layout is the C++ one: the 6-byte packet id (the caller fills
//! the flags and the packet number; the builder writes the offset byte), the
//! message number (1, 2 or 4 bytes by frequency), `offset` bytes of padding,
//! then every block of the template in template order: a repeat count byte
//! for a `Variable` block, then for each repeat every variable in template
//! order, a variable-length field as its size (1, 2 or 4 bytes, little
//! endian) followed by the bytes. Data is stored as given, in host byte
//! order, as `htolememcpy` does on a little-endian host.
//!
//! Where the C++ aborted (`LL_ERRS`) this returns an error with the same
//! wording for the caller to abort with; where it warned, the result says
//! so. Two differences, both on the error path: a `Multiple` block gets its
//! `number` repeats and no more (the C++ accepted one extra repeat and then
//! failed when building), and `remove_last_block` on a block with a single
//! repeat leaves that repeat in place (the C++ deleted it and then reported
//! that it had not).

use crate::template::Frequency;
use crate::template::{BlockType, Template, VarType};
use crate::{MAX_BLOCKS, MAX_BUFFER_SIZE, MTUBYTES, PACKET_ID_SIZE, PHL_OFFSET};
use std::fmt;
use std::sync::Arc;

/// A failure the C++ builder aborted on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuilderError {
    /// `newMessage` was not called.
    NoMessage,
    /// `nextBlock` named something that is not a block of the message.
    NotABlock,
    /// `nextBlock` called again for a `Single` block.
    SingleRepeated,
    /// `nextBlock` called more than `number` times for a `Multiple` block.
    MultipleExceeded { count: i32, number: i32 },
    /// More than `MAX_BLOCKS` repeats of a `Variable` block.
    TooManyBlocks,
    /// `add*` before `nextBlock`.
    NoBlock,
    /// `add*` named something that is not a variable of the current block.
    NotAVariable,
    /// A fixed-size variable given data of another size.
    FixedSizeMismatch { requested: i32, template: i32 },
}

impl fmt::Display for BuilderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuilderError::NoMessage => write!(f, "newMessage not called"),
            BuilderError::NotABlock => write!(f, "not a block in the message"),
            BuilderError::SingleRepeated => write!(
                f,
                "nextBlock called multiple times for a block of type MBT_SINGLE"
            ),
            BuilderError::MultipleExceeded { count, number } => {
                write!(
                    f,
                    "nextBlock called {count} times exceeding {number} specified in type MBT_MULTIPLE"
                )
            }
            BuilderError::TooManyBlocks => write!(
                f,
                "Trying to pack too many blocks into MBT_VARIABLE type (limited to {MAX_BLOCKS})"
            ),
            BuilderError::NoBlock => write!(f, "setBlock not called prior to addData"),
            BuilderError::NotAVariable => write!(f, "not a variable in the block"),
            BuilderError::FixedSizeMismatch {
                requested,
                template,
            } => {
                write!(
                    f,
                    "is type MVT_FIXED but request size {requested} doesn't match template size {template}"
                )
            }
        }
    }
}

impl std::error::Error for BuilderError {}

/// What `add_data` wants the caller to log, when anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddOutcome {
    /// A `Variable 1` field was given more than 255 bytes: 255 were kept and
    /// the last of them set to 0, as the C++ did (it also wrote that 0 into
    /// the caller's buffer; this does not).
    pub clamped: bool,
    /// The data type given does not match the template's (`LLMsgVarData::addData`
    /// warned and stored the data anyway).
    pub type_mismatch: bool,
}

/// A failure of `build_message`, all `LL_ERRS` in the C++.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    NoMessage,
    UnexpectedFrequency,
    /// The buffer is too small for the packet header.
    BufferTooSmall,
    /// A `Variable` block's count byte would pass `MAX_BUFFER_SIZE`.
    CountOverflow,
    /// A `Multiple` block with fewer repeats than its number.
    MultipleIncomplete {
        block: String,
        count: i32,
        number: i32,
    },
    /// A variable never given data.
    VariableNotSet {
        block: String,
        variable: String,
    },
    /// A variable-length field whose size field is not 1, 2 or 4 bytes.
    UnknownSizeField {
        variable: String,
        size: i32,
    },
    /// Packing `attempted` bytes into a buffer of `size`.
    BufferOverflow {
        attempted: usize,
        size: usize,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::NoMessage => write!(f, "newMessage not called prior to buildMessage"),
            BuildError::UnexpectedFrequency => {
                write!(f, "unexpected message frequency in buildMessage")
            }
            BuildError::BufferTooSmall => {
                write!(f, "buildMessage buffer smaller than the packet header")
            }
            BuildError::CountOverflow => {
                write!(f, "buildBlock failed. Message excedding sendBuffersize.")
            }
            BuildError::MultipleIncomplete {
                block,
                count,
                number,
            } => {
                write!(
                    f,
                    "Block {block} is type MBT_MULTIPLE but only has data for {count} out of its {number} blocks"
                )
            }
            BuildError::VariableNotSet { block, variable } => {
                write!(
                    f,
                    "The variable {variable} in block {block} wasn't set prior to buildMessage call"
                )
            }
            BuildError::UnknownSizeField { variable, size } => {
                write!(
                    f,
                    "Attempting to build variable field {variable} with unknown size of {size}"
                )
            }
            BuildError::BufferOverflow { attempted, size } => {
                write!(
                    f,
                    "buildBlock failed. Attempted to pack {attempted} bytes into a buffer with size {size}."
                )
            }
        }
    }
}

impl std::error::Error for BuildError {}

#[derive(Clone, Copy, Debug)]
struct Slot {
    /// -1 until data is added.
    size: i32,
    /// The width of the size field for a variable-length field, else -1.
    data_size: i32,
    off: u32,
}

const UNSET: Slot = Slot {
    size: -1,
    data_size: -1,
    off: 0,
};

#[derive(Default)]
struct BlockState {
    /// Repeats added so far (`mBlockNumber` of the first repeat).
    count: i32,
    /// `count * variables` slots, repeat-major.
    slots: Vec<Slot>,
}

/// The message builder. One per thread of message building; cleared and
/// reused message after message without reallocating.
#[derive(Default)]
pub struct Builder {
    template: Option<Arc<Template>>,
    blocks: Vec<BlockState>,
    data: Vec<u8>,
    cur_block: Option<usize>,
    built: bool,
    clear: bool,
    send_total: i32,
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            clear: true,
            ..Default::default()
        }
    }

    /// `newMessage`. Returns whether the message is deprecated (the C++
    /// warned when sending one).
    pub fn new_message(&mut self, template: Arc<Template>) -> bool {
        self.built = false;
        self.clear = false;
        self.send_total = 0;
        self.cur_block = None;
        self.data.clear();
        let n = template.blocks.len();
        self.blocks.truncate(n);
        for b in &mut self.blocks {
            b.count = 0;
            b.slots.clear();
        }
        while self.blocks.len() < n {
            self.blocks.push(BlockState::default());
        }
        let deprecated = template.deprecation != crate::template::Deprecation::NotDeprecated;
        self.template = Some(template);
        deprecated
    }

    /// `clearMessage`.
    pub fn clear_message(&mut self) {
        self.built = false;
        self.clear = true;
        self.send_total = 0;
        self.template = None;
        self.cur_block = None;
        self.data.clear();
        for b in &mut self.blocks {
            b.count = 0;
            b.slots.clear();
        }
    }

    pub fn template(&self) -> Option<&Arc<Template>> {
        self.template.as_ref()
    }

    pub fn is_built(&self) -> bool {
        self.built
    }

    pub fn set_built(&mut self, b: bool) {
        self.built = b;
    }

    pub fn is_clear(&self) -> bool {
        self.clear
    }

    /// `getMessageSize`: the data bytes added so far.
    pub fn message_size(&self) -> i32 {
        self.send_total
    }

    /// `nextBlock`: starts a repeat of the block with `key`.
    pub fn next_block(&mut self, key: u64) -> Result<(), BuilderError> {
        let template = self.template.as_ref().ok_or(BuilderError::NoMessage)?;
        let b = template.block_index(key).ok_or(BuilderError::NotABlock)?;
        let block = &template.blocks[b];
        let state = &mut self.blocks[b];
        if state.count != 0 {
            match block.ty {
                BlockType::Single => return Err(BuilderError::SingleRepeated),
                BlockType::Multiple if state.count >= block.number => {
                    return Err(BuilderError::MultipleExceeded {
                        count: state.count,
                        number: block.number,
                    });
                }
                _ => {}
            }
            if state.count + 1 > MAX_BLOCKS {
                return Err(BuilderError::TooManyBlocks);
            }
        }
        state.count += 1;
        state
            .slots
            .extend(std::iter::repeat_n(UNSET, block.variables.len()));
        self.cur_block = Some(b);
        Ok(())
    }

    /// `removeLastBlock`: drops the last repeat of the current block.
    /// Returns false when there is nothing to drop or only one repeat (kept).
    pub fn remove_last_block(&mut self) -> bool {
        let (Some(template), Some(b)) = (self.template.as_ref(), self.cur_block) else {
            return false;
        };
        let block = &template.blocks[b];
        let state = &mut self.blocks[b];
        if state.count < 1 {
            return false;
        }
        for v in &block.variables {
            self.send_total -= v.size;
        }
        if state.count <= 1 {
            return false;
        }
        state.count -= 1;
        let n = block.variables.len();
        state.slots.truncate(state.slots.len() - n);
        true
    }

    /// `addData(varname, data, type, size)`: the data for variable `key` of
    /// the current repeat of the current block.
    pub fn add_data(
        &mut self,
        key: u64,
        data: &[u8],
        ty: VarType,
    ) -> Result<AddOutcome, BuilderError> {
        let template = self.template.as_ref().ok_or(BuilderError::NoMessage)?;
        let b = self.cur_block.ok_or(BuilderError::NoBlock)?;
        let block = &template.blocks[b];
        let v = block
            .variable_index(key)
            .ok_or(BuilderError::NotAVariable)?;
        let var = &block.variables[v];
        let mut outcome = AddOutcome::default();
        let state = &mut self.blocks[b];
        let nvars = block.variables.len();
        let slot_index = state.slots.len() - nvars + v;
        let mut data = data;
        let (size, data_size) = if var.ty == VarType::Variable {
            if var.size == 1 && data.len() > 255 {
                outcome.clamped = true;
                data = &data[..255];
            }
            (data.len() as i32, var.size)
        } else {
            if data.len() as i32 != var.size {
                return Err(BuilderError::FixedSizeMismatch {
                    requested: data.len() as i32,
                    template: var.size,
                });
            }
            (var.size, -1)
        };
        if ty != VarType::Variable
            && ty != VarType::Fixed
            && var.ty != VarType::Variable
            && var.ty != VarType::Fixed
            && var.ty != ty
        {
            outcome.type_mismatch = true;
        }
        let off = self.data.len() as u32;
        self.data.extend_from_slice(data);
        if outcome.clamped {
            let last = self.data.len() - 1;
            self.data[last] = 0;
        }
        state.slots[slot_index] = Slot {
            size,
            data_size,
            off,
        };
        self.send_total += size;
        Ok(outcome)
    }

    /// `isMessageFull`: over the MTU, or the block with `key` (when given)
    /// has all the repeats it may have.
    pub fn is_message_full(&self, key: Option<u64>) -> Result<bool, BuilderError> {
        if self.send_total > MTUBYTES {
            return Ok(true);
        }
        let Some(key) = key else { return Ok(false) };
        let template = self.template.as_ref().ok_or(BuilderError::NoMessage)?;
        let b = template.block_index(key).ok_or(BuilderError::NotABlock)?;
        let block = &template.blocks[b];
        let max = match block.ty {
            BlockType::Single => 1,
            BlockType::Multiple => block.number,
            _ => MAX_BLOCKS,
        };
        Ok(self.blocks[b].count >= max)
    }

    /// `buildMessage`: packs the message into `buffer` (whose length is the
    /// C++ `buffer_size`) with `offset` padding bytes after the message
    /// number, and returns the packed size. The packet id bytes other than
    /// the offset are left as they are.
    pub fn build_message(&mut self, buffer: &mut [u8], offset: u8) -> Result<usize, BuildError> {
        let template = self.template.as_ref().ok_or(BuildError::NoMessage)?;
        let header = PACKET_ID_SIZE + template.number_size() + offset as usize;
        if buffer.len() < header {
            return Err(BuildError::BufferTooSmall);
        }
        buffer[PHL_OFFSET] = offset;
        let mut result = PACKET_ID_SIZE;
        match template.frequency {
            Frequency::High => {
                buffer[result] = template.number as u8;
                result += 1;
            }
            Frequency::Medium => {
                buffer[result] = 255;
                buffer[result + 1] = (template.number & 255) as u8;
                result += 2;
            }
            Frequency::Low => {
                buffer[result] = 255;
                buffer[result + 1] = 255;
                buffer[result + 2..result + 4]
                    .copy_from_slice(&((template.number & 0xFFFF) as u16).to_be_bytes());
                result += 4;
            }
            Frequency::Null => return Err(BuildError::UnexpectedFrequency),
        }
        result += offset as usize;
        for (b, block) in template.blocks.iter().enumerate() {
            let state = &self.blocks[b];
            let buffer_size = buffer.len() - result;
            let mut r = 0usize;
            match block.ty {
                BlockType::Variable => {
                    if r + 1 < MAX_BUFFER_SIZE {
                        if result + r >= buffer.len() {
                            return Err(BuildError::BufferOverflow {
                                attempted: r + 1,
                                size: buffer_size,
                            });
                        }
                        buffer[result + r] = state.count as u8;
                        r += 1;
                    } else {
                        return Err(BuildError::CountOverflow);
                    }
                }
                BlockType::Multiple if state.count != block.number => {
                    return Err(BuildError::MultipleIncomplete {
                        block: block.name.clone(),
                        count: state.count,
                        number: block.number,
                    });
                }
                _ => {}
            }
            let nvars = block.variables.len();
            for repeat in 0..state.count.max(0) as usize {
                for (v, var) in block.variables.iter().enumerate() {
                    let slot = state.slots[repeat * nvars + v];
                    if slot.size == -1 {
                        return Err(BuildError::VariableNotSet {
                            block: block.name.clone(),
                            variable: var.name.clone(),
                        });
                    }
                    let size = slot.size as usize;
                    if slot.data_size > 0 {
                        let w = slot.data_size as usize;
                        if result + r + w > buffer.len() {
                            return Err(BuildError::BufferOverflow {
                                attempted: r + w,
                                size: buffer_size,
                            });
                        }
                        let dst = &mut buffer[result + r..result + r + w];
                        match w {
                            1 => dst[0] = size as u8,
                            2 => dst.copy_from_slice(&(size as u16).to_le_bytes()),
                            4 => dst.copy_from_slice(&(slot.size).to_le_bytes()),
                            _ => {
                                return Err(BuildError::UnknownSizeField {
                                    variable: var.name.clone(),
                                    size: slot.size,
                                });
                            }
                        }
                        r += w;
                    }
                    if size != 0 {
                        if r + size < buffer_size {
                            let src = &self.data[slot.off as usize..slot.off as usize + size];
                            buffer[result + r..result + r + size].copy_from_slice(src);
                            r += size;
                        } else {
                            return Err(BuildError::BufferOverflow {
                                attempted: r + size,
                                size: buffer_size,
                            });
                        }
                    }
                }
            }
            result += r;
        }
        self.built = true;
        Ok(result)
    }

    /// The repeats added for template block `b`.
    pub fn block_count(&self, b: usize) -> i32 {
        self.blocks.get(b).map_or(0, |s| s.count)
    }

    /// The data of variable `v` in repeat `repeat` of template block `b`:
    /// the bytes and the size-field width (-1 for fixed), or `None` when
    /// the slot has no data yet.
    pub fn slot_data(&self, b: usize, repeat: usize, v: usize) -> Option<(&[u8], i32)> {
        let template = self.template.as_ref()?;
        let nvars = template.blocks.get(b)?.variables.len();
        let slot = self.blocks.get(b)?.slots.get(repeat * nvars + v)?;
        if slot.size < 0 {
            return None;
        }
        Some((
            &self.data[slot.off as usize..slot.off as usize + slot.size as usize],
            slot.data_size,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::Block;

    fn template() -> Arc<Template> {
        let mut t = Template::new("TestMessage", 1, 1, Frequency::High);
        let mut b = Block::new("Test0", 0, BlockType::Variable, 1);
        b.add_variable("Test0", 0, VarType::U32, 4).unwrap();
        b.add_variable("Str", 0, VarType::Variable, 1).unwrap();
        t.add_block(b).unwrap();
        Arc::new(t.with_index_keys())
    }

    #[test]
    fn builds_variable_block() {
        let t = template();
        let bk = t.blocks[0].key;
        let v0 = t.blocks[0].variables[0].key;
        let v1 = t.blocks[0].variables[1].key;
        let mut b = Builder::new();
        b.new_message(t.clone());
        b.next_block(bk).unwrap();
        b.add_data(v0, &7u32.to_le_bytes(), VarType::U32).unwrap();
        b.add_data(v1, b"hi\0", VarType::Variable).unwrap();
        b.next_block(bk).unwrap();
        b.add_data(v0, &9u32.to_le_bytes(), VarType::U32).unwrap();
        b.add_data(v1, b"", VarType::Variable).unwrap();
        let mut buf = [0u8; 64];
        let n = b.build_message(&mut buf, 0).unwrap();
        assert_eq!(
            &buf[..n],
            &[
                0, 0, 0, 0, 0, 0, 1, 2, 7, 0, 0, 0, 3, b'h', b'i', 0, 9, 0, 0, 0, 0
            ]
        );
        assert_eq!(b.message_size(), 4 + 3 + 4);
        assert!(b.is_built());
        assert!(b.remove_last_block());
        let mut buf = [0u8; 64];
        let n = b.build_message(&mut buf, 2).unwrap();
        assert_eq!(
            &buf[..n],
            &[0, 0, 0, 0, 0, 2, 1, 0, 0, 1, 7, 0, 0, 0, 3, b'h', b'i', 0]
        );
    }

    #[test]
    fn errors() {
        let t = template();
        let bk = t.blocks[0].key;
        let v0 = t.blocks[0].variables[0].key;
        let mut b = Builder::new();
        assert_eq!(b.next_block(bk), Err(BuilderError::NoMessage));
        b.new_message(t.clone());
        assert_eq!(b.next_block(99), Err(BuilderError::NotABlock));
        assert_eq!(
            b.add_data(v0, &[0; 4], VarType::U32),
            Err(BuilderError::NoBlock)
        );
        b.next_block(bk).unwrap();
        assert_eq!(
            b.add_data(v0, &[0; 3], VarType::U32),
            Err(BuilderError::FixedSizeMismatch {
                requested: 3,
                template: 4
            })
        );
        assert_eq!(
            b.add_data(77, &[0; 4], VarType::U32),
            Err(BuilderError::NotAVariable)
        );
        let mut buf = [0u8; 64];
        assert!(matches!(
            b.build_message(&mut buf, 0),
            Err(BuildError::VariableNotSet { .. })
        ));
        let big = vec![1u8; 300];
        let out = b
            .add_data(t.blocks[0].variables[1].key, &big, VarType::Variable)
            .unwrap();
        assert!(out.clamped);
        assert_eq!(b.slot_data(0, 0, 1).unwrap().0.len(), 255);
        assert_eq!(b.slot_data(0, 0, 1).unwrap().0[254], 0);
    }
}
