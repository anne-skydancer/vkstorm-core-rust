//! The file I/O behind `LLFileSystem` (one asset file per operation,
//! opened and closed each time, as the C++ did) and `LLLFSThread`'s read
//! and write requests.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ll_warns;

/// `LLFileSystem::getExists`: a regular file with a positive size.
pub fn exists(path: &str) -> bool {
    match fs::metadata(path) {
        Ok(m) => m.is_file() && m.len() > 0,
        Err(_) => false,
    }
}

/// `LLFileSystem::getFileSize`: the size, 0 when absent.
pub fn file_size(path: &str) -> i32 {
    match fs::metadata(path) {
        Ok(m) => m.len().min(i32::MAX as u64) as i32,
        Err(_) => 0,
    }
}

/// `LLFileSystem::read`: opens, seeks to `position`, reads up to `bytes`.
/// Returns the count read (`Err` when the file cannot be opened or the
/// seek fails; 0 bytes read is `Ok(0)`, a failure for the C++ caller).
pub fn read_at(path: &str, position: i32, buf: &mut [u8]) -> Result<i32, ()> {
    let mut f = File::open(path).map_err(|_| ())?;
    f.seek(SeekFrom::Start(position.max(0) as u64))
        .map_err(|_| ())?;
    let mut total = 0usize;
    while total < buf.len() {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    Ok(total as i32)
}

/// `LLFileSystem`'s open modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Mode {
    Read = 1,
    Write = 2,
    ReadWrite = 3,
    Append = 6,
}

impl Mode {
    pub fn from_i32(v: i32) -> Mode {
        match v {
            3 => Mode::ReadWrite,
            6 => Mode::Append,
            2 => Mode::Write,
            _ => Mode::Read,
        }
    }
}

/// `LLFileSystem::write`: appends (`Append`), writes at `position` into an
/// existing file or creates it (`ReadWrite`), or truncates and writes
/// (any other mode). Returns the new position on success.
pub fn write_at(path: &str, mode: Mode, position: i32, data: &[u8]) -> Option<i32> {
    match mode {
        Mode::Append => {
            let mut f = OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .ok()?;
            f.write_all(data).ok()?;
            let pos = f.stream_position().ok()?;
            Some(pos.min(i32::MAX as u64) as i32)
        }
        Mode::ReadWrite => {
            if let Ok(mut f) = OpenOptions::new().read(true).write(true).open(path) {
                f.seek(SeekFrom::Start(position.max(0) as u64)).ok()?;
                f.write_all(data).ok()?;
                let pos = f.stream_position().ok()?;
                Some(pos.min(i32::MAX as u64) as i32)
            } else {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(path)
                    .ok()?;
                f.write_all(data).ok()?;
                let pos = f.stream_position().ok()?;
                Some(pos.min(i32::MAX as u64) as i32)
            }
        }
        _ => {
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .ok()?;
            f.write_all(data).ok()?;
            let pos = f.stream_position().ok()?;
            Some(pos.min(i32::MAX as u64) as i32)
        }
    }
}

/// `LLFileSystem::seek`'s position arithmetic: the new position, clamped
/// to the file with the C++'s warnings; `Err` carries the clamped position.
pub fn seek_position(position: i32, offset: i32, origin: i32, size: i32) -> Result<i32, i32> {
    let origin = if origin == -1 { position } else { origin };
    let new_pos = origin + offset;
    if new_pos > size {
        ll_warns!("Attempt to seek past end of file");
        Err(size)
    } else if new_pos < 0 {
        ll_warns!("Attempt to seek past beginning of file");
        Err(0)
    } else {
        Ok(new_pos)
    }
}

/// `LLFileSystem::removeFile`.
pub fn remove_file(path: &str) {
    let _ = fs::remove_file(path);
}

/// `LLFileSystem::renameFile`; false (with the warning) on failure.
pub fn rename_file(from: &str, to: &str) -> bool {
    match fs::rename(from, to) {
        Ok(()) => true,
        Err(e) => {
            ll_warns!("Failed to rename {} to {} reason: {}", from, to, e);
            false
        }
    }
}

