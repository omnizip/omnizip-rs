//! Chunked IO — port of the Ruby gem's `chunked/` subsystem
//! (`omnizip-rs #712`): a bounded-memory writer that splits
//! output into fixed-size chunks (`Writer::DEFAULT_CHUNK_SIZE` =
//! 64 MiB), a chunked file reader, plus a memory budget manager
//! (`MemoryManager`, 256 MiB default) with allocate/release
//! accounting and disk-spill strategy.

#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Default chunk size (the Ruby `Writer::DEFAULT_CHUNK_SIZE`).
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024 * 1024;
/// Default memory budget (the Ruby `MemoryManager::DEFAULT_MAX_MEMORY`).
pub const DEFAULT_MAX_MEMORY: usize = 256 * 1024 * 1024;
/// Flush a file writer every N chunks (the Ruby `FLUSH_THRESHOLD`).
pub const FLUSH_THRESHOLD: usize = 10;

/// What [`MemoryManager::allocate`] granted: an in-memory budget
/// reservation (the caller holds the buffer) or a disk spill file the
/// manager created and tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allocation {
    /// The reservation fits the budget; `size` bytes are accounted.
    Memory { size: usize },
    /// Over budget with [`SpillStrategy::Disk`]: the caller should
    /// write to this tracked temp file instead.
    Disk { path: PathBuf },
}

/// Overflow behavior when the budget would be exceeded (the Ruby
/// `strategy:` option).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpillStrategy {
    /// Grant a disk spill file.
    Disk,
    /// Report a memory error instead.
    Error,
}

/// Memory budget with allocate/release accounting, disk-spill
/// strategy, and tracked-temp-file cleanup (the Ruby
/// `MemoryManager`, minus the String-buffer identity map — Rust
/// callers own their buffers; only the accounting flows through
/// here).
#[derive(Debug)]
pub struct MemoryManager {
    max: usize,
    live: usize,
    temp_dir: Option<PathBuf>,
    strategy: SpillStrategy,
    spilled: Vec<PathBuf>,
    spill_seq: u64,
}

impl MemoryManager {
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            max,
            live: 0,
            temp_dir: None,
            strategy: SpillStrategy::Disk,
            spilled: Vec::new(),
            spill_seq: 0,
        }
    }

    /// Set the temp directory for spill files (the Ruby `temp_dir:`).
    #[must_use]
    pub fn temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = Some(dir.into());
        self
    }

    /// Set the overflow strategy (the Ruby `strategy:`).
    #[must_use]
    pub fn strategy(mut self, strategy: SpillStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Reserve `size` bytes, or spill to disk / report an error per
    /// the strategy (the Ruby `allocate`).
    ///
    /// # Errors
    ///
    /// `Err(MemoryError)` under [`SpillStrategy::Error`] when the
    /// budget would be exceeded; I/O errors while creating a spill
    /// file under [`SpillStrategy::Disk`].
    pub fn allocate(&mut self, size: usize) -> Result<Allocation, MemoryError> {
        if self.live.saturating_add(size) <= self.max {
            self.live += size;
            return Ok(Allocation::Memory { size });
        }
        match self.strategy {
            SpillStrategy::Disk => {
                let path = self.create_spill_file().map_err(|e| MemoryError {
                    reason: format!("spill file creation failed: {e}"),
                })?;
                Ok(Allocation::Disk { path })
            }
            SpillStrategy::Error => Err(MemoryError {
                reason: format!(
                    "Memory limit exceeded: {} > {}",
                    self.live.saturating_add(size),
                    self.max
                ),
            }),
        }
    }

    /// Return `size` reserved bytes.
    pub fn release(&mut self, size: usize) {
        self.live = self.live.saturating_sub(size);
    }

    /// Spill `data` to a new tracked temp file (the Ruby
    /// `spill_to_disk`).
    ///
    /// # Errors
    ///
    /// I/O errors while creating or writing the file.
    pub fn spill_to_disk(&mut self, data: &[u8]) -> io::Result<PathBuf> {
        let path = self.create_spill_file()?;
        fs::write(&path, data)?;
        Ok(path)
    }

    /// Delete a tracked spill file, returning its size (the Ruby
    /// `release_temp_file`). Untracked files are left alone and
    /// report 0.
    ///
    /// # Errors
    ///
    /// I/O errors while statting or removing the file.
    pub fn release_file(&mut self, path: &Path) -> io::Result<u64> {
        if let Some(i) = self.spilled.iter().position(|p| p == path) {
            self.spilled.remove(i);
            let size = fs::metadata(path)?.len();
            fs::remove_file(path)?;
            Ok(size)
        } else {
            Ok(0)
        }
    }

    /// Bytes currently reserved.
    #[must_use]
    pub fn live(&self) -> usize {
        self.live
    }

    /// Budget headroom, floored at 0 (the Ruby `available`).
    #[must_use]
    pub fn available(&self) -> usize {
        self.max.saturating_sub(self.live)
    }

    #[must_use]
    pub fn over_limit(&self) -> bool {
        self.live > self.max
    }

    /// Reserved ÷ budget, 0.0..=1.0+ (the Ruby `usage_ratio`).
    #[must_use]
    pub fn usage_ratio(&self) -> f64 {
        if self.max == 0 {
            0.0
        } else {
            self.live as f64 / self.max as f64
        }
    }

    /// Delete every tracked spill file and reset the accounting (the
    /// Ruby `cleanup`).
    pub fn cleanup(&mut self) {
        for path in self.spilled.drain(..) {
            let _ = fs::remove_file(path);
        }
        self.live = 0;
    }

    fn create_spill_file(&mut self) -> io::Result<PathBuf> {
        self.spill_seq += 1;
        let dir = self.temp_dir.clone().unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!(
            "omnizip_chunk_{}_{:x}.tmp",
            std::process::id(),
            self.spill_seq
        ));
        // Create (truncate) eagerly so the caller can write immediately.
        fs::File::create(&path)?;
        self.spilled.push(path.clone());
        Ok(path)
    }
}

