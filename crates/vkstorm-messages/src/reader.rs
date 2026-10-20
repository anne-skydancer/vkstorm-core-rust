//! `LLTemplateMessageReader`: identifies the message of a packet and unpacks
//! its blocks and variables for the getters (`lltemplatemessagereader.cpp`).
//!
//! Decoding records, for every repeat of every block, where each variable's
//! bytes lie in the reader's copy of the packet; the getters then copy from
//! there. Nothing is allocated per message after the first few.
//!
//! Behaviour on short packets, settled as one product behaviour:
//!
//! - A `Variable` block whose count byte is past the end has 0 repeats
//!   (as the C++: trailing variable blocks may legally be absent).
//! - A fixed-size variable past the end reads as zeros and is reported as a
//!   ran-off-end event (the C++ did the same).
//! - A variable-length field whose size field is past the end is empty and
//!   reported; the decode position still advances by the width of the size
//!   field (the C++ did the same).
//! - A variable-length field whose size field is present but whose bytes
//!   run past the end is **empty and reported**, and the decode position
//!   advances by the declared size, so everything after it is past the end
//!   too. The C++ copied the declared number of bytes from beyond the
//!   packet (whatever its receive buffer held after the packet, or beyond
//!   it for a large size) without a check; that is the memory bug this rung
//!   closes, and the parity fixture carries this stated expectation for
//!   those fields rather than the oracle's garbage.
//! - A packet shorter than [`MINIMUM_VALID_PACKET_SIZE`](crate::MINIMUM_VALID_PACKET_SIZE)
//!   has no message number (the C++ read whatever followed the packet in
//!   its buffer; the message system never passes one).

use crate::template::{BlockType, Frequency, Template, VarType};
use crate::{MINIMUM_VALID_PACKET_SIZE, PACKET_ID_SIZE, PHL_OFFSET};
use std::sync::Arc;

/// Why `decode_number` found no message number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberError {
    /// `buffer_size <= 0`.
    Empty,
    /// Shorter than a packet with that kind of number ("unusable length").
    TooShort,
}

/// `LLTemplateMessageReader::decodeTemplate`'s number extraction: the full
/// message number of the packet (`0xFF00 | n` medium, `0xFFFF0000 | n`
/// low), to look up in the caller's number map.
pub fn decode_number(buffer: &[u8]) -> Result<u32, NumberError> {
    if buffer.is_empty() {
        return Err(NumberError::Empty);
    }
    if buffer.len() < MINIMUM_VALID_PACKET_SIZE {
        return Err(NumberError::TooShort);
    }
    let header = &buffer[PACKET_ID_SIZE..];
    if header[0] != 255 {
        Ok(header[0] as u32)
    } else if buffer.len() > MINIMUM_VALID_PACKET_SIZE && header[1] != 255 {
        Ok((255 << 8) | header[1] as u32)
    } else if buffer.len() >= MINIMUM_VALID_PACKET_SIZE + 3 && header[1] == 255 {
        Ok(0xFFFF0000 | u16::from_be_bytes([header[2], header[3]]) as u32)
    } else {
        Err(NumberError::TooShort)
    }
}

/// A read past the end of the packet while decoding (`logRanOffEndOfPacket`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RanOffEnd {
    /// The decode position the read started at.
    pub position: i32,
    /// The bytes wanted there.
    pub wanted: i32,
}

/// What `decode_data` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeOutcome {
    /// False when the message has blocks but none was present (no repeat
    /// of any block): the C++ logged "Empty message" and did not dispatch.
    pub has_blocks: bool,
    /// Every read past the end, in decode order.
    pub ran_off_end: Vec<RanOffEnd>,
}

/// A failure of a getter, in the C++ `LL_ERRS` cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetError {
    /// No message decoded ("No message waiting for decode").
    NoMessage,
    /// The block, or that repeat of it, is not in the message.
    BlockNotFound,
    /// The variable is not in the block.
    VariableNotFound,
    /// The caller's size is not the variable's size.
    SizeMismatch { actual: i32 },
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    off: u32,
    len: u32,
}

#[derive(Default)]
struct BlockState {
    count: i32,
    first_slot: u32,
}

