//! `ozip` — the unified codec + container CLI (TODO.containers tasks
//! 18 and 15): xz / zstd / gzip / bzip2 / lzip / lzma-alone single-file
//! codecs with gzip(1)-style handling, plus `c/x/t/l` archive commands
//! over tar/zip/cpio (and compressed tar) with deterministic creation
//! by default.
//!
//! Pure Rust, no argument-parsing dependency: the codec set maps onto
//! a fixed table — adding a codec is one row, never a new flag branch.
#![forbid(unsafe_code)]

mod container;


use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

struct CodecSpec {
    name: &'static str,
    suffix: &'static str,
    /// (input, level) -> compressed bytes
    compress: fn(&[u8], u8) -> Result<Vec<u8>, String>,
    /// compressed -> plaintext
    decompress: fn(&[u8]) -> Result<Vec<u8>, String>,
    default_level: u8,
    max_level: u8,
}

fn specs() -> Vec<CodecSpec> {
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

fn zstd_level(lvl: u8) -> omnizip_zstd::ZstdLevel {
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

fn usage(codecs: &[CodecSpec]) {
    println!(
        "ozip {} — pure-Rust codec + container CLI",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("USAGE:");
    println!("    ozip <codec> [OPTIONS] [FILE ...]   compress FILEs (or stdin)");
    println!("    ozip -d [OPTIONS] [FILE ...]         decompress (codec from suffix/magic)");
    println!("    ozip c ARCHIVE INPUT...               create archive (format by ext or -f)");
    println!("    ozip x ARCHIVE [-C DIR]               extract archive (auto-detect)");
    println!("    ozip t ARCHIVE                        list entry names");
    println!("    ozip l ARCHIVE                        long listing (mode/size/mtime)");
    println!("    ozip --list-codecs                    codec registry");
    println!("    ozip --formats                        container registry");
    println!();
    println!("OPTIONS:");
    println!("    -#       compression level (codec range applies)");
    println!("    -d       decompress");
    println!("    -k       keep (do not delete) input files");
    println!("    -c       write to stdout");
    println!("    -o FILE  output name (single input only)");
    println!("    -f FMT   container format override (tar, tar.gz, zip, cpio, ...)");
    println!("    -C DIR   extraction directory (ozip x)");
    println!();
    println!("CODECS:");
    for c in codecs {
        println!(
            "    {:<8} .{:<4} levels 0-{} (default {})",
            c.name,
            c.suffix.trim_start_matches('.'),
            c.max_level,
            c.default_level
        );
    }
}

fn read_stdin() -> Vec<u8> {
    let mut v = Vec::new();
    std::io::stdin().read_to_end(&mut v).expect("stdin");
    v
}

fn detect(data: &[u8]) -> Option<&'static str> {
    use omnizip_archive_core::detect::{detect_format, FormatKind};
    match detect_format(data) {
        FormatKind::Xz => Some("xz"),
        FormatKind::Zstd => Some("zstd"),
        FormatKind::Gzip => Some("gzip"),
        FormatKind::Bzip2 => Some("bzip2"),
        FormatKind::Lzip => Some("lzip"),
        FormatKind::LzmaAlone => Some("lzma"),
        _ => None,
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let codecs = specs();

    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        usage(&codecs);
        return Ok(());
    }
    if args[0] == "--list-codecs" {
        for c in &codecs {
            println!(
                "{} {} 0-{} {}",
                c.name, c.suffix, c.max_level, c.default_level
            );
        }
        return Ok(());
    }
    if args[0] == "--formats" {
        container::print_formats();
        return Ok(());
    }

    // Container commands: c (create), x (extract), t (list), l (long).
    if matches!(args[0].as_str(), "c" | "x" | "t" | "l") {
        return run_container(&args);
    }

    let decompress = args.iter().any(|a| a == "-d");
    let keep = args.iter().any(|a| a == "-k");
    let to_stdout = args.iter().any(|a| a == "-c");
    let mut level: Option<u8> = None;
    let mut out_name: Option<String> = None;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut codec_name: Option<&str> = None;

    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "-d" || a == "-k" || a == "-c" {
            i += 1;
            continue;
        }
        if a == "-o" {
            out_name = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if a.len() >= 2 && a.starts_with('-') && a[1..].chars().all(|c| c.is_ascii_digit()) {
            level = Some(a[1..].parse().map_err(|_| format!("bad level {a}"))?);
            i += 1;
            continue;
        }
        if codec_name.is_none() && !a.starts_with('-') && codecs.iter().any(|c| c.name == *a) {
            codec_name = Some(a);
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            return Err(format!("unknown option {a}"));
        }
        files.push(PathBuf::from(a));
        i += 1;
    }

    // Resolve the codec: explicit name, else file suffix, else magic.
    let name = if let Some(n) = codec_name {
        n.to_string()
    } else if !files.is_empty() {
        let suffix = files[0]
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        codecs
            .iter()
            .find(|c| c.suffix == suffix)
            .map(|c| c.name.to_string())
            .ok_or_else(|| {
                format!(
                    "cannot infer codec from '{}'; name one of: {}",
                    files[0].display(),
                    codecs.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")
                )
            })?
    } else {
        return Err("no codec given and no input files".into());
    };
    let spec = codecs
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| format!("unknown codec {name}"))?;

    // stdin/stdout mode when no files.
    if files.is_empty() {
        let input = read_stdin();
        let out = if decompress {
            (spec.decompress)(&input)?
        } else {
            (spec.compress)(&input, level.unwrap_or(spec.default_level))?
        };
        std::io::stdout()
            .write_all(&out)
            .map_err(|e| format!("stdout: {e}"))?;
        return Ok(());
    };

    if files.len() > 1 && out_name.is_some() {
        return Err("-o applies to a single input".into());
    }
    if !decompress && codec_name.is_none() {
        return Err("compression requires an explicit codec".into());
    }

    for path in &files {
        let input = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // In decompress mode, sniff the actual format when the suffix
        // is absent or disagrees.
        let actual = if decompress {
            detect(&input)
                .map(String::from)
                .unwrap_or_else(|| name.clone())
        } else {
            name.clone()
        };
        let s = codecs
            .iter()
            .find(|c| c.name == actual)
            .ok_or_else(|| format!("unrecognized input format for {}", path.display()))?;

        let out = if decompress {
            (s.decompress)(&input)?
        } else {
            (s.compress)(&input, level.unwrap_or(s.default_level))?
        };

        let dest: PathBuf = if let Some(o) = &out_name {
            PathBuf::from(o)
        } else if decompress {
            let stem = path.with_extension("");
            if stem.as_os_str().is_empty() {
                return Err(format!("{}: no output name", path.display()));
            }
            stem
        } else {
            let mut p = path.clone().into_os_string();
            p.push(s.suffix);
            PathBuf::from(p)
        };

        if to_stdout {
            std::io::stdout()
                .write_all(&out)
                .map_err(|e| format!("stdout: {e}"))?;
        } else {
            std::fs::write(&dest, &out).map_err(|e| format!("{}: {e}", dest.display()))?;
        }

        if !keep && !to_stdout {
            std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn run_container(args: &[String]) -> Result<(), String> {
    let command = args[0].as_str();
    let mut level: Option<u8> = None;
    let mut format: Option<String> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut paths: Vec<PathBuf> = Vec::new();

    let mut i = 1usize;
    while i < args.len() {
        let a = &args[i];
        if a.len() >= 2 && a.starts_with('-') && a[1..].chars().all(|c| c.is_ascii_digit()) {
            level = Some(a[1..].parse().map_err(|_| format!("bad level {a}"))?);
            i += 1;
            continue;
        }
        if a == "-f" {
            format = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if a == "-C" {
            out_dir = args.get(i + 1).cloned().map(PathBuf::from);
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            return Err(format!("unknown option {a}"));
        }
        paths.push(PathBuf::from(a));
        i += 1;
    }

    if paths.is_empty() {
        return Err(format!("ozip {command}: an archive path is required"));
    }
    let archive = paths.remove(0);
    match command {
        "c" => container::create(&archive, &paths, format.as_deref(), level),
        "x" => container::extract(&archive, out_dir.as_deref()),
        "t" => container::list(&archive, false),
        "l" => container::list(&archive, true),
        _ => unreachable!(),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ozip: {e}");
            ExitCode::FAILURE
        }
    }
}
