//! The message template model: messages, blocks and variables with the
//! viewer's enumerations and size rules (`llmessagetemplate.h`).
//!
//! A [`Template`] is used in two roles. The parser produces templates with
//! names and no keys; the viewer builds runtime templates from its own
//! `LLMessageTemplate` objects, giving every block and variable the integer
//! key it later passes to the builder and reader (its canonical string
//! pointer). The crate's own tests give sequential keys.

/// `EMsgVariableType`, with the C++ numbering.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarType {
    Null = 0,
    Fixed = 1,
    Variable = 2,
    U8 = 3,
    U16 = 4,
    U32 = 5,
    U64 = 6,
    S8 = 7,
    S16 = 8,
    S32 = 9,
    S64 = 10,
    F32 = 11,
    F64 = 12,
    LLVector3 = 13,
    LLVector3d = 14,
    LLVector4 = 15,
    LLQuaternion = 16,
    LLUUID = 17,
    Bool = 18,
    IpAddr = 19,
    IpPort = 20,
    U16Vec3 = 21,
    U16Quat = 22,
    S16Array = 23,
    Eol = 24,
}

impl VarType {
    pub fn from_i32(v: i32) -> Option<VarType> {
        use VarType::*;
        Some(match v {
            0 => Null,
            1 => Fixed,
            2 => Variable,
            3 => U8,
            4 => U16,
            5 => U32,
            6 => U64,
            7 => S8,
            8 => S16,
            9 => S32,
            10 => S64,
            11 => F32,
            12 => F64,
            13 => LLVector3,
            14 => LLVector3d,
            15 => LLVector4,
            16 => LLQuaternion,
            17 => LLUUID,
            18 => Bool,
            19 => IpAddr,
            20 => IpPort,
            21 => U16Vec3,
            22 => U16Quat,
            23 => S16Array,
            24 => Eol,
            _ => return None,
        })
    }

    /// The byte size `htolememcpy` insists on for the type, or `None` when
    /// any size is accepted (fixed, variable, bytes).
    pub fn swizzle_size(self) -> Option<usize> {
        use VarType::*;
        match self {
            U16 | S16 => Some(2),
            U32 | S32 | F32 => Some(4),
            U64 | S64 | F64 => Some(8),
            LLVector3 | LLQuaternion => Some(12),
            LLVector3d => Some(24),
            LLVector4 => Some(16),
            U16Vec3 => Some(6),
            U16Quat => Some(8),
            _ => None,
        }
    }

    /// The template keyword and the fixed size the parser assigns, for the
    /// types the template language names directly.
    pub fn keyword(self) -> Option<(&'static str, i32)> {
        use VarType::*;
        Some(match self {
            U8 => ("U8", 1),
            U16 => ("U16", 2),
            U32 => ("U32", 4),
            U64 => ("U64", 8),
            S8 => ("S8", 1),
            S16 => ("S16", 2),
            S32 => ("S32", 4),
            S64 => ("S64", 8),
            F32 => ("F32", 4),
            F64 => ("F64", 8),
            LLVector3 => ("LLVector3", 12),
            LLVector3d => ("LLVector3d", 24),
            LLVector4 => ("LLVector4", 16),
            LLQuaternion => ("LLQuaternion", 12),
            LLUUID => ("LLUUID", 16),
            Bool => ("BOOL", 1),
            IpAddr => ("IPADDR", 4),
            IpPort => ("IPPORT", 2),
            _ => return None,
        })
    }
}

/// `EMsgBlockType`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockType {
    Null = 0,
    Single = 1,
    Multiple = 2,
    Variable = 3,
    Eof = 4,
}

impl BlockType {
    pub fn from_i32(v: i32) -> Option<BlockType> {
        Some(match v {
            0 => BlockType::Null,
            1 => BlockType::Single,
            2 => BlockType::Multiple,
            3 => BlockType::Variable,
            4 => BlockType::Eof,
            _ => return None,
        })
    }
}

/// `EMsgFrequency`: the value is the size of the message number on the wire.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    Null = 0,
    High = 1,
    Medium = 2,
    Low = 4,
}

impl Frequency {
    pub fn from_i32(v: i32) -> Option<Frequency> {
        Some(match v {
            0 => Frequency::Null,
            1 => Frequency::High,
            2 => Frequency::Medium,
            4 => Frequency::Low,
            _ => return None,
        })
    }
}

/// `EMsgTrust`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted = 0,
    NotTrusted = 1,
}

/// `EMsgEncoding`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Unencoded = 0,
    ZeroCoded = 1,
}

/// `EMsgDeprecation`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deprecation {
    NotDeprecated = 0,
    UdpDeprecated = 1,
    UdpBlacklisted = 2,
    Deprecated = 3,
}