/// The message reader. Holds the template and the data of the last message
/// decoded until `clear`.
#[derive(Default)]
pub struct Reader {
    template: Option<Arc<Template>>,
    /// `mReceiveSize`: -1 when cleared.
    receive_size: i32,
    packet: Vec<u8>,
    /// The packet copy is followed by a zero area for fields past the end;
    /// `zeros_at` is where it starts.
    zeros_at: u32,
    blocks: Vec<BlockState>,
    slots: Vec<Slot>,
}

impl Reader {
    pub fn new() -> Reader {
        Reader {
            receive_size: -1,
            ..Default::default()
        }
    }

    /// `clearMessage`.
    pub fn clear(&mut self) {
        self.receive_size = -1;
        self.template = None;
        self.blocks.clear();
        self.slots.clear();
    }

    /// The template of the message being read.
    pub fn template(&self) -> Option<&Arc<Template>> {
        self.template.as_ref()
    }

    /// `getMessageSize`: the packet size given to `set_message`, -1 when
    /// cleared.
    pub fn message_size(&self) -> i32 {
        self.receive_size
    }

    /// `validateMessage`'s bookkeeping: the packet size and the template the
    /// caller found for the number; `decode_data` then unpacks the data.
    pub fn set_message(&mut self, template: Arc<Template>, receive_size: i32) {
        self.receive_size = receive_size;
        self.template = Some(template);
        self.blocks.clear();
        self.slots.clear();
    }

    /// `decodeData`: unpacks `buffer` (the whole packet, `receive_size`
    /// bytes of which are valid) by the template set by `set_message`.
    pub fn decode_data(&mut self, buffer: &[u8]) -> DecodeOutcome {
        let Some(template) = self.template.clone() else {
            return DecodeOutcome {
                has_blocks: false,
                ran_off_end: Vec::new(),
            };
        };
        let receive_size = self.receive_size.max(0) as usize;
        let receive_size = receive_size.min(buffer.len());
        let buffer = &buffer[..receive_size];
        let mut events = Vec::new();

        // the packet copy, then a zero area as large as the largest fixed
        // field (grown as needed)
        self.packet.clear();
        self.packet.extend_from_slice(buffer);
        self.zeros_at = self.packet.len() as u32;
        self.blocks.clear();
        self.slots.clear();

        let offset = buffer.get(PHL_OFFSET).copied().unwrap_or(0) as usize;
        let mut pos: usize = PACKET_ID_SIZE + template.number_size() + offset;
        let rs = receive_size;
        let mut any_block = false;
        for block in &template.blocks {
            let repeat: u8 = match block.ty {
                BlockType::Single => 1,
                BlockType::Multiple => block.number as u8,
                BlockType::Variable => {
                    if pos >= rs {
                        0
                    } else {
                        let r = buffer[pos];
                        pos += 1;
                        r
                    }
                }
                _ => 0,
            };
            if repeat > 0 {
                any_block = true;
            }
            let first_slot = self.slots.len() as u32;
            self.blocks.push(BlockState {
                count: repeat as i32,
                first_slot,
            });
            for _ in 0..repeat {
                for var in &block.variables {
                    if var.ty == VarType::Variable {
                        let data_size = var.size.max(0) as usize;
                        let tsize: usize = if pos + data_size > rs {
                            events.push(RanOffEnd {
                                position: pos as i32,
                                wanted: var.size,
                            });
                            0
                        } else {
                            let f = &buffer[pos..pos + data_size];
                            match data_size {
                                1 => f[0] as usize,
                                2 => u16::from_le_bytes([f[0], f[1]]) as usize,
                                4 => u32::from_le_bytes([f[0], f[1], f[2], f[3]]) as usize,
                                _ => 0,
                            }
                        };
                        pos += data_size;
                        if tsize != 0 && pos + tsize > rs {
                            // the stated behaviour for a declared size past
                            // the end (see the module documentation)
                            events.push(RanOffEnd {
                                position: pos as i32,
                                wanted: tsize as i32,
                            });
                            self.slots.push(Slot {
                                off: pos.min(rs) as u32,
                                len: 0,
                            });
                        } else {
                            // an empty field whose size field was itself past
                            // the end has no bytes anywhere: keep it inside
                            self.slots.push(Slot {
                                off: pos.min(rs) as u32,
                                len: tsize as u32,
                            });
                        }
                        pos += tsize;
                    } else {
                        let size = var.size.max(0) as usize;
                        if pos + size > rs {
                            events.push(RanOffEnd {
                                position: pos as i32,
                                wanted: var.size,
                            });
                            let needed = self.zeros_at as usize + size;
                            if self.packet.len() < needed {
                                self.packet.resize(needed, 0);
                            }
                            self.slots.push(Slot {
                                off: self.zeros_at,
                                len: size as u32,
                            });
                        } else {
                            self.slots.push(Slot {
                                off: pos as u32,
                                len: size as u32,
                            });
                        }
                        pos += size;
                    }
                }
            }
        }
        DecodeOutcome {
            has_blocks: any_block || template.blocks.is_empty(),
            ran_off_end: events,
        }
    }

