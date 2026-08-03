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
}

fn main() {
    let args = Args::parse();

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
