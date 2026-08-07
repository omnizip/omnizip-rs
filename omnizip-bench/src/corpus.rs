//! Corpus model + downloader/cache.
//!
//! Corpora are downloaded once into a per-user cache directory and
//! reused on subsequent runs. Synthetic corpora ([`crate::synthetic`])
//! bypass the cache entirely.
//!
//! ## Cache layout
//!
//! ```text
//! ~/.cache/omnizip-bench/
//! ├── calgary/
//! │   ├── bib
//! │   ├── book1
//! │   └── ...
//! └── silesia/
//!     ├── dickens
//!     └── ...
//! ```
//!
//! ## Adding a corpus
//!
//! Add one entry to [`known_corpora`]. No runner or reporter edits.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A named collection of byte files to benchmark against.
#[derive(Debug, Clone)]
pub struct Corpus {
    name: String,
    files: Vec<CorpusFile>,
}

impl Corpus {
    #[must_use]
    pub fn new(name: impl Into<String>, files: Vec<CorpusFile>) -> Self {
        Self {
            name: name.into(),
            files,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn files(&self) -> &[CorpusFile] {
        &self.files
    }

    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::try_from(f.content().len()).unwrap_or(0))
            .sum()
    }
}

/// One file within a [`Corpus`]. Always loaded eagerly — benchmark
/// cases iterate the content multiple times.
#[derive(Debug, Clone)]
pub struct CorpusFile {
    name: String,
    bytes: Vec<u8>,
}

impl CorpusFile {
    #[must_use]
    pub fn in_memory(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            bytes,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn content(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        u64::try_from(self.bytes.len()).unwrap_or(0)
    }
}

/// Specification of a downloadable corpus — used by the cache loader.
#[derive(Debug, Clone)]
pub struct CorpusSpec {
    /// Short identifier used on the CLI (`--corpus <name>`).
    pub name: &'static str,
    /// Human-readable description for `--list-corpora`.
    pub description: &'static str,
    /// Approximate uncompressed total size, bytes (for progress display).
    pub approx_size: u64,
    /// Download URL — must be a `.zip`.
    pub url: &'static str,
    /// Files inside the zip to include. Matched by suffix, so a zip
    /// with files under a `corpus/` subdir still resolves.
    pub files: &'static [&'static str],
}

/// All corpora the benchmark knows how to download.
///
/// Adding a corpus = one entry here. The cache loader, runner, and
/// reporters never change (open/closed).
#[must_use]
pub fn known_corpora() -> &'static [CorpusSpec] {
    &[
        CorpusSpec {
            name: "calgary",
            description: "Calgary compression corpus (classic, ~3 MB)",
            approx_size: 3_000_000,
            url: "http://corpus.canterbury.ac.nz/resources/cantrbry.zip",
            files: &[
                "bib", "book1", "book2", "geo", "news", "obj1", "obj2", "paper1", "paper2",
                "paper3", "paper4", "paper5", "paper6", "pic", "progc", "progl", "progp", "trans",
            ],
        },
        CorpusSpec {
            name: "canterbury",
            description: "Canterbury corpus (updated Calgary, ~3 MB)",
            approx_size: 3_000_000,
            url: "http://corpus.canterbury.ac.nz/resources/cantrbry.zip",
            files: &[
                "grammar.lsp",
                "xargs.1",
                "fields.c",
                "cp.html",
                "grammar.lsp",
                "lecture.txt",
                "lctet10.txt",
                "plrabn12.txt",
                "ptt5",
                "sum",
                "kennedy.xls",
                "sep9811.txt",
            ],
        },
        CorpusSpec {
            name: "silesia",
            description: "Silesia compression corpus (~200 MB)",
            approx_size: 200_000_000,
            url: "https://sun.aei.polsl.pl/~sdeor/corpus/silesia.zip",
            files: &[
                "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao",
                "webster", "xml", "x-ray",
            ],
        },
        CorpusSpec {
            name: "enwik8",
            description: "Enwik8 — first 100 MB of English Wikipedia XML",
            approx_size: 100_000_000,
            url: "http://mattmahoney.net/dc/enwik8.zip",
            files: &["enwik8"],
        },
        // NOTE: AIT 2026 corpus (TODO 90) is exposed as a synthetic
        // `ait-mix` in `synthetic.rs` — see TODO 90 for context. Once
        // the official zip is released, replace this spec with the
        // real URL.
    ]
}

/// Errors that can occur during corpus loading.
#[derive(Debug)]
pub enum CorpusError {
    /// `--corpus <name>` did not match anything in [`known_corpora`].
    UnknownCorpus { name: String },
    /// Cache directory could not be created or written.
    CacheIo(String),
    /// Download failed (network, HTTP status, etc.).
    Download { url: String, reason: String },
    /// Zip was downloaded but the expected file was missing inside.
    MissingFileInZip { file: String },
}

impl std::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCorpus { name } => {
                let names: Vec<&str> = known_corpora().iter().map(|c| c.name).collect();
                write!(f, "unknown corpus '{name}' (known: {})", names.join(", "))
            }
            Self::CacheIo(s) => write!(f, "cache I/O error: {s}"),
            Self::Download { url, reason } => write!(f, "download {url} failed: {reason}"),
            Self::MissingFileInZip { file } => {
                write!(f, "expected file '{file}' not found inside zip")
            }
        }
    }
}

