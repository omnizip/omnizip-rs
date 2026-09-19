//! Container commands (TODO.containers task 15): `ozip c/x/t/l` over
//! the shipped format crates — tar (+ gzip/bzip2/xz/zstd wrappers),
//! zip, cpio — with format inference by extension or magic, the
//! shared extraction security boundary, and deterministic creation
//! on by default (task 17).
#![forbid(unsafe_code)]

use omnizip_archive_core::detect::{detect_format, FormatKind};
use omnizip_archive_core::security::SecurityPolicy;
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveEntry, ArchiveReader, ArchiveWriter, EntryKind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Single-file codec specification (the `ozip -d` / `-N` codec mode
/// AND the single-file archive view below share this table — SSOT).
pub(crate) struct CodecSpec {
    pub(crate) name: &'static str,
    pub(crate) suffix: &'static str,
    /// (input, level) -> compressed bytes
    pub(crate) compress: fn(&[u8], u8) -> Result<Vec<u8>, String>,
    /// compressed -> plaintext
    pub(crate) decompress: fn(&[u8]) -> Result<Vec<u8>, String>,
    pub(crate) default_level: u8,
    pub(crate) max_level: u8,
}

pub(crate) fn specs() -> Vec<CodecSpec> {
    vec![
        CodecSpec {
            name: "xz",
            suffix: ".xz",
            compress: |data, lvl| {
                omnizip_lzma::xz_compress_with_options(
                    data,
                    &omnizip_lzma::LzmaOptions {
                        max_chain_length: lvl_factor(lvl),
                        nice_match: u32::from(lvl.min(9)) * 30,
                        ..omnizip_lzma::LzmaOptions::default()
                    },
                )
                .map_err(|e| e.to_string())
            },
            decompress: |data| omnizip_lzma::xz_decompress(data).map_err(|e| e.to_string()),
            default_level: 6,
            max_level: 9,
        },
        CodecSpec {
            name: "zstd",
            suffix: ".zst",
            compress: |data, lvl| {
                omnizip_zstd::compress(data, zstd_level(lvl)).map_err(|e| e.to_string())
            },
            decompress: |data| omnizip_zstd::decompress(data, u32::MAX).map_err(|e| e.to_string()),
            default_level: 6,
            max_level: 22,
        },
        CodecSpec {
            name: "gzip",
            suffix: ".gz",
            compress: |data, _| {
                omnizip_archive_core::formats::gzip::compress(
                    data,
                    &omnizip_archive_core::formats::gzip::GzipOptions::default(),
                )
                .map_err(|e| e.to_string())
            },
            decompress: |data| {
                omnizip_archive_core::formats::gzip::decompress(data).map_err(|e| e.to_string())
            },
            default_level: 6,
            max_level: 9,
        },
        CodecSpec {
            name: "bzip2",
            suffix: ".bz2",
            compress: |data, lvl| {
                omnizip_archive_core::formats::bzip2_file::compress(data, lvl.max(1))
                    .map_err(|e| e.to_string())
            },
            decompress: |data| {
                omnizip_archive_core::formats::bzip2_file::decompress(data)
                    .map_err(|e| e.to_string())
            },
            default_level: 9,
            max_level: 9,
        },
        CodecSpec {
            name: "lzip",
            suffix: ".lz",
            compress: |data, _| {
                omnizip_archive_core::formats::lzip::compress(
                    data,
                    &omnizip_archive_core::formats::lzip::LzipOptions::default(),
                )
                .map_err(|e| e.to_string())
            },
            decompress: |data| {
                omnizip_archive_core::formats::lzip::decompress(data).map_err(|e| e.to_string())
            },
            default_level: 6,
            max_level: 9,
        },
        CodecSpec {
            name: "lzma",
            suffix: ".lzma",
            compress: |data, _| {
                omnizip_archive_core::formats::lzma_alone::compress(data).map_err(|e| e.to_string())
            },
            decompress: |data| {
                omnizip_archive_core::formats::lzma_alone::decompress(data)
                    .map_err(|e| e.to_string())
            },
            default_level: 6,
            max_level: 9,
        },
    ]
}

pub(crate) fn zstd_level(lvl: u8) -> omnizip_zstd::ZstdLevel {
    match lvl {
        0..=2 => omnizip_zstd::ZstdLevel::Fastest,
        3..=5 => omnizip_zstd::ZstdLevel::Fast,
        6..=11 => omnizip_zstd::ZstdLevel::Default,
        12..=21 => omnizip_zstd::ZstdLevel::Better,
        _ => omnizip_zstd::ZstdLevel::Best,
    }
}

fn lvl_factor(lvl: u8) -> u32 {
    match lvl {
        1 => 4,
        2 => 8,
        3 => 24,
        4 => 24,
        5 => 32,
        _ => 48,
    }
}

/// Strip a codec suffix for the single-file archive view's entry
/// name (`notes.txt.gz` -> `notes.txt`, `archive.txz` ->
/// `archive.tar`); unknown suffixes get `.out`.
pub(crate) fn strip_codec_suffix(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return stem(name, ".tar.gz", ".tgz");
    }
    if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
        return stem(name, ".tar.bz2", ".tbz2");
    }
    if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        return stem(name, ".tar.xz", ".txz");
    }
    if lower.ends_with(".tar.zst") {
        return stem(name, ".tar.zst", ".tar.zst");
    }
    for suffix in [".gz", ".bz2", ".xz", ".zst", ".lz", ".lzma"] {
        if lower.len() > suffix.len() && lower.ends_with(suffix) {
            return name[..name.len() - suffix.len()].to_string();
        }
    }
    format!("{name}.out")
}

