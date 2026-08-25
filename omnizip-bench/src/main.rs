//! CLI entry point for `omnizip-bench`.

use std::io::{self, Write};

use clap::{Parser, ValueEnum};
use omnizip_bench::{
    default_codecs, known_corpora, run_suite, synthetic, Corpus, CsvReporter, JsonReporter,
    MarkdownReporter, Reporter,
};

#[derive(Debug, Clone, ValueEnum)]
enum Format {
    Csv,
    Json,
    Markdown,
}

#[derive(Debug, Parser)]
#[command(
    name = "omnizip-bench",
    about = "Benchmark omnizip-rs codecs on standard compression corpora."
)]
struct Args {
    /// Comma-separated codec names (default: all).
    #[arg(long, value_delimiter = ',')]
    codec: Option<Vec<String>>,

    /// Comma-separated compression levels (overrides each codec's defaults).
    #[arg(long, value_delimiter = ',')]
    level: Option<Vec<u8>>,

    /// Comma-separated corpus names from `known_corpora` (downloads on first use).
    #[arg(long, value_delimiter = ',')]
    corpus: Option<Vec<String>>,

    /// Use synthetic in-process corpora of this size (bytes), no network.
    /// Useful for CI smoke tests.
    #[arg(long, default_value = "0")]
    synthetic: usize,

    /// Iterations per case (best-of is reported). Default 3.
    #[arg(long, default_value_t = 3)]
    iterations: u32,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    format: Format,

    /// List known corpora and exit.
    #[arg(long)]
    list_corpora: bool,

    /// List known codecs and exit.
    #[arg(long)]
    list_codecs: bool,

    /// Run the archive-level benchmark suite (create + extract per
    /// format, with the determinism double-run check).
    #[arg(long)]
    archives: bool,

    /// Diff two benchmark JSON outputs (positional args after `--`).
    /// Usage: omnizip-bench --diff -- baseline.json current.json
    #[arg(long)]
    diff: bool,
}

fn run_diff(files: &[String]) -> i32 {
    if files.len() != 2 {
        eprintln!("usage: omnizip-bench --diff -- <baseline.json> <current.json>");
        return 1;
    }
    let baseline_text = match std::fs::read_to_string(&files[0]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("read {}: {e}", files[0]);
            return 2;
        }
    };
    let current_text = match std::fs::read_to_string(&files[1]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("read {}: {e}", files[1]);
            return 2;
        }
    };
    let baseline: serde_json::Value = match serde_json::from_str(&baseline_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("parse {}: {e}", files[0]);
            return 3;
        }
    };
    let current: serde_json::Value = match serde_json::from_str(&current_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("parse {}: {e}", files[1]);
            return 3;
        }
    };

    let baseline_results = baseline.get("results").and_then(|v| v.as_array());
    let current_results = current.get("results").and_then(|v| v.as_array());
    let (Some(baseline_results), Some(current_results)) = (baseline_results, current_results)
    else {
        eprintln!("missing 'results' array in one of the files");
        return 4;
    };

    // Build a map: case_key -> compressed_bytes from baseline.
    let mut baseline_map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for entry in baseline_results {
        let key = format!(
            "{}/{}/{}",
            entry.get("codec").and_then(|v| v.as_str()).unwrap_or("?"),
            entry.get("level").and_then(|v| v.as_i64()).unwrap_or(0),
            entry.get("file").and_then(|v| v.as_str()).unwrap_or("?"),
        );
        if let Some(b) = entry.get("compressed_bytes").and_then(|v| v.as_u64()) {
            baseline_map.insert(key, b);
        }
    }

    let mut regressions = 0;
    let mut improvements = 0;
    println!(
        "{:<50} {:>15} {:>15} {:>10}",
        "case", "baseline", "current", "delta%"
    );
    println!("{}", "-".repeat(95));
    for entry in current_results {
        let key = format!(
            "{}/{}/{}",
            entry.get("codec").and_then(|v| v.as_str()).unwrap_or("?"),
            entry.get("level").and_then(|v| v.as_i64()).unwrap_or(0),
            entry.get("file").and_then(|v| v.as_str()).unwrap_or("?"),
        );
        let Some(current_bytes) = entry.get("compressed_bytes").and_then(|v| v.as_u64()) else {
            continue;
        };
        let baseline_bytes = match baseline_map.get(&key) {
            Some(&b) => b,
            None => {
                println!("{key:<50}     unknown     {current_bytes:>13} NEW");
                continue;
            }
        };
        let delta_pct =
            (current_bytes as f64 - baseline_bytes as f64) / baseline_bytes as f64 * 100.0;
        let marker = if delta_pct > 1.0 {
            regressions += 1;
            "REGRESSION"
        } else if delta_pct < -1.0 {
            improvements += 1;
            "improved"
        } else {
            ""
        };
        println!("{key:<50} {baseline_bytes:>15} {current_bytes:>15} {delta_pct:>+8.2}% {marker}");
    }

    eprintln!("\n{improvements} improvement(s), {regressions} regression(s)");
    if regressions > 0 {
        5
    } else {
        0
    }
}

fn main() {
    let args = Args::parse();

    if args.diff {
        let files: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(run_diff(&files));
    }

    if args.list_corpora {
        for c in known_corpora() {
            println!(
                "{:<12} {:>10} MB  {}",
                c.name,
                c.approx_size / 1_000_000,
                c.description
            );
        }
        return;
    }

    if args.list_codecs {
        for c in default_codecs() {
            println!("{:<10} levels {:?}", c.name(), c.levels());
        }
        return;
    }

    if args.archives {
        omnizip_bench::archives::run();
        return;
    }

    let codecs: Vec<_> = default_codecs()
        .into_iter()
        .filter(|c| match &args.codec {
            None => true,
            Some(names) => names.iter().any(|n| n == c.name()),
        })
        .map(|c| match &args.level {
            None => c,
            Some(levels) => c.with_levels(levels.clone()),
        })
        .collect();

    if codecs.is_empty() {
        eprintln!("[bench] no codecs matched; nothing to do");
        std::process::exit(1);
    }

    let corpora: Vec<Corpus> = if args.synthetic > 0 {
        synthetic::all(args.synthetic)
    } else {
        let names = args
            .corpus
            .clone()
            .unwrap_or_else(|| vec!["calgary".to_string()]);
        names
            .iter()
            .map(|n| omnizip_bench::corpus::load(n))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|e| {
                eprintln!("[bench] corpus load failed: {e}");
                std::process::exit(2);
            })
    };

    let mut all_results = Vec::new();
    for corpus in &corpora {
        eprintln!(
            "[bench] running {} codec(s) on {} ({} files, {} bytes)",
            codecs.len(),
            corpus.name(),
            corpus.files().len(),
            corpus.total_size()
        );
        all_results.extend(run_suite(&codecs, corpus, args.iterations));
    }

    let reporter: Box<dyn Reporter> = match args.format {
        Format::Csv => Box::new(CsvReporter),
        Format::Json => Box::new(JsonReporter),
        Format::Markdown => Box::new(MarkdownReporter),
    };
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(reporter.report(&all_results).as_bytes());

    let failed = all_results
        .iter()
        .filter(|r| !r.error.is_empty() && !r.error.contains("level out of range"))
        .count();
    if failed > 0 {
        eprintln!(
            "[bench] WARNING: {failed} cases failed (non-deterministic or round-trip mismatch)"
        );
        std::process::exit(3);
    }
}