impl std::error::Error for CorpusError {}

/// Resolve `name` to a fully-loaded [`Corpus`], downloading if needed.
///
/// # Errors
///
/// See [`CorpusError`].
pub fn load(name: &str) -> Result<Corpus, CorpusError> {
    let spec = known_corpora()
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| CorpusError::UnknownCorpus {
            name: name.to_string(),
        })?;

    let cache = cache_dir_for(name);
    if !cache_is_populated(&cache, spec) {
        download_and_extract(spec, &cache)?;
    }

    let mut files = Vec::with_capacity(spec.files.len());
    for fname in spec.files {
        let path = find_cached_file(&cache, fname)?;
        let bytes = fs::read(&path)
            .map_err(|e| CorpusError::CacheIo(format!("read {}: {e}", path.display())))?;
        files.push(CorpusFile::in_memory((*fname).to_string(), bytes));
    }
    Ok(Corpus::new(spec.name, files))
}

fn cache_dir_for(corpus_name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIZIP_BENCH_CACHE") {
        return PathBuf::from(dir).join(corpus_name);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("omnizip-bench")
            .join(corpus_name);
    }
    PathBuf::from(".omnizip-bench-cache").join(corpus_name)
}

fn cache_is_populated(dir: &Path, spec: &CorpusSpec) -> bool {
    if !dir.is_dir() {
        return false;
    }
    spec.files.iter().all(|f| find_cached_file(dir, f).is_ok())
}

fn find_cached_file(dir: &Path, name: &str) -> Result<PathBuf, CorpusError> {
    // Files may be at the top level of the cache dir or under a single
    // subdirectory (depending on the zip layout).
    let direct = dir.join(name);
    if direct.is_file() {
        return Ok(direct);
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let candidate = p.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(CorpusError::CacheIo(format!(
        "file '{name}' missing under {}",
        dir.display()
    )))
}

fn download_and_extract(spec: &CorpusSpec, dest: &Path) -> Result<(), CorpusError> {
    fs::create_dir_all(dest)
        .map_err(|e| CorpusError::CacheIo(format!("mkdir {}: {e}", dest.display())))?;

    eprintln!(
        "[bench] downloading {} (~{} MB)...",
        spec.url,
        spec.approx_size / 1_000_000
    );
    let zip_bytes = download(spec.url)?;
    eprintln!("[bench] extracting {} bytes...", zip_bytes.len());
    extract_zip(&zip_bytes, dest, spec)?;
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>, CorpusError> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(600))
        .call()
        .map_err(|e| CorpusError::Download {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| CorpusError::Download {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
    Ok(buf)
}

fn extract_zip(bytes: &[u8], dest: &Path, spec: &CorpusSpec) -> Result<(), CorpusError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| CorpusError::Download {
        url: spec.url.to_string(),
        reason: format!("zip parse: {e}"),
    })?;

    let mut wanted: Vec<&str> = spec.files.to_vec();
    for idx in 0..archive.len() {
        let entry = archive.by_index(idx).map_err(|e| CorpusError::Download {
            url: spec.url.to_string(),
            reason: format!("zip entry {idx}: {e}"),
        })?;
        let entry_name = entry.name().to_string();
        // Match by suffix so a leading directory is tolerated.
        if let Some(pos) = wanted.iter().position(|w| entry_name.ends_with(w)) {
            let matched = wanted.remove(pos);
            if entry.is_dir() {
                continue;
            }
            let out_path = dest.join(matched);
            let mut content = Vec::new();
            let mut entry = entry;
            entry
                .read_to_end(&mut content)
                .map_err(|e| CorpusError::Download {
                    url: spec.url.to_string(),
                    reason: format!("read {}: {e}", entry_name),
                })?;
            let mut file = fs::File::create(&out_path)
                .map_err(|e| CorpusError::CacheIo(format!("create {}: {e}", out_path.display())))?;
            file.write_all(&content)
                .map_err(|e| CorpusError::CacheIo(format!("write {}: {e}", out_path.display())))?;
        }
    }
    if !wanted.is_empty() {
        return Err(CorpusError::MissingFileInZip {
            file: wanted.join(", "),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_corpora_contains_classics() {
        let names: Vec<&str> = known_corpora().iter().map(|c| c.name).collect();
        assert!(names.contains(&"calgary"));
        assert!(names.contains(&"silesia"));
        assert!(names.contains(&"enwik8"));
    }

    #[test]
    fn unknown_corpus_errors() {
        let err = load("does-not-exist").unwrap_err();
        assert!(matches!(err, CorpusError::UnknownCorpus { .. }));
    }

    #[test]
    fn corpus_total_size_sums_files() {
        let c = Corpus::new(
            "test",
            vec![
                CorpusFile::in_memory("a", vec![0; 100]),
                CorpusFile::in_memory("b", vec![0; 200]),
            ],
        );
        assert_eq!(c.total_size(), 300);
    }
}