fn stem(name: &str, a: &str, b: &str) -> String {
    if name.len() >= a.len() && name.to_lowercase().ends_with(a) {
        format!("{}tar", &name[..name.len() - a.len()])
    } else {
        format!("{}tar", &name[..name.len() - b.len()])
    }
}

/// A registered archive format: how to identify it and its read/write
/// capability. Adding a format is one row — command code never grows
/// a format branch.
struct FormatSpec {
    name: &'static str,
    extensions: &'static [&'static str],
    write: bool,
}

const FORMATS: &[FormatSpec] = &[
    FormatSpec {
        name: "tar",
        extensions: &["tar"],
        write: true,
    },
    FormatSpec {
        name: "tar.gz",
        extensions: &["tar.gz", "tgz"],
        write: true,
    },
    FormatSpec {
        name: "tar.bz2",
        extensions: &["tar.bz2", "tbz2"],
        write: true,
    },
    FormatSpec {
        name: "tar.xz",
        extensions: &["tar.xz", "txz"],
        write: true,
    },
    FormatSpec {
        name: "tar.zst",
        extensions: &["tar.zst"],
        write: true,
    },
    FormatSpec {
        name: "zip",
        extensions: &["zip"],
        write: true,
    },
    FormatSpec {
        name: "cpio",
        extensions: &["cpio"],
        write: true,
    },
    FormatSpec {
        name: "7z",
        extensions: &["7z"],
        write: true,
    },
    FormatSpec {
        name: "rpm",
        extensions: &["rpm"],
        write: true,
    },
    FormatSpec {
        name: "rar5",
        extensions: &["rar"],
        write: true,
    },
    FormatSpec {
        name: "rar4",
        extensions: &[],
        write: false,
    },
    FormatSpec {
        name: "iso",
        extensions: &["iso"],
        write: true,
    },
    FormatSpec {
        name: "xar",
        extensions: &["xar", "pkg"],
        write: false,
    },
];

/// Print the registered-format table (`ozip --formats`).
pub fn print_formats() {
    println!("{:<10} {:<28} MODE", "FORMAT", "EXTENSIONS");
    for f in FORMATS {
        println!(
            "{:<10} {:<28} {}",
            f.name,
            f.extensions.join(", "),
            if f.write { "rw" } else { "read" }
        );
    }
}

/// The container the user asked to create, by extension or `-f`.
enum OutputFormat {
    Tar,
    Gzip,
    Bzip2,
    Xz,
    Zstd,
    Lzip,
    LzmaAlone,
    TarGzip,
    TarBzip2,
    TarXz,
    TarZstd,
    Zip,
    Cpio,
    SevenZip,
    Rpm,
    Iso,
    Rar5,
}