/// Memory budget exceeded (the Ruby `Chunked::MemoryError`).
#[derive(Debug, Clone)]
pub struct MemoryError {
    /// Human-readable cause.
    pub reason: String,
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "memory error: {}", self.reason)
    }
}

impl std::error::Error for MemoryError {}

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

/// Incremental file writer (the Ruby `Writer`): appends chunks to an
/// output path, flushing the handle every [`FLUSH_THRESHOLD`] chunks.
pub struct ChunkedFileWriter {
    handle: Option<fs::File>,
    output_path: PathBuf,
    chunk_size: usize,
    written: u64,
    chunks_written: usize,
    chunks_since_flush: usize,
}

impl ChunkedFileWriter {
    /// Target `output_path`, chunking at `chunk_size` (0/default →
    /// [`DEFAULT_CHUNK_SIZE`]).
    #[must_use]
    pub fn new(output_path: impl Into<PathBuf>, chunk_size: usize) -> Self {
        Self {
            handle: None,
            output_path: output_path.into(),
            chunk_size: if chunk_size == 0 {
                DEFAULT_CHUNK_SIZE
            } else {
                chunk_size
            },
            written: 0,
            chunks_written: 0,
            chunks_since_flush: 0,
        }
    }

    fn ensure_open(&mut self) -> io::Result<&mut fs::File> {
        if self.handle.is_none() {
            self.handle = Some(fs::File::create(&self.output_path)?);
        }
        Ok(self.handle.as_mut().expect("just set"))
    }

    /// Append one chunk (the Ruby `write_chunk`); returns bytes
    /// accepted. Flushes the handle every [`FLUSH_THRESHOLD`] chunks.
    ///
    /// # Errors
    ///
    /// I/O errors on create/write/flush.
    pub fn write_chunk(&mut self, chunk: &[u8]) -> io::Result<u64> {
        let handle = self.ensure_open()?;
        handle.write_all(chunk)?;
        self.written += chunk.len() as u64;
        self.chunks_written += 1;
        self.chunks_since_flush += 1;
        if self.chunks_since_flush >= FLUSH_THRESHOLD {
            self.handle.as_mut().expect("open").flush()?;
            self.chunks_since_flush = 0;
        }
        Ok(chunk.len() as u64)
    }

    /// Chunks accepted so far.
    #[must_use]
    pub fn chunks_written(&self) -> usize {
        self.chunks_written
    }

    /// Bytes accepted so far (the Ruby `written`).
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Flush and drop the handle (the Ruby close).
    ///
    /// # Errors
    ///
    /// I/O errors on flush.
    pub fn close(&mut self) -> io::Result<()> {
        if let Some(mut f) = self.handle.take() {
            f.flush()?;
        }
        Ok(())
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

/// Bounded chunked reader over a file (the Ruby `Reader`): hands out
/// one `chunk_size` slice of the file at a time — never the whole
/// file — with progress/count/remaining accessors.
pub struct ChunkedReader {
    inner: Box<dyn Read>,
    chunk_size: usize,
    total: u64,
    position: u64,
}

impl ChunkedReader {
    /// Open `path` for chunked reading. `total` comes from the file
    /// metadata, like the Ruby `File.size`.
    ///
    /// # Errors
    ///
    /// I/O errors on open or stat.
    pub fn from_path(path: impl AsRef<Path>, chunk_size: usize) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let total = file.metadata()?.len();
        Ok(Self::new(file, chunk_size, total))
    }

    /// Wrap any reader; `total` (when known) powers the progress and
    /// chunk-count accessors.
    #[must_use]
    pub fn new(inner: impl Read + 'static, chunk_size: usize, total: u64) -> Self {
        Self {
            inner: Box::new(inner),
            chunk_size: chunk_size.max(1),
            total,
            position: 0,
        }
    }

    /// Next chunk, or `None` at EOF (the Ruby `read_chunk`).
    ///
    /// # Errors
    ///
    /// I/O errors on read.
    pub fn next_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.position >= self.total {
            return Ok(None);
        }
        let want = self.chunk_size.min((self.total - self.position) as usize);
        let mut chunk = Vec::with_capacity(want);
        (&mut self.inner)
            .take(want as u64)
            .read_to_end(&mut chunk)?;
        if chunk.is_empty() {
            return Ok(None);
        }
        self.position += chunk.len() as u64;
        Ok(Some(chunk))
    }