/// `updateFileAccessTime`: bumps the modification time to now when it is
/// more than an hour old (the purge's age measure).
pub fn update_file_access_time(path: &str) {
    const TIME_THRESHOLD: u64 = 60 * 60;
    let cur = SystemTime::now();
    let Ok(m) = fs::metadata(path) else {
        ll_warns!(
            "Failed to read last write time for cache file {}: not found",
            path
        );
        return;
    };
    let Ok(last) = m.modified() else { return };
    let delta = cur.duration_since(last).unwrap_or(Duration::ZERO);
    if delta.as_secs() > TIME_THRESHOLD {
        let set = OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(cur));
        if let Err(e) = set {
            ll_warns!(
                "Failed to update last write time for cache file {}: {}",
                path,
                e
            );
        }
    }
}

/// `LLLFSThread::Request::processRequest` for a read: `offset` < 0 reads
/// from the end. Returns the bytes read (0 on failure, as the C++).
pub fn lfs_read(path: &str, offset: i32, buf: &mut [u8]) -> i32 {
    let Ok(mut f) = File::open(path) else {
        ll_warns!("LLLFS: Unable to read file: {}", path);
        return 0;
    };
    let seek = if offset < 0 {
        f.seek(SeekFrom::End(0))
    } else {
        f.seek(SeekFrom::Start(offset as u64))
    };
    if seek.is_err() {
        return 0;
    }
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

/// `LLLFSThread::Request::processRequest` for a write: creates, seeks to
/// `offset` (appends when negative), writes. Returns the bytes written.
pub fn lfs_write(path: &str, offset: i32, data: &[u8]) -> i32 {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if offset < 0 {
        opts.append(true);
    }
    let Ok(mut f) = opts.open(path) else {
        ll_warns!("LLLFS: Unable to write file: {}", path);
        return 0;
    };
    if offset >= 0 && f.seek(SeekFrom::Start(offset as u64)).is_err() {
        ll_warns!("LLLFS: Unable to write file (seek failed): {}", path);
        return 0;
    }
    match f.write_all(data) {
        Ok(()) => data.len() as i32,
        Err(_) => 0,
    }
}

/// Seconds since the epoch of a file's modification time (for the tests
/// and the viewer's diagnostics).
pub fn modified_secs(path: &str) -> Option<i64> {
    let m = fs::metadata(path).ok()?;
    let t = m.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        let dir = std::env::temp_dir().join(format!("vks_file_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.asset").to_string_lossy().into_owned();
        assert!(!exists(&p));
        assert_eq!(write_at(&p, Mode::Write, 0, b"hello"), Some(5));
        assert!(exists(&p));
        assert_eq!(write_at(&p, Mode::Append, 0, b" world"), Some(11));
        assert_eq!(write_at(&p, Mode::ReadWrite, 6, b"W"), Some(7));
        let mut buf = [0u8; 20];
        assert_eq!(read_at(&p, 0, &mut buf), Ok(11));
        assert_eq!(&buf[..11], b"hello World");
        assert_eq!(seek_position(0, 5, -1, 11), Ok(5));
        assert_eq!(seek_position(0, 50, -1, 11), Err(11));
        assert_eq!(seek_position(3, -5, -1, 11), Err(0));
        assert_eq!(lfs_read(&p, 6, &mut buf[..5]), 5);
        assert_eq!(&buf[..5], b"World");
        assert_eq!(lfs_write(&p, -1, b"!"), 1);
        assert_eq!(file_size(&p), 12);
        assert_eq!(write_at(&p, Mode::Write, 0, b"x"), Some(1));
        assert_eq!(file_size(&p), 1);
        let q = dir.join("b.asset").to_string_lossy().into_owned();
        assert!(rename_file(&p, &q));
        assert!(!exists(&p) && exists(&q));
        remove_file(&q);
        assert!(!exists(&q));
        let _ = fs::remove_dir_all(&dir);
    }
}
