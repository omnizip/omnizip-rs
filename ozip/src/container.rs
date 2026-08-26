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
        "rpm", "iso", "rar",
    ] {
        if name.ends_with(candidate) {
            return match candidate {
                "tar.gz" | "tgz" => Ok(OutputFormat::TarGzip),
                "tar.bz2" | "tbz2" => Ok(OutputFormat::TarBzip2),
                "tar.xz" | "txz" => Ok(OutputFormat::TarXz),
                "tar.zst" => Ok(OutputFormat::TarZstd),
                "tar" => Ok(OutputFormat::Tar),
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
    let staged = stage(inputs, &options)?;

    let bytes = match output {
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
            &|tar| omnizip_zstd::compress(tar, crate::zstd_level(level)).map_err(|e| e.to_string()),
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

    fn extract_to(&mut self, dir: &Path) -> Result<(), String> {
        let policy = SecurityPolicy::default();
        match self {
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
        return open_bytes(&data, password);
    }
    let data = std::fs::read(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    open_bytes(&data, password)
}

fn open_bytes(data: &[u8], password: Option<&str>) -> Result<Opened, String> {
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
        FormatKind::Rar5 => omnizip_rar::rar5::Rar5Reader::from_bytes(data)
            .map(|r| Opened::Rar5(Box::new(r)))
            .map_err(|e| e.to_string()),
        FormatKind::Rar4 => omnizip_rar::rar3::Rar4Reader::from_bytes(data)
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
        FormatKind::Gzip => {
            let inner = omnizip_archive_core::formats::gzip::decompress(data)
                .map_err(|e| format!("gzip: {e}"))?;
            match open_bytes(&inner, None)? {
                opened @ Opened::Tar(_) => Ok(opened),
                _ => Err("gzip payload is not a tar archive".into()),
            }
        }
        FormatKind::Bzip2 => {
            let inner = omnizip_archive_core::formats::bzip2_file::decompress(data)
                .map_err(|e| format!("bzip2: {e}"))?;
            match open_bytes(&inner, None)? {
                opened @ Opened::Tar(_) => Ok(opened),
                _ => Err("bzip2 payload is not a tar archive".into()),
            }
        }
        FormatKind::Xz => {
            let inner = omnizip_lzma::xz_decompress(data).map_err(|e| format!("xz: {e}"))?;
            match open_bytes(&inner, None)? {
                opened @ Opened::Tar(_) => Ok(opened),
                _ => Err("xz payload is not a tar archive".into()),
            }
        }
        FormatKind::Zstd => {
            let inner =
                omnizip_zstd::decompress(data, u32::MAX).map_err(|e| format!("zstd: {e}"))?;
            match open_bytes(&inner, None)? {
                opened @ Opened::Tar(_) => Ok(opened),
                _ => Err("zstd payload is not a tar archive".into()),
            }
        }
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
        FormatKind::Lz4 | FormatKind::Lzip | FormatKind::LzmaAlone | FormatKind::Unknown => {
            Err(format!(
                "not a container archive (detected {:?}); use 'ozip -d' for single-file codecs",
                detect_format(data)
            ))
        }
        _ => Err("unsupported archive format".into()),
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
