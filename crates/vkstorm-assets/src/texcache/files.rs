//! The file operations `LLTextureCache` made through `LLAPRFile` and
//! `LLFile`, with the same semantics: a read or write by name opens, seeks
//! and transfers, returning the byte count (0 when the file cannot be
//! opened); writes create but never truncate; sizes of missing files are 0.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// `LLAPRFile::size`: the file's size, 0 when it does not exist.
pub fn file_size(path: &str) -> i32 {
    match fs::metadata(path) {
        Ok(m) if m.is_file() => m.len().min(i32::MAX as u64) as i32,
        _ => 0,
    }
}

/// `LLAPRFile::isExist`.
pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// `LLAPRFile::readEx`: reads up to `len` bytes at `offset` into `buf`;
/// returns the count read, 0 when the file cannot be opened.
pub fn read_at(path: &str, offset: i64, buf: &mut [u8]) -> i32 {
    let Ok(mut f) = File::open(path) else {
        return 0;
    };
    if offset > 0 && f.seek(SeekFrom::Start(offset as u64)).is_err() {
        return 0;
    }
    read_fill(&mut f, buf)
}

fn read_fill(f: &mut File, buf: &mut [u8]) -> i32 {
    let mut total = 0usize;
    while total < buf.len() {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    total as i32
}

/// `LLAPRFile::writeEx`: creates the file if needed (never truncating),
/// seeks to `offset` (append when negative) and writes; returns the count
/// written, 0 when the file cannot be opened.
pub fn write_at(path: &str, offset: i64, data: &[u8]) -> i32 {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if offset < 0 {
        opts.append(true);
    }
    let Ok(mut f) = opts.open(path) else { return 0 };
    if offset > 0 && f.seek(SeekFrom::Start(offset as u64)).is_err() {
        return 0;
    }
    match f.write_all(data) {
        Ok(()) => data.len() as i32,
        Err(_) => 0,
    }
}

/// `LLAPRFile::remove` (errors ignored, as the C++ only logged them).
pub fn remove_file(path: &str) {
    let _ = fs::remove_file(path);
}

/// `LLFile::isdir`.
pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// `LLFile::mkdir` (one level; an existing directory is not an error).
pub fn mkdir(path: &str) {
    let _ = fs::create_dir(path);
}

/// `LLFile::rmdir`.
pub fn rmdir(path: &str) {
    let _ = fs::remove_dir(path);
}

/// `LLFile::rename`; true on success.
pub fn rename(from: &str, to: &str) -> bool {
    fs::rename(from, to).is_ok()
}

/// `LLDir::deleteFilesInDir(dir, "*")`: removes the regular files directly
/// in `dir` (not subdirectories). Returns the number removed.
pub fn delete_files_in_dir(dir: &str) -> u32 {
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
    }
    n
}

/// `LLDir::deleteDirAndContents`.
pub fn delete_dir_and_contents(dir: &str) {
    let _ = fs::remove_dir_all(dir);
}

/// An open file handle with the `LLAPRFile` operations the header-entries
/// code used on one: seek, read, write.
pub struct OpenFile {
    f: File,
}

impl OpenFile {
    /// `new LLAPRFile(name, flags)`: read-only, or read-write without
    /// creation. `None` when the open fails (the C++ handle had no file and
    /// every transfer returned 0).
    pub fn open(path: &str, read_only: bool) -> Option<OpenFile> {
        let f = if read_only {
            File::open(path).ok()?
        } else {
            OpenOptions::new().read(true).write(true).open(path).ok()?
        };
        Some(OpenFile { f })
    }

    pub fn seek_set(&mut self, offset: i64) -> bool {
        self.f.seek(SeekFrom::Start(offset.max(0) as u64)).is_ok()
    }

    pub fn seek_cur(&mut self, delta: i64) -> bool {
        self.f.seek(SeekFrom::Current(delta)).is_ok()
    }

    pub fn read(&mut self, buf: &mut [u8]) -> i32 {
        read_fill(&mut self.f, buf)
    }

    pub fn write(&mut self, data: &[u8]) -> i32 {
        match self.f.write_all(data) {
            Ok(()) => data.len() as i32,
            Err(_) => 0,
        }
    }
}