fn infer_output(archive: &Path, explicit: Option<&str>) -> Result<OutputFormat, String> {
    if let Some(name) = explicit {
        return match name {
            "tar" => Ok(OutputFormat::Tar),
            "gzip" | "gz" => Ok(OutputFormat::Gzip),
            "bzip2" | "bz2" => Ok(OutputFormat::Bzip2),
            "xz" => Ok(OutputFormat::Xz),
            "zstd" | "zst" => Ok(OutputFormat::Zstd),
            "lzip" | "lz" => Ok(OutputFormat::Lzip),
            "lzma" | "lzma-alone" => Ok(OutputFormat::LzmaAlone),
            "tar.gz" | "tgz" => Ok(OutputFormat::TarGzip),
            "tar.bz2" | "tbz2" => Ok(OutputFormat::TarBzip2),
            "tar.xz" | "txz" => Ok(OutputFormat::TarXz),
            "tar.zst" => Ok(OutputFormat::TarZstd),
            "zip" => Ok(OutputFormat::Zip),
            "cpio" => Ok(OutputFormat::Cpio),
            "7z" => Ok(OutputFormat::SevenZip),
            "rpm" => Ok(OutputFormat::Rpm),
            "iso" => Ok(OutputFormat::Iso),
            "rar" | "rar5" => Ok(OutputFormat::Rar5),
            other => Err(format!(
                "unknown format '{other}' (registered: {})",
                FORMATS
                    .iter()
                    .map(|f| f.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    for candidate in [
        "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "tgz", "tbz2", "txz", "tar", "zip", "cpio", "7z",
        "rpm", "iso", "rar", "gzip", "gz", "bzip2", "bz2", "xz", "zstd", "zst", "lzip", "lz",
        "lzma",
    ] {
        if name.ends_with(candidate) {
            return match candidate {
                "tar.gz" | "tgz" => Ok(OutputFormat::TarGzip),
                "tar.bz2" | "tbz2" => Ok(OutputFormat::TarBzip2),
                "tar.xz" | "txz" => Ok(OutputFormat::TarXz),
                "tar.zst" => Ok(OutputFormat::TarZstd),
                "tar" => Ok(OutputFormat::Tar),
                "gzip" | "gz" => Ok(OutputFormat::Gzip),
                "bzip2" | "bz2" => Ok(OutputFormat::Bzip2),
                "xz" => Ok(OutputFormat::Xz),
                "zstd" | "zst" => Ok(OutputFormat::Zstd),
                "lzip" | "lz" => Ok(OutputFormat::Lzip),
                "lzma" => Ok(OutputFormat::LzmaAlone),
                "zip" => Ok(OutputFormat::Zip),
                "cpio" => Ok(OutputFormat::Cpio),
                "7z" => Ok(OutputFormat::SevenZip),
                "rpm" => Ok(OutputFormat::Rpm),
                "iso" => Ok(OutputFormat::Iso),
                "rar" => Ok(OutputFormat::Rar5),
                _ => unreachable!(),
            };
        }
    }
    Err(format!(
        "cannot infer format from '{}'; use -f <format>",
        archive.display()
    ))
}

/// One walked input: metadata + (for files) content, in lexicographic
/// path order (the task-17 rule: never readdir order).
struct Staged {
    entry: omnizip_archive_core::NewEntry,
    data: Vec<u8>,
}

/// Walk `inputs` (files, dirs, symlinks) deterministically.
fn stage(inputs: &[PathBuf], options: &WriteOptions) -> Result<Vec<Staged>, String> {
    let mut out = Vec::new();
    for input in inputs {
        let meta =
            std::fs::symlink_metadata(input).map_err(|e| format!("{}: {e}", input.display()))?;
        let base = input
            .file_name()
            .ok_or_else(|| format!("{}: cannot archive the filesystem root", input.display()))?
            .to_string_lossy()
            .into_owned();
        if meta.is_dir() {
            walk_dir(input, &base, options, &mut out)?;
        } else if meta.file_type().is_symlink() {
            let target = std::fs::read_link(input)
                .map_err(|e| format!("{}: {e}", input.display()))?
                .to_string_lossy()
                .into_owned();
            out.push(Staged {
                entry: omnizip_archive_core::NewEntry::symlink(base, target, options),
                data: Vec::new(),
            });
        } else {
            let data = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
            out.push(Staged {
                entry: omnizip_archive_core::NewEntry::file(base, options),
                data,
            });
        }
    }
    Ok(out)
}

fn walk_dir(
    dir: &Path,
    prefix: &str,
    options: &WriteOptions,
    out: &mut Vec<Staged>,
) -> Result<(), String> {
    out.push(Staged {
        entry: omnizip_archive_core::NewEntry::directory(prefix, options),
        data: Vec::new(),
    });
    // BTreeMap = lexicographic child order, independent of readdir.
    let mut children: BTreeMap<String, PathBuf> = BTreeMap::new();
    for child in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let child = child.map_err(|e| format!("{}: {e}", dir.display()))?;
        children.insert(
            child.file_name().to_string_lossy().into_owned(),
            child.path(),
        );
    }
    for (name, path) in children {
        let rel = format!("{prefix}/{name}");
        let meta =
            std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_dir() {
            walk_dir(&path, &rel, options, out)?;
        } else if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .to_string_lossy()
                .into_owned();
            out.push(Staged {
                entry: omnizip_archive_core::NewEntry::symlink(rel, target, options),
                data: Vec::new(),
            });
        } else {
            let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push(Staged {
                entry: omnizip_archive_core::NewEntry::file(rel, options),
                data,
            });
        }
    }
    Ok(())
}

/// `ozip c ARCHIVE INPUTS...` — create a deterministic archive.
/// `-p` (encryption) and `--volume` (multi-volume split) apply to 7z.
pub fn create(
    archive: &Path,
    inputs: &[PathBuf],
    format: Option<&str>,
    level: Option<u8>,
    password: Option<&str>,
    volume: Option<usize>,
) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("create needs at least one input file or directory".into());
    }
    let output = infer_output(archive, format)?;
    if (password.is_some() || volume.is_some()) && !matches!(output, OutputFormat::SevenZip) {
        return Err("-p/--volume are only supported for 7z output".into());
    }
    let level = level.unwrap_or(6);
    let options = WriteOptions::deterministic();

    // Single-file formats compress the FILE itself (Ruby
    // compress_command semantics): one regular-file input only.
    let single = match output {
        OutputFormat::Gzip => Some("gzip"),
        OutputFormat::Bzip2 => Some("bzip2"),
        OutputFormat::Xz => Some("xz"),
        OutputFormat::Zstd => Some("zstd"),
        OutputFormat::Lzip => Some("lzip"),
        OutputFormat::LzmaAlone => Some("lzma"),
        _ => None,
    };
    if let Some(codec_name) = single {
        if inputs.len() != 1 || !inputs[0].is_file() {
            return Err(format!(
                "format '{codec_name}' compresses a single file; \
                 give one file input or use the tar.* container format"
            ));
        }
        let spec = specs()
            .into_iter()
            .find(|s| s.name == codec_name)
            .ok_or_else(|| format!("codec '{codec_name}' missing from the spec table"))?;
        let data =
            std::fs::read(&inputs[0]).map_err(|e| format!("{}: {e}", inputs[0].display()))?;
        let clamped = level.min(spec.max_level);
        let compressed = (spec.compress)(&data, clamped)?;
        std::fs::write(archive, &compressed).map_err(|e| format!("{}: {e}", archive.display()))?;
        return Ok(());
    }

    let staged = stage(inputs, &options)?;

    let bytes = match output {
        OutputFormat::Gzip
        | OutputFormat::Bzip2
        | OutputFormat::Xz
        | OutputFormat::Zstd
        | OutputFormat::Lzip
        | OutputFormat::LzmaAlone => {
            unreachable!("single-file formats return in the branch above")
        }
        OutputFormat::Tar => {
            let mut w = omnizip_tar::TarWriter::new();
            write_all(&mut w, &staged, &options)?;
            w.finish_bytes().map_err(|e| e.to_string())?
        }
        OutputFormat::TarGzip => tar_then(
            &staged,
            &options,
            &|tar| {
                omnizip_archive_core::formats::gzip::compress(
                    tar,
                    &omnizip_archive_core::formats::gzip::GzipOptions::default(),
                )
                .map_err(|e| e.to_string())
            },
            "gzip",
        )?,
        OutputFormat::TarBzip2 => tar_then(
            &staged,
            &options,
            &|tar| {
                omnizip_archive_core::formats::bzip2_file::compress(tar, level.max(1))
                    .map_err(|e| e.to_string())
            },
            "bzip2",
        )?,
        OutputFormat::TarXz => tar_then(
            &staged,
            &options,
            &|tar| omnizip_lzma::xz_compress(tar).map_err(|e| e.to_string()),
            "xz",
        )?,
        OutputFormat::TarZstd => tar_then(
            &staged,
            &options,
            &|tar| omnizip_zstd::compress(tar, zstd_level(level)).map_err(|e| e.to_string()),
            "zstd",
        )?,
        OutputFormat::Zip => {
            let mut w = omnizip_zip::ZipWriter::new().with_method(if level == 0 {
                omnizip_zip::ZipMethod::Store
            } else {
                omnizip_zip::ZipMethod::Deflate
            });
            write_all(&mut w, &staged, &options)?;
            w.finish_bytes().map_err(|e| e.to_string())?
        }
        OutputFormat::Cpio => {
            let mut w = omnizip_cpio::CpioWriter::new().with_format(omnizip_cpio::CpioFormat::Newc);
            write_all(&mut w, &staged, &options)?;
            w.finish_bytes().map_err(|e| e.to_string())?
        }
        OutputFormat::SevenZip => {
            // Solid by default; level 0 stores, everything else LZMA2.
            let method = if level == 0 {
                omnizip_sevenzip::writer::SevenZipMethod::Copy
            } else {
                omnizip_sevenzip::writer::SevenZipMethod::Lzma2
            };
            let mut w = omnizip_sevenzip::writer::SevenZipWriter::new(method).with_solid(true);
            if let Some(pw) = password {
                w = w.with_password(pw);
            }
            write_all(&mut w, &staged, &options)?;
            if let Some(volume_size) = volume {
                let parts = w
                    .finish_volumes(&options, volume_size)
                    .map_err(|e| e.to_string())?;
                for (i, part) in parts.iter().enumerate() {
                    let name = format!("{}.{:03}", archive.display(), i + 1);
                    std::fs::write(&name, part).map_err(|e| format!("{name}: {e}"))?;
                }
                return Ok(());
            }
            w.finish_bytes(&options).map_err(|e| e.to_string())?
        }
        OutputFormat::Rpm => {
            let mut w = omnizip_rpm::writer::RpmWriter::new("archive", "1.0.0", "1");
            write_all(&mut w, &staged, &options)?;
            w.finish_bytes(&options).map_err(|e| e.to_string())?
        }
        OutputFormat::Iso => {
            let mut w = omnizip_iso::writer::IsoWriter::new("OZIPVOL");
            write_all(&mut w, &staged, &options)?;
            w.finish_bytes(&options).map_err(|e| e.to_string())?
        }
        OutputFormat::Rar5 => {
            let mut w = omnizip_rar::rar5::Rar5Writer::new();
            for s in &staged {
                match s.entry.kind {
                    EntryKind::Symlink(_) => {
                        return Err("rar5: symlink writing not supported".into())
                    }
                    EntryKind::Directory => w
                        .add_directory(&s.entry, &options)
                        .map_err(|e| e.to_string())?,
                    _ => w
                        .add_file(&s.entry, &s.data, &options)
                        .map_err(|e| e.to_string())?,
                }
            }
            w.finish_bytes(&options).map_err(|e| e.to_string())?
        }
    };

    std::fs::write(archive, &bytes).map_err(|e| format!("{}: {e}", archive.display()))
}

fn write_all<W: omnizip_archive_core::ArchiveWriter>(
    writer: &mut W,
    staged: &[Staged],
    options: &WriteOptions,
) -> Result<(), String> {
    for s in staged {
        match s.entry.kind {
            EntryKind::Directory => writer
                .add_directory(&s.entry, options)
                .map_err(|e| e.to_string())?,
            EntryKind::Symlink(_) => writer
                .add_symlink(&s.entry, options)
                .map_err(|e| e.to_string())?,
            _ => writer
                .add_file(&s.entry, &s.data, options)
                .map_err(|e| e.to_string())?,
        }
    }
    Ok(())
}

type TarCodec<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;

fn tar_then(
    staged: &[Staged],
    options: &WriteOptions,
    codec: &TarCodec<'_>,
    name: &str,
) -> Result<Vec<u8>, String> {
    let mut w = omnizip_tar::TarWriter::new();
    write_all(&mut w, staged, options)?;
    let tar = w.finish_bytes().map_err(|e| e.to_string())?;
    codec(&tar).map_err(|e| format!("{name}: {e}"))
}

/// An opened archive, ready to list or extract.
enum Opened {
    /// A single-file codec payload viewed as a one-entry archive
    /// (task 53: the Ruby format-handler semantics).
    SingleFile {
        name: String,
        data: Vec<u8>,
    },
    Tar(Box<omnizip_tar::TarReader>),
    Zip(Box<omnizip_zip::ZipReader>),
    Cpio(Box<omnizip_cpio::CpioReader>),
    SevenZip(Box<omnizip_sevenzip::reader::SevenZipReader>),
    Rpm(Box<omnizip_rpm::reader::RpmReader>),
    Iso(Box<omnizip_iso::reader::IsoReader>),
    Rar5(Box<omnizip_rar::rar5::Rar5Reader>),
    Rar4(Box<omnizip_rar::rar3::Rar4Reader>),
}

impl Opened {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, String> {
        match self {
            Self::SingleFile { name, data } => {
                Ok(vec![ArchiveEntry::file(name.clone(), data.len() as u64)])
            }
            Self::Tar(r) => r.entries().map_err(|e| e.to_string()),
            Self::Zip(r) => r.entries().map_err(|e| e.to_string()),
            Self::Cpio(r) => r.entries().map_err(|e| e.to_string()),
            Self::SevenZip(r) => r.entries().map_err(|e| e.to_string()),
            Self::Rpm(r) => r.entries().map_err(|e| e.to_string()),
            Self::Iso(r) => r.entries().map_err(|e| e.to_string()),
            Self::Rar5(r) => r.entries().map_err(|e| e.to_string()),
            Self::Rar4(r) => r.entries().map_err(|e| e.to_string()),
        }
    }

    /// Read one entry's bytes (the ArchiveReader seam, plus the
    /// single-file view).
    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, String> {
        match self {
            Self::SingleFile { data, .. } => {
                if index == 0 {
                    Ok(data.clone())
                } else {
                    Err(format!("entry index {index} out of range"))
                }
            }
            Self::Tar(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::Zip(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::Cpio(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::SevenZip(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::Rpm(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::Iso(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::Rar5(r) => r.read_entry(index).map_err(|e| e.to_string()),
            Self::Rar4(r) => r.read_entry(index).map_err(|e| e.to_string()),
        }
    }

    fn extract_to(&mut self, dir: &Path) -> Result<(), String> {
        let policy = SecurityPolicy::default();
        match self {
            Self::SingleFile { name, data } => {
                let target = dir.join(Path::new(name).file_name().ok_or_else(|| {
                    format!("single-file entry name has no file component: {name}")
                })?);
                std::fs::write(&target, data).map_err(|e| format!("{}: {e}", target.display()))?;
                Ok(())
            }
            Self::Tar(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::Zip(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::Cpio(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::SevenZip(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::Rpm(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::Iso(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::Rar5(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
            Self::Rar4(r) => r.extract_to(dir, &policy).map_err(|e| e.to_string()),
        }
    }
}

fn open_archive(archive: &Path, password: Option<&str>) -> Result<Opened, String> {
    let name = archive
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    if name.len() > 4 && name.ends_with(".001") {
        // Multi-volume split: concatenate .001/.002/... parts in order.
        let base = name[..name.len() - 3].to_string();
        let dir = archive.parent().unwrap_or_else(|| Path::new("."));
        let mut data = std::fs::read(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
        for part in 2.. {
            let path = dir.join(format!("{base}{part:03}"));
            let Ok(bytes) = std::fs::read(&path) else {
                break;
            };
            data.extend_from_slice(&bytes);
        }
        return open_bytes_named(&data, password, Some(&name));
    }
    // RAR volume sets (.partNN.rar numbering, or name.rar + name.rNN
    // siblings): concatenate the parts and open the concatenation —
    // RAR4 and RAR5 volume data are both plain byte splits.
    let rar_set = (name.contains(".part") && name.ends_with(".rar"))
        || (name.ends_with(".rar") && {
            let stem = &name[..name.len() - 4];
            archive.with_file_name(format!("{stem}.r00")).exists()
        });
    if rar_set {
        let parts = omnizip_rar::scan_volume_set(archive);
        let mut data =
            std::fs::read(&parts[0]).map_err(|e| format!("{}: {e}", parts[0].display()))?;
        for part in &parts[1..] {
            data.extend_from_slice(
                &std::fs::read(part).map_err(|e| format!("{}: {e}", part.display()))?,
            );
        }
        return open_bytes_named(&data, password, Some(&name));
    }
    let data = std::fs::read(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    open_bytes_named(&data, password, Some(&name))
}

/// A single-file codec payload: if the decompressed bytes are a tar,
/// recurse into the container path (the historical `ozip x file.tgz`
/// behavior); otherwise expose the payload itself as a one-entry
/// archive (task 53: Ruby's format-handler view).
fn payload_or_single(
    data: &[u8],
    hint: Option<&str>,
    codec: &str,
    decompress: fn(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Opened, String> {
    let inner = decompress(data)?;
    if detect_format(&inner) == FormatKind::Tar {
        return open_bytes_named(&inner, None, hint);
    }
    let name = match hint {
        Some(h) => strip_codec_suffix(h),
        None => format!("payload.{codec}"),
    };
    Ok(Opened::SingleFile { name, data: inner })
}

/// `hint`: the archive's file name, used to derive the entry name for
/// single-file payload views (`notes.txt.gz` -> `notes.txt`).
fn open_bytes_named(
    data: &[u8],
    password: Option<&str>,
    hint: Option<&str>,
) -> Result<Opened, String> {
    match detect_format(data) {
        FormatKind::Tar => omnizip_tar::TarReader::from_bytes(data)
            .map(|r| Opened::Tar(Box::new(r)))
            .map_err(|e| e.to_string()),
        FormatKind::Zip => omnizip_zip::ZipReader::from_bytes(data)
            .map(|r| Opened::Zip(Box::new(r)))
            .map_err(|e| e.to_string()),
        FormatKind::Cpio => omnizip_cpio::CpioReader::from_bytes(data)
            .map(|r| Opened::Cpio(Box::new(r)))
            .map_err(|e| e.to_string()),
        FormatKind::SevenZip => {
            omnizip_sevenzip::reader::SevenZipReader::from_bytes_with_password(data, password)
                .map(|r| Opened::SevenZip(Box::new(r)))
                .map_err(|e| e.to_string())
        }
        FormatKind::Rar5 => match password {
            Some(pw) => omnizip_rar::rar5::Rar5Reader::from_bytes_with_password(data, pw),
            None => omnizip_rar::rar5::Rar5Reader::from_bytes(data),
        }
        .map(|r| Opened::Rar5(Box::new(r)))
        .map_err(|e| e.to_string()),
        FormatKind::Rar4 => match password {
            Some(pw) => omnizip_rar::rar3::Rar4Reader::from_bytes_with_password(data, pw),
            None => omnizip_rar::rar3::Rar4Reader::from_bytes(data),
        }
        .map(|r| Opened::Rar4(Box::new(r)))
        .map_err(|e| e.to_string()),
        _ if data.starts_with(&[0xED, 0xAB, 0xEE, 0xDB]) => {
            omnizip_rpm::reader::RpmReader::from_bytes(data)
                .map(|r| Opened::Rpm(Box::new(r)))
                .map_err(|e| e.to_string())
        }
        _ if data.len() >= 16 * 2048 + 6
            && data.get(16 * 2048 + 1..16 * 2048 + 6) == Some(b"CD001") =>
        {
            omnizip_iso::reader::IsoReader::from_bytes(data)
                .map(|r| Opened::Iso(Box::new(r)))
                .map_err(|e| e.to_string())
        }
        // Compressed tar: unwrap the codec layer and parse the tar
        // inside — `ozip x` accepts what `ozip c` produces plus
        // anything the system tools emit.
        FormatKind::Gzip => payload_or_single(data, hint, "gzip", |d| {
            omnizip_archive_core::formats::gzip::decompress(d).map_err(|e| format!("gzip: {e}"))
        }),
        FormatKind::Bzip2 => payload_or_single(data, hint, "bzip2", |d| {
            omnizip_archive_core::formats::bzip2_file::decompress(d)
                .map_err(|e| format!("bzip2: {e}"))
        }),
        FormatKind::Xz => payload_or_single(data, hint, "xz", |d| {
            omnizip_lzma::xz_decompress(d).map_err(|e| format!("xz: {e}"))
        }),
        FormatKind::Zstd => payload_or_single(data, hint, "zstd", |d| {
            omnizip_zstd::decompress(d, u32::MAX).map_err(|e| format!("zstd: {e}"))
        }),
        FormatKind::Lzip => payload_or_single(data, hint, "lzip", |d| {
            omnizip_archive_core::formats::lzip::decompress(d).map_err(|e| format!("lzip: {e}"))
        }),
        FormatKind::LzmaAlone => payload_or_single(data, hint, "lzma", |d| {
            omnizip_archive_core::formats::lzma_alone::decompress(d)
                .map_err(|e| format!("lzma: {e}"))
        }),
        _ if data.starts_with(&[0xED, 0xAB, 0xEE, 0xDB]) => {
            omnizip_rpm::reader::RpmReader::from_bytes(data)
                .map(|r| Opened::Rpm(Box::new(r)))
                .map_err(|e| e.to_string())
        }
        _ if data.len() >= 16 * 2048 + 6
            && data.get(16 * 2048 + 1..16 * 2048 + 6) == Some(b"CD001") =>
        {
            omnizip_iso::reader::IsoReader::from_bytes(data)
                .map(|r| Opened::Iso(Box::new(r)))
                .map_err(|e| e.to_string())
        }
        FormatKind::Lz4 | FormatKind::Unknown => Err(format!(
            "not a container archive (detected {:?}); use 'ozip -d' for single-file codecs",
            detect_format(data)
        )),
        _ => Err("unsupported archive format".into()),
    }
}

/// `ozip verify ARCHIVE` — structural + checksum verification:
/// open the archive, parse every entry, and read every byte (zip
/// re-checks each entry's CRC32 on read; the single-file
/// decompressors verify their own trailers; container decoders
/// verify their block/stream digests). Prints one line per entry
/// and a summary; exits non-zero on any failure.
pub(crate) fn verify(archive: &Path, password: Option<&str>) -> Result<(), String> {
    let mut opened = open_archive(archive, password)?;
    let entries = opened.entries()?;
    let mut failed = 0_usize;
    let mut checked = 0_usize;
    for (index, entry) in entries.iter().enumerate() {
        if matches!(entry.kind, EntryKind::Directory) {
            println!("ok     dir  {}", entry.name);
            continue;
        }
        checked += 1;
        match opened.read_entry(index) {
            Ok(bytes) => {
                if let Some(size) = entry.size {
                    if bytes.len() as u64 != size {
                        failed += 1;
                        println!(
                            "FAIL   {}   size stored {} read {}",
                            entry.name,
                            size,
                            bytes.len()
                        );
                        continue;
                    }
                }
                println!("ok     {}   {} bytes", entry.name, bytes.len());
            }
            Err(e) => {
                failed += 1;
                println!("FAIL   {}   {}", entry.name, e);
            }
        }
    }
    if failed > 0 {
        Err(format!("{failed} of {checked} entries failed verification"))
    } else {
        println!("all {checked} entries verified");
        Ok(())
    }
}

/// Minimal JSON string escaping (quotes, backslash, control bytes
/// as \u00XX). Stable output: no maps iterated, archive order.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `ozip metadata ARCHIVE` — the entry table as JSON (Ruby
/// metadata_command parity at our ArchiveEntry fidelity).
pub(crate) fn metadata(archive: &Path, password: Option<&str>) -> Result<(), String> {
    let mut opened = open_archive(archive, password)?;
    let entries = opened.entries()?;
    println!("{{");
    println!(
        "  \"archive\": \"{}\",",
        json_escape(&archive.to_string_lossy())
    );
    println!("  \"entry_count\": {},", entries.len());
    println!("  \"entries\": [");
    for (i, entry) in entries.iter().enumerate() {
        let kind = match &entry.kind {
            EntryKind::Directory => "directory",
            EntryKind::Regular => "file",
            EntryKind::Symlink(_) | EntryKind::HardLink(_) => "link",
            EntryKind::Other(_) => "other",
        };
        let comma = if i + 1 == entries.len() { "" } else { "," };
        println!("    {{");
        println!("      \"name\": \"{}\",", json_escape(&entry.name));
        println!("      \"kind\": \"{kind}\",");
        println!(
            "      \"size\": {},",
            entry.size.map_or("null".to_string(), |s| s.to_string())
        );
        println!(
            "      \"mtime\": {},",
            entry.mtime.map_or("null".to_string(), |t| t.to_string())
        );
        println!(
            "      \"mode\": {},",
            entry
                .mode
                .map_or("null".to_string(), |m| format!("0o{:o}", m))
        );
        println!(
            "      \"method\": {}",
            entry.method.map_or("null".to_string(), |m| m.to_string())
        );
        println!("    }}{comma}");
    }
    println!("  ]");
    println!("}}");
    Ok(())
}

/// `ozip convert SRC DST` — the Ruby `ExtractRepackStrategy`: extract
/// the source under a private temp dir, repack through the same
/// deterministic create path. Metadata (mtimes, modes, symlinks,
/// empty dirs) travels through the filesystem; lossy cells are the
/// ones the fs cannot represent (hardlinks materialize, `Other`
/// kinds skip) — the dedicated zip⇄7z entry-at-a-time strategies
/// from the Ruby converter land separately (task 61 follow-up).
pub(crate) fn convert(
    src: &Path,
    dst: &Path,
    format: Option<&str>,
    level: Option<u8>,
    password: Option<&str>,
) -> Result<(), String> {
    // Staging name = source stem: deterministic (no pid/tmp randomness
    // leaking into the archive), and the top-level dir reads sanely.
    let stem = src.file_stem().map_or_else(
        || "converted".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let staging = std::env::temp_dir().join(format!("{stem}.converted"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let result = (|| {
        let mut opened = open_archive(src, password)?;
        opened.extract_to(&staging)?;
        create(
            dst,
            std::slice::from_ref(&staging),
            format,
            level,
            password,
            None,
        )
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// `ozip parity create|verify|repair` — CLI over the omnizip-par2
/// crate (task 55, second slice; the Ruby parity_* commands).
pub(crate) fn parity(args: &[String]) -> Result<(), String> {
    let sub = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| "ozip parity: expected create|verify|repair".to_string())?;
    let files: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with('-')).collect();
    let files = files.as_slice();
    match sub {
        "create" => {
            let input = files
                .first()
                .map(|s| s.as_str())
                .ok_or("ozip parity create: an input file is required")?;
            let count: u32 = flag_value(args, "-n")
                .map(|v| v.parse().map_err(|_| format!("bad -n '{v}'")))
                .transpose()?
                .unwrap_or(4);
            let out =
                flag_value(args, "-o").map_or_else(|| format!("{input}.par2"), |v| v.to_string());
            let data = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
            let name = Path::new(input)
                .file_name()
                .map_or_else(|| input.to_string(), |n| n.to_string_lossy().into_owned());
            let volume = omnizip_par2::verify::create(
                &[(name, data)],
                &omnizip_par2::verify::CreateOptions {
                    recovery_count: count,
                    ..omnizip_par2::verify::CreateOptions::default()
                },
            )
            .map_err(|e| e.to_string())?;
            std::fs::write(&out, &volume).map_err(|e| format!("{out}: {e}"))?;
            println!("{out}: {} recovery slices", count);
            Ok(())
        }
        "verify" => {
            let [input, par2] = two_paths(files, "verify")?;
            let (input, par2) = (input.as_str(), par2.as_str());
            let set = load_par2(par2)?;
            let data = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
            let name = Path::new(input)
                .file_name()
                .map_or_else(|| input.to_string(), |n| n.to_string_lossy().into_owned());
            let tracked = set
                .files
                .iter()
                .find(|f| f.name == name)
                .ok_or_else(|| format!("'{name}' is not covered by {}", par2))?;
            match omnizip_par2::verify::verify_file(tracked, Some(&data), set.block_size as usize) {
                omnizip_par2::verify::FileStatus::Ok => {
                    println!("{input}: OK");
                    Ok(())
                }
                omnizip_par2::verify::FileStatus::Missing => Err(format!("{input}: file missing")),
                omnizip_par2::verify::FileStatus::Damaged(slices) => Err(format!(
                    "{input}: {} damaged slice(s): {:?}",
                    slices.len(),
                    slices
                )),
            }
        }
        "repair" => {
            let [input, par2] = two_paths(files, "repair")?;
            let (input, par2) = (input.as_str(), par2.as_str());
            let set = load_par2(par2)?;
            let data = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
            let name = Path::new(input)
                .file_name()
                .map_or_else(|| input.to_string(), |n| n.to_string_lossy().into_owned());
            let (idx, tracked) = set
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == name)
                .ok_or_else(|| format!("'{name}' is not covered by {}", par2))?;
            let others: Vec<(usize, Vec<u8>)> = set
                .files
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != idx)
                .map(|(i, f)| {
                    let sibling = std::fs::read(&f.name).unwrap_or_default();
                    (i, sibling)
                })
                .collect();
            let repaired = omnizip_par2::verify::repair_file(&set, tracked, &data, &others)
                .map_err(|e| format!("repair: {e}"))?;
            let out = flag_value(args, "-o").map_or_else(|| input.to_string(), str::to_string);
            std::fs::write(&out, &repaired).map_err(|e| format!("{out}: {e}"))?;
            println!("{out}: repaired {} bytes", repaired.len());
            Ok(())
        }
        other => Err(format!(
            "ozip parity: unknown subcommand '{other}' (create|verify|repair)"
        )),
    }
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().map(String::as_str);
        }
    }
    None
}

fn two_paths(files: &[&String], sub: &str) -> Result<[String; 2], String> {
    let a = files.first().map(|s| s.as_str());
    let b = files.get(1).map(|s| s.as_str());
    match (a, b) {
        (Some(a), Some(b)) => Ok([a.to_string(), b.to_string()]),
        _ => Err(format!(
            "ozip parity {sub}: FILE and PAR2 paths are required"
        )),
    }
}

fn load_par2(path: &str) -> Result<omnizip_par2::RecoverySet, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let packets = omnizip_par2::packet::parse_packets(&data).map_err(|e| e.to_string())?;
    omnizip_par2::packet::assemble(&packets).map_err(|e| e.to_string())
}

/// `ozip convert` argument handling. Unambiguous arity rule:
/// exactly two paths = single conversion (SRC DST); three or more =
/// batch (SOURCE... DIR), where the last path is the output
/// directory created on demand. A mistyped batch can never silently
/// degrade into single mode and overwrite a source.
pub(crate) fn convert_command(
    paths: &[PathBuf],
    format: Option<&str>,
    level: Option<u8>,
    password: Option<&str>,
) -> Result<(), String> {
    if paths.len() >= 3 {
        if format.is_none() {
            return Err("ozip convert batch mode needs -f <format>".into());
        }
        let (sources, dir) = paths.split_at(paths.len() - 1);
        batch_convert(sources, &dir[0], format.expect("checked"), level, password)
    } else if paths.len() == 2 {
        convert(&paths[0], &paths[1], format, level, password)
    } else {
        Err("ozip convert: SRC DST (single) or SOURCE... DIR (batch, with -f)".into())
    }
}

/// Ruby `batch_convert`: convert many sources into `out_dir`, each
/// named `<source-stem>.<ext>` — `ext` is the canonical extension
/// of the requested format. Deterministic per-source output (the
/// single-source invariants carry over).
pub(crate) fn batch_convert(
    sources: &[PathBuf],
    out_dir: &Path,
    format: &str,
    level: Option<u8>,
    password: Option<&str>,
) -> Result<(), String> {
    if sources.is_empty() {
        return Err("batch convert needs at least one source".into());
    }
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for src in sources {
        let stem = src.file_stem().map_or_else(
            || "source".to_string(),
            |s| s.to_string_lossy().into_owned(),
        );
        let ext = canonical_ext(format);
        let dst = out_dir.join(format!("{stem}.{ext}"));
        convert(src, &dst, Some(format), level, password)?;
        println!("{} -> {}", src.display(), dst.display());
    }
    Ok(())
}

/// The canonical output extension per format name.
fn canonical_ext(format: &str) -> &'static str {
    match format {
        "tar.gz" | "gzip" | "gz" => "tar.gz",
        "tar.bz2" | "bzip2" | "bz2" => "tar.bz2",
        "tar.xz" | "xz" => "tar.xz",
        "tar.zst" | "zstd" | "zst" => "tar.zst",
        "lzip" | "lz" => "lz",
        "lzma" | "lzma-alone" => "lzma",
        "zip" => "zip",
        "cpio" => "cpio",
        "7z" => "7z",
        "rpm" => "rpm",
        "iso" => "iso",
        "rar" | "rar5" => "rar",
        _ => "converted",
    }
}

/// `ozip x ARCHIVE [-C DIR]` — extract under DIR (default `.`).
pub fn extract(
    archive: &Path,
    out_dir: Option<&Path>,
    password: Option<&str>,
) -> Result<(), String> {
    let mut opened = open_archive(archive, password)?;
    let dir = out_dir.unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    opened.extract_to(dir)
}

/// `ozip t` / `ozip l` — short and long listings.
pub fn list(archive: &Path, long: bool, password: Option<&str>) -> Result<(), String> {
    let mut opened = open_archive(archive, password)?;
    let entries = opened.entries()?;
    for entry in &entries {
        if long {
            println!(
                "{} {:>10}  {}  {}",
                mode_string(entry),
                entry.size.unwrap_or(0),
                mtime_string(entry),
                entry.name
            );
        } else {
            println!("{}", entry.name);
        }
    }
    Ok(())
}

fn mode_string(entry: &ArchiveEntry) -> String {
    let kind = match entry.kind {
        EntryKind::Directory => 'd',
        EntryKind::Symlink(_) => 'l',
        EntryKind::HardLink(_) => 'h',
        _ => '-',
    };
    let bits = entry
        .mode
        .unwrap_or(if entry.is_directory() { 0o755 } else { 0o644 });
    let mut s = String::new();
    s.push(kind);
    for shift in [6, 3, 0] {
        for (bit, ch) in [(0o4, 'r'), (0o2, 'w'), (0o1, 'x')] {
            s.push(if bits >> shift & bit != 0 { ch } else { '-' });
        }
    }
    s
}

/// Days→(y,m,d) and the inverse, UTC-proleptic (shared shape with the
/// zip writer's DOS-time helper).
fn mtime_string(entry: &ArchiveEntry) -> String {
    let Some(secs) = entry.mtime else {
        return "-------------------".into();
    };
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}
