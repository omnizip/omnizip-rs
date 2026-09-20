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

use container::{specs, CodecSpec};
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

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
    println!("    ozip verify ARCHIVE                   structural + checksum verification");
    println!("    ozip convert SRC DST                  re-encode into another format");
    println!("    ozip parity create|verify|repair      PAR2 recovery sets");
    println!("    ozip repair ARCHIVE                   verify + recovery-record audit (RAR)");
    println!("    ozip profile list|show                named compression profiles");
    println!("    ozip metadata ARCHIVE                 entry table as JSON");
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
    println!("    -p PASS  archive password (7z encrypted streams/headers)");
    println!("    --password-prompt          read the password from stdin");
    println!("    --password-file PATH       first line of PATH is the password");
    println!("    --password-env VAR         password from environment variable VAR");
    println!("    --volume N  split 7z output into N-byte .001/.002 parts (ozip c)");
    println!("    --threads N  parallel zip creation (byte-identical output)");
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
    if matches!(
        args[0].as_str(),
        "c" | "x" | "t" | "l" | "verify" | "metadata" | "convert" | "repair"
    ) {
        return run_container(&args);
    }
    if args[0] == "parity" {
        return container::parity(&args[1..]);
    }
    if args[0] == "profile" {
        return profile_command(&args[1..]);
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

    let mut password: Option<String> = None;
    let mut volume: Option<usize> = None;
    let mut threads: usize = 1;
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
        if a == "-p" {
            password = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if a == "--password-prompt" {
            print!("password: ");
            if std::io::Write::flush(&mut std::io::stdout()).is_err() {
                return Err("writing prompt failed".to_string());
            }
            let mut line = String::new();
            if let Err(e) = std::io::stdin().read_line(&mut line) {
                return Err(format!("reading password: {e}"));
            }
            let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
            password = Some(trimmed);
            i += 1;
            continue;
        }
        if a == "--password-file" {
            let path = args
                .get(i + 1)
                .ok_or_else(|| "--password-file needs a path".to_string())?;
            let raw = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
            password = match raw.lines().next() {
                Some(line) => Some(line.to_string()),
                None => return Err(format!("{path} is empty")),
            };
            i += 2;
            continue;
        }
        if a == "--password-env" {
            let var = args
                .get(i + 1)
                .ok_or_else(|| "--password-env needs a variable name".to_string())?;
            password = Some(std::env::var(var).map_err(|e| format!("reading ${var}: {e}"))?);
            i += 2;
            continue;
        }
        if a == "--threads" {
            threads = args
                .get(i + 1)
                .ok_or_else(|| "--threads needs a count".to_string())?
                .parse()
                .map_err(|_| format!("bad thread count {}", args[i + 1]))?;
            i += 2;
            continue;
        }
        if a == "--volume" {
            volume = Some(
                args.get(i + 1)
                    .ok_or_else(|| "--volume needs a byte size".to_string())?
                    .parse()
                    .map_err(|_| format!("bad volume size {}", args[i + 1]))?,
            );
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
    if command != "c" && volume.is_some() {
        return Err("--volume only applies to ozip c".into());
    }
    if command == "convert" {
        return container::convert_command(&paths, format.as_deref(), level, password.as_deref());
    }
    let archive = paths.remove(0);
    match command {
        "c" => container::create_with_threads(
            &archive,
            &paths,
            format.as_deref(),
            level,
            password.as_deref(),
            volume,
            threads,
        ),
        "x" => container::extract(&archive, out_dir.as_deref(), password.as_deref()),
        "t" => container::list(&archive, false, password.as_deref()),
        "l" => container::list(&archive, true, password.as_deref()),
        "verify" => container::verify(&archive, password.as_deref()),
        "metadata" => container::metadata(&archive, password.as_deref()),
        "repair" => container::archive_repair(&archive, password.as_deref()),
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

fn profile_command(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("list") => {
            println!(
                "{:<10} {:<8} {:<6} {:<10} {:<6} DESCRIPTION",
                "NAME", "CODEC", "LEVEL", "FILTER", "SOLID"
            );
            for p in omnizip_codecs::profile::named_profiles() {
                println!(
                    "{:<10} {:<8} {:<6} {:<10} {:<6} {}",
                    p.name,
                    p.codec,
                    p.level,
                    match p.filter {
                        omnizip_codecs::profile::ProfileFilter::None => "-",
                        omnizip_codecs::profile::ProfileFilter::BcjX86 => "bcj_x86",
                        omnizip_codecs::profile::ProfileFilter::Auto => "auto",
                    },
                    if p.solid { "yes" } else { "no" },
                    p.description
                );
            }
            Ok(())
        }
        Some("show") => {
            let name = args
                .get(1)
                .ok_or_else(|| "ozip profile show: a profile name is required".to_string())?;
            let p = omnizip_codecs::profile::named_profile(name)
                .ok_or_else(|| format!("unknown profile '{name}' (see: ozip profile list)"))?;
            println!("name:        {}", p.name);
            println!("codec:       {}", p.codec);
            println!("level:       {}", p.level);
            println!("solid:       {}", p.solid);
            println!("description: {}", p.description);
            Ok(())
        }
        other => Err(format!(
            "ozip profile: expected list|show, got {}",
            other.unwrap_or("(nothing)")
        )),
    }
}