    /// Fraction consumed, 0.0..=1.0 (the Ruby `progress`).
    #[must_use]
    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.position as f64 / self.total as f64
        }
    }

    /// True when everything has been read (the Ruby `eof?`).
    #[must_use]
    pub fn eof(&self) -> bool {
        self.position >= self.total
    }

    /// Chunks the file divides into (the Ruby `chunk_count`).
    #[must_use]
    pub fn chunk_count(&self) -> u64 {
        self.total.div_ceil(self.chunk_size as u64)
    }

    /// Bytes not yet read (the Ruby `remaining`).
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.total - self.position.min(self.total)
    }
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
        let dir = std::env::temp_dir().join(format!("ozip-mm2-{}", std::process::id()));
        // Default strategy is Disk: over-budget allocations spill.
        let mut m = MemoryManager::new(100).temp_dir(&dir);
        assert!(matches!(
            m.allocate(60).unwrap(),
            Allocation::Memory { size: 60 }
        ));
        m.allocate(40).unwrap();
        let spill = match m.allocate(1).unwrap() {
            Allocation::Disk { path } => path,
            Allocation::Memory { .. } => panic!("over budget must spill"),
        };
        assert_eq!(m.live(), 100); // the spill is not in-memory accounting
        m.release(40);
        m.allocate(1).unwrap();
        assert_eq!(m.live(), 61);
        assert!(!m.over_limit());
        m.cleanup();
        assert!(!spill.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_error_strategy_reports() {
        let mut m = MemoryManager::new(100).strategy(SpillStrategy::Error);
        m.allocate(100).unwrap();
        let err = m.allocate(1).unwrap_err();
        assert!(err.reason.contains("Memory limit exceeded"));
        assert_eq!(m.live(), 100); // nothing reserved on error
    }

    #[test]
    fn disk_spill_tracks_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("ozip-mm-{}", std::process::id()));
        let mut m = MemoryManager::new(16)
            .temp_dir(&dir)
            .strategy(SpillStrategy::Disk);
        let spill = match m.allocate(64).unwrap() {
            Allocation::Disk { path } => path,
            Allocation::Memory { .. } => panic!("over-budget allocation must spill"),
        };
        std::fs::write(&spill, b"spilled payload").unwrap();
        assert_eq!(m.spill_to_disk(b"second").unwrap().exists(), true);
        assert_eq!(m.release_file(&spill).unwrap(), 15);
        assert!(!spill.exists());
        m.cleanup(); // removes the remaining tracked file
        assert!(m.available() == 16 && m.live() == 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunked_reader_bounded_and_accurate() {
        let dir = std::env::temp_dir().join(format!("ozip-cr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blob.bin");
        let input: Vec<u8> = (0..5000u32).map(|i| (i % 13) as u8).collect();
        std::fs::write(&path, &input).unwrap();

        let mut r = ChunkedReader::from_path(&path, 1024).unwrap();
        assert_eq!(r.chunk_count(), 5);
        let mut got = Vec::new();
        let mut chunks = 0;
        while let Some(chunk) = r.next_chunk().unwrap() {
            got.extend_from_slice(&chunk);
            chunks += 1;
            assert!(r.progress() > 0.0);
        }
        assert_eq!(chunks, 5);
        assert!(r.eof());
        assert_eq!(r.remaining(), 0);
        assert_eq!(got, input);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunked_file_writer_appends_and_flushes() {
        let dir = std::env::temp_dir().join(format!("ozip-cw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.bin");
        let mut w = ChunkedFileWriter::new(&path, 0);
        let mut total = 0u64;
        for i in 0..25u32 {
            let chunk = vec![i as u8; 100];
            total += w.write_chunk(&chunk).unwrap();
        }
        w.close().unwrap();
        assert_eq!((w.chunks_written(), w.written()), (25, total));
        assert_eq!(std::fs::read(&path).unwrap().len() as u64, total);
        let _ = std::fs::remove_dir_all(&dir);
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
