//! Chunked IO — port of the Ruby gem's `chunked/` subsystem
//! (`TODO.ref-parity/57`): a bounded-memory writer that splits
//! output into fixed-size chunks (`Writer::DEFAULT_CHUNK_SIZE` =
//! 64 MiB) plus a memory budget manager (`MemoryManager`,
//! 256 MiB default) with allocate/release accounting.

#![forbid(unsafe_code)]

use std::io;
use std::path::{Path, PathBuf};

/// Default chunk size (the Ruby `Writer::DEFAULT_CHUNK_SIZE`).
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024 * 1024;
/// Default memory budget (the Ruby `MemoryManager::DEFAULT_MAX_MEMORY`).
pub const DEFAULT_MAX_MEMORY: usize = 256 * 1024 * 1024;

/// Memory budget with allocate/release accounting. Overflow is
/// reported, never silently allowed — callers spill to disk or
/// propagate (the Ruby `strategy: :disk` decision stays with them).
#[derive(Debug)]
pub struct MemoryManager {
    max: usize,
    live: usize,
}

impl MemoryManager {
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self { max, live: 0 }
    }

    /// Reserve `size` bytes.
    ///
    /// # Errors
    ///
    /// `Err(shortfall)` when the budget would be exceeded; nothing
    /// is reserved then.
    pub fn allocate(&mut self, size: usize) -> Result<(), usize> {
        let would = self
            .live
            .checked_add(size)
            .ok_or(self.max.saturating_sub(self.live).max(1))?;
        if would > self.max {
            return Err(would - self.max);
        }
        self.live = would;
        Ok(())
    }

    /// Return `size` reserved bytes.
    pub fn release(&mut self, size: usize) {
        self.live = self.live.saturating_sub(size);
    }

    /// Bytes currently reserved.
    #[must_use]
    pub fn live(&self) -> usize {
        self.live
    }

    #[must_use]
    pub fn over_limit(&self) -> bool {
        self.live > self.max
    }
}

/// Receives one chunk at a time, in order. Implementations: a file
/// per chunk (`ChunkFiles`), a hash, a network stream — anything
/// that never needs the whole input in memory.
pub trait ChunkSink {
    /// Persist chunk `index` (`data.len() == chunk_size` except for
    /// the final chunk).
    ///
    /// # Errors
    ///
    /// Implementation-specific I/O errors.
    fn write_chunk(&mut self, index: usize, data: &[u8]) -> io::Result<()>;
}

/// Splits a byte stream into fixed-size files under a directory:
/// `NAME.0000`, `NAME.0001`, … (4-digit, zero-padded — sorts
/// lexically up to 10,000 chunks; use a wider pad for more).
pub struct ChunkFiles {
    dir: PathBuf,
    name: String,
}

impl ChunkFiles {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            name: name.into(),
        }
    }

    fn path(&self, index: usize) -> PathBuf {
        self.dir.join(format!("{}.{:04}", self.name, index))
    }
}

impl ChunkSink for ChunkFiles {
    fn write_chunk(&mut self, index: usize, data: &[u8]) -> io::Result<()> {
        std::fs::write(self.path(index), data)
    }
}

/// Fixed-size chunked writer (the Ruby `Writer`): `write` buffers
/// into the current chunk; full chunks flush to the sink. `finish`
/// flushes the tail. Chunk boundaries are a pure function of the
/// input — byte order and size are independent of the write
/// partitioning.
pub struct ChunkedWriter<S: ChunkSink> {
    sink: S,
    chunk_size: usize,
    buf: Vec<u8>,
    written_chunks: usize,
    written_bytes: u64,
}

impl<S: ChunkSink> ChunkedWriter<S> {
    #[must_use]
    pub fn new(sink: S, chunk_size: usize) -> Self {
        Self {
            sink,
            chunk_size: chunk_size.max(1),
            buf: Vec::new(),
            written_chunks: 0,
            written_bytes: 0,
        }
    }