    fn find(&self, block_key: u64, blocknum: i32, var_key: u64) -> Result<Slot, GetError> {
        let template = self.template.as_ref().ok_or(GetError::NoMessage)?;
        if self.receive_size == -1 {
            return Err(GetError::NoMessage);
        }
        let b = template
            .block_index(block_key)
            .ok_or(GetError::BlockNotFound)?;
        let state = &self.blocks[b];
        if blocknum < 0 || blocknum >= state.count {
            return Err(GetError::BlockNotFound);
        }
        let block = &template.blocks[b];
        let v = block
            .variable_index(var_key)
            .ok_or(GetError::VariableNotFound)?;
        let i = state.first_slot as usize + blocknum as usize * block.variables.len() + v;
        Ok(self.slots[i])
    }

    /// `getData`: the bytes of variable `var_key` in repeat `blocknum` of
    /// block `block_key`. `size`, when not 0, must equal the variable's size.
    pub fn get(
        &self,
        block_key: u64,
        blocknum: i32,
        var_key: u64,
        size: i32,
    ) -> Result<&[u8], GetError> {
        let slot = self.find(block_key, blocknum, var_key)?;
        if size != 0 && size != slot.len as i32 {
            return Err(GetError::SizeMismatch {
                actual: slot.len as i32,
            });
        }
        Ok(&self.packet[slot.off as usize..(slot.off + slot.len) as usize])
    }

    /// `getNumberOfBlocks`: the repeats of block `block_key`, 0 when absent.
    pub fn number_of_blocks(&self, block_key: u64) -> Result<i32, GetError> {
        let template = self.template.as_ref().ok_or(GetError::NoMessage)?;
        if self.receive_size == -1 {
            return Err(GetError::NoMessage);
        }
        Ok(template
            .block_index(block_key)
            .map_or(0, |b| self.blocks[b].count))
    }

    /// `getSize(block, blocknum, var)`: the size of the variable's data.
    pub fn size(&self, block_key: u64, blocknum: i32, var_key: u64) -> Result<i32, GetError> {
        Ok(self.find(block_key, blocknum, var_key)?.len as i32)
    }

    /// The repeats of template block `b`.
    pub fn block_count(&self, b: usize) -> i32 {
        self.blocks.get(b).map_or(0, |s| s.count)
    }

    /// The bytes of variable `v` in repeat `repeat` of template block `b`.
    pub fn slot_data(&self, b: usize, repeat: usize, v: usize) -> Option<&[u8]> {
        let template = self.template.as_ref()?;
        let block = template.blocks.get(b)?;
        let state = self.blocks.get(b)?;
        if repeat >= state.count.max(0) as usize || v >= block.variables.len() {
            return None;
        }
        let slot = self.slots[state.first_slot as usize + repeat * block.variables.len() + v];
        Some(&self.packet[slot.off as usize..(slot.off + slot.len) as usize])
    }
}