impl Deprecation {
    pub fn from_i32(v: i32) -> Option<Deprecation> {
        Some(match v {
            0 => Deprecation::NotDeprecated,
            1 => Deprecation::UdpDeprecated,
            2 => Deprecation::UdpBlacklisted,
            3 => Deprecation::Deprecated,
            _ => return None,
        })
    }
}

/// `LLMessageVariable`: `size` is the fixed byte size, or for
/// [`VarType::Variable`] the width of the size field (1, 2 or 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub key: u64,
    pub ty: VarType,
    pub size: i32,
}

/// `LLMessageBlock`. `number` is the repeat count of a `Multiple` block and 1
/// otherwise; `total_size` is the sum of the fixed variable sizes, or -1 once
/// a variable-length field is present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub name: String,
    pub key: u64,
    pub ty: BlockType,
    pub number: i32,
    pub variables: Vec<Variable>,
    pub total_size: i32,
}

impl Block {
    pub fn new(name: impl Into<String>, key: u64, ty: BlockType, number: i32) -> Block {
        Block {
            name: name.into(),
            key,
            ty,
            number,
            variables: Vec::new(),
            total_size: 0,
        }
    }

    /// `LLMessageBlock::addVariable`. A duplicate name is an error (the C++
    /// aborted); returns the variable's index.
    pub fn add_variable(
        &mut self,
        name: impl Into<String>,
        key: u64,
        ty: VarType,
        size: i32,
    ) -> Result<usize, String> {
        let name = name.into();
        if self.variables.iter().any(|v| v.name == name) {
            return Err(format!("{name} has already been used as a variable name!"));
        }
        if ty != VarType::Variable && self.total_size != -1 {
            self.total_size += size;
        } else {
            self.total_size = -1;
        }
        self.variables.push(Variable {
            name,
            key,
            ty,
            size,
        });
        Ok(self.variables.len() - 1)
    }

    pub fn variable_index(&self, key: u64) -> Option<usize> {
        self.variables.iter().position(|v| v.key == key)
    }

    pub fn variable_by_name(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|v| v.name == name)
    }
}

/// `LLMessageTemplate`, the definition part (the handler, statistics and ban
/// flags stay with the viewer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub key: u64,
    /// The full message number: `High` as given, `Medium` `0xFF00 | n`,
    /// `Low` `0xFFFF0000 | n`.
    pub number: u32,
    pub frequency: Frequency,
    pub trust: Trust,
    pub encoding: Encoding,
    pub deprecation: Deprecation,
    pub blocks: Vec<Block>,
    pub total_size: i32,
}

impl Template {
    pub fn new(name: impl Into<String>, key: u64, number: u32, frequency: Frequency) -> Template {
        Template {
            name: name.into(),
            key,
            number,
            frequency,
            trust: Trust::NotTrusted,
            encoding: Encoding::ZeroCoded,
            deprecation: Deprecation::NotDeprecated,
            blocks: Vec::new(),
            total_size: 0,
        }
    }

    /// `LLMessageTemplate::addBlock`. A duplicate name is an error; returns
    /// the block's index.
    pub fn add_block(&mut self, block: Block) -> Result<usize, String> {
        if self.blocks.iter().any(|b| b.name == block.name) {
            return Err(format!(
                "Block {}has already been used as a block name!",
                block.name
            ));
        }
        if self.total_size != -1
            && block.total_size != -1
            && (block.ty == BlockType::Single || block.ty == BlockType::Multiple)
        {
            self.total_size += block.number * block.total_size;
        } else {
            self.total_size = -1;
        }
        self.blocks.push(block);
        Ok(self.blocks.len() - 1)
    }

    pub fn block_index(&self, key: u64) -> Option<usize> {
        self.blocks.iter().position(|b| b.key == key)
    }

    pub fn block_by_name(&self, name: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.name == name)
    }

    /// The size of the message number on the wire (`EMsgFrequency`'s value).
    pub fn number_size(&self) -> usize {
        self.frequency as i32 as usize
    }

    /// Gives every block and variable a key from its index: block `b` gets
    /// `(b + 1) << 16`, its variable `v` gets `((b + 1) << 16) | (v + 1)`.
    /// For callers that have no string table (the crate's tests).
    pub fn with_index_keys(mut self) -> Template {
        for (b, block) in self.blocks.iter_mut().enumerate() {
            block.key = ((b as u64) + 1) << 16;
            for (v, var) in block.variables.iter_mut().enumerate() {
                var.key = block.key | ((v as u64) + 1);
            }
        }
        self
    }
}