    /// Absorb `data`, flushing every full chunk.
    ///
    /// # Errors
    ///
    /// Sink errors propagate.
    pub fn write(&mut self, mut data: &[u8]) -> io::Result<()> {
        loop {
            let room = self.chunk_size - self.buf.len();
            let take = room.min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == self.chunk_size {
                let chunk = std::mem::take(&mut self.buf);
                self.sink.write_chunk(self.written_chunks, &chunk)?;
                self.written_chunks += 1;
                self.written_bytes += chunk.len() as u64;
            }
            if data.is_empty() {
                return Ok(());
            }
        }
    }

    /// Flush the tail chunk (no-op when the stream was chunk-
    /// aligned) and return `(chunks, bytes)`.
    ///
    /// # Errors
    ///
    /// Sink errors propagate.
    pub fn finish(mut self) -> io::Result<(usize, u64)> {
        if !self.buf.is_empty() {
            let chunk = std::mem::take(&mut self.buf);
            let index = self.written_chunks;
            self.sink.write_chunk(index, &chunk)?;
            self.written_chunks += 1;
            self.written_bytes += chunk.len() as u64;
        }
        Ok((self.written_chunks, self.written_bytes))
    }

    /// Bytes accepted so far (accepted, not necessarily flushed).
    #[must_use]
    pub fn written_bytes(&self) -> u64 {
        self.written_bytes + self.buf.len() as u64
    }
}

/// Read a chunk-file set produced by [`ChunkFiles`] back into one
/// buffer, in index order. (The `Reader` half of the Ruby pair;
/// callers processing chunks one at a time should read the files
/// individually to stay bounded.)
///
/// # Errors
///
/// Missing chunk or I/O error.
pub fn read_chunks(dir: &Path, name: &str) -> io::Result<Vec<u8>> {
    let files = ChunkFiles::new(dir, name);
    let mut out = Vec::new();
    for index in 0.. {
        let path = files.path(index);
        if !path.is_file() {
            if index == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no chunk 0 under {}", dir.display()),
                ));
            }
            return Ok(out);
        }
        out.extend_from_slice(&std::fs::read(&path)?);
    }
    unreachable!("loop returns")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct VecSink(Vec<Vec<u8>>);
    impl ChunkSink for VecSink {
        fn write_chunk(&mut self, _index: usize, data: &[u8]) -> io::Result<()> {
            self.0.push(data.to_vec());
            Ok(())
        }
    }

    #[test]
    fn chunk_boundaries_are_partition_independent() {
        let input: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let baseline = {
            let mut w = ChunkedWriter::new(VecSink(Vec::new()), 1024);
            w.write(&input).unwrap();
            w.finish().unwrap()
        };
        // Random-ish partitioning, same output chunking.
        let mut w = ChunkedWriter::new(VecSink(Vec::new()), 1024);
        let mut i = 0;
        while i < input.len() {
            let n = (37 + i % 291).min(input.len() - i);
            w.write(&input[i..i + n]).unwrap();
            i += n;
        }
        assert_eq!(w.finish().unwrap(), baseline);
        assert_eq!(baseline.0, 10); // ceil(10000/1024)
        assert_eq!(baseline.1, 10_000);
    }

    #[test]
    fn memory_manager_accounts() {
        let mut m = MemoryManager::new(100);
        m.allocate(60).unwrap();
        m.allocate(40).unwrap();
        assert!(m.allocate(1).is_err());
        assert_eq!(m.live(), 100);
        m.release(40);
        m.allocate(1).unwrap();
        assert_eq!(m.live(), 61);
        assert!(!m.over_limit());
    }

    #[test]
    fn chunk_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("ozip-chunked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input: Vec<u8> = (0..5000u32).map(|i| (i % 7) as u8).collect();
        let files = ChunkFiles::new(&dir, "blob");
        let mut w = ChunkedWriter::new(files, 1024);
        w.write(&input).unwrap();
        let (chunks, bytes) = w.finish().unwrap();
        assert_eq!((chunks, bytes), (5, 5000));
        let restored = read_chunks(&dir, "blob").unwrap();
        assert_eq!(restored, input);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