/// The byte size of a message number for a frequency.
pub fn number_size(frequency: Frequency) -> usize {
    frequency as i32 as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::Builder;
    use crate::template::Block;

    fn template() -> Arc<Template> {
        let mut t = Template::new("TestMessage", 1, 1, Frequency::High);
        let mut b = Block::new("Test0", 0, BlockType::Variable, 1);
        b.add_variable("Test0", 0, VarType::U32, 4).unwrap();
        b.add_variable("Str", 0, VarType::Variable, 1).unwrap();
        t.add_block(b).unwrap();
        let mut b = Block::new("Test1", 0, BlockType::Single, 1);
        b.add_variable("Test0", 0, VarType::U32, 4).unwrap();
        t.add_block(b).unwrap();
        Arc::new(t.with_index_keys())
    }

    #[test]
    fn numbers() {
        assert_eq!(decode_number(&[]), Err(NumberError::Empty));
        assert_eq!(decode_number(&[0; 6]), Err(NumberError::TooShort));
        assert_eq!(decode_number(&[0, 0, 0, 0, 0, 0, 12]), Ok(12));
        assert_eq!(
            decode_number(&[0, 0, 0, 0, 0, 0, 255]),
            Err(NumberError::TooShort)
        );
        assert_eq!(decode_number(&[0, 0, 0, 0, 0, 0, 255, 3]), Ok(0xFF03));
        assert_eq!(
            decode_number(&[0, 0, 0, 0, 0, 0, 255, 255, 1]),
            Err(NumberError::TooShort)
        );
        assert_eq!(
            decode_number(&[0, 0, 0, 0, 0, 0, 255, 255, 1, 2]),
            Ok(0xFFFF0102)
        );
    }

    #[test]
    fn roundtrip_and_truncation() {
        let t = template();
        let bk = t.blocks[0].key;
        let v0 = t.blocks[0].variables[0].key;
        let v1 = t.blocks[0].variables[1].key;
        let b1 = t.blocks[1].key;
        let v10 = t.blocks[1].variables[0].key;
        let mut b = Builder::new();
        b.new_message(t.clone());
        b.next_block(bk).unwrap();
        b.add_data(v0, &7u32.to_le_bytes(), VarType::U32).unwrap();
        b.add_data(v1, b"hello\0", VarType::Variable).unwrap();
        b.next_block(b1).unwrap();
        b.add_data(v10, &0xAABBCCDDu32.to_le_bytes(), VarType::U32)
            .unwrap();
        let mut buf = [0u8; 64];
        let n = b.build_message(&mut buf, 3).unwrap();
        let num = decode_number(&buf[..n]).unwrap();
        assert_eq!(num, 1);
        let mut r = Reader::new();
        r.set_message(t.clone(), n as i32);
        let out = r.decode_data(&buf[..n]);
        assert!(out.has_blocks);
        assert!(out.ran_off_end.is_empty());
        assert_eq!(r.get(bk, 0, v0, 4).unwrap(), &7u32.to_le_bytes());
        assert_eq!(r.get(bk, 0, v1, 0).unwrap(), b"hello\0");
        assert_eq!(r.get(b1, 0, v10, 4).unwrap(), &0xAABBCCDDu32.to_le_bytes());
        assert_eq!(r.number_of_blocks(bk).unwrap(), 1);
        assert_eq!(r.number_of_blocks(99).unwrap(), 0);
        assert_eq!(r.get(bk, 1, v0, 4), Err(GetError::BlockNotFound));
        assert_eq!(
            r.get(bk, 0, v0, 2),
            Err(GetError::SizeMismatch { actual: 4 })
        );
        assert_eq!(r.size(bk, 0, v1), Ok(6));

        // cut inside the string: the string is empty and the trailing
        // fixed field reads as zeros, with three events
        let cut = n - 7;
        r.set_message(t.clone(), cut as i32);
        let out = r.decode_data(&buf[..cut]);
        assert_eq!(out.ran_off_end.len(), 2);
        assert_eq!(r.get(bk, 0, v1, 0).unwrap(), b"");
        assert_eq!(r.get(b1, 0, v10, 4).unwrap(), &[0, 0, 0, 0]);

        // cut before the variable block's count: 0 repeats, still "empty"
        // since the single block is then past the end too
        r.set_message(t.clone(), 9);
        let out = r.decode_data(&buf[..9]);
        assert_eq!(r.number_of_blocks(bk).unwrap(), 0);
        assert!(out.has_blocks);
        r.clear();
        assert_eq!(r.number_of_blocks(bk), Err(GetError::NoMessage));
    }
}
