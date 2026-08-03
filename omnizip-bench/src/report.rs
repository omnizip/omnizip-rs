//! Reporters — strategy pattern for emitting benchmark results.
//!
//! Each reporter is a struct implementing [`Reporter`]. Adding a new
//! output format (e.g. HTML, Excel) = one new impl, no runner edits
//! (open/closed).

use crate::case::BenchmarkResult;

/// Emit a textual report from benchmark results.
pub trait Reporter {
    fn report(&self, results: &[BenchmarkResult]) -> String;
}

/// CSV reporter — one header row + one row per result.
pub struct CsvReporter;

impl Reporter for CsvReporter {
    fn report(&self, results: &[BenchmarkResult]) -> String {
        let mut out = String::from(
            "codec,level,corpus,file,input_size,compressed_size,ratio,\
             encode_ms,decode_ms,encode_mib_s,decode_mib_s,\
             deterministic,roundtrip_ok,error\n",
        );
        for r in results {
            out.push_str(&format!(
                "{},{},{},{},{},{},{:.6},{:.3},{:.3},{:.2},{:.2},{},{},\"{}\"\n",
                r.codec,
                r.level,
                r.corpus,
                r.file,
                r.input_size,
                r.compressed_size,
                r.ratio,
                r.encode_ms,
                r.decode_ms,
                r.encode_mib_s,
                r.decode_mib_s,
                r.deterministic,
                r.roundtrip_ok,
                r.error.replace('"', "'"),
            ));
        }
        out
    }
}

/// JSON reporter — serde-derived, one object per result, wrapped in an array.
pub struct JsonReporter;

impl Reporter for JsonReporter {
    fn report(&self, results: &[BenchmarkResult]) -> String {
        serde_json::to_string_pretty(results).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
    }
}

/// Markdown reporter — pipe-table for GitHub READMEs.
pub struct MarkdownReporter;

impl Reporter for MarkdownReporter {
    fn report(&self, results: &[BenchmarkResult]) -> String {
        let mut out = String::from(
            "| codec | level | corpus | file | input | compressed | ratio | enc ms | dec ms | enc MiB/s | dec MiB/s | det | rt | error |\n",
        );
        out.push_str(
            "|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|---|\n",
        );
        for r in results {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {:.4} | {:.2} | {:.2} | {:.1} | {:.1} | {} | {} | {} |\n",
                r.codec,
                r.level,
                r.corpus,
                r.file,
                r.input_size,
                r.compressed_size,
                r.ratio,
                r.encode_ms,
                r.decode_ms,
                r.encode_mib_s,
                r.decode_mib_s,
                if r.deterministic { "✓" } else { "✗" },
                if r.roundtrip_ok { "✓" } else { "✗" },
                r.error,
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result(codec: &str, level: u8, ratio: f64, ok: bool) -> BenchmarkResult {
        BenchmarkResult {
            codec: codec.to_string(),
            level,
            corpus: "test".to_string(),
            file: "f.bin".to_string(),
            input_size: 1000,
            compressed_size: (ratio * 1000.0) as u64,
            ratio,
            encode_ms: 10.0,
            decode_ms: 5.0,
            encode_mib_s: 95.0,
            decode_mib_s: 190.0,
            deterministic: ok,
            roundtrip_ok: ok,
            error: if ok { String::new() } else { "fail".to_string() },
        }
    }

    #[test]
    fn csv_has_header_and_rows() {
        let out = CsvReporter.report(&[sample_result("zstd", 3, 0.5, true)]);
        assert!(out.starts_with("codec,"));
        assert!(out.contains("zstd,3,test,f.bin"));
    }

    #[test]
    fn json_is_valid() {
        let out = JsonReporter.report(&[sample_result("lzma", 6, 0.4, true)]);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed[0]["codec"], "lzma");
    }

    #[test]
    fn markdown_has_table_header() {
        let out = MarkdownReporter.report(&[sample_result("brotli", 9, 0.45, true)]);
        assert!(out.contains("| codec |"));
        assert!(out.contains("|---|"));
        assert!(out.contains("| brotli | 9 |"));
    }
}
