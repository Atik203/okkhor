//! Report schemas and optional JSON/CSV files under `bench-out/`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::ranking::RankingRun;
use crate::stats::LatencySummary;

pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// Output format selected on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
    Csv,
}

/// JSON/CSV schema for `--ranking`.
#[derive(Debug, Clone, Serialize)]
pub struct RankingReport {
    pub schema_version: u32,
    pub mode: &'static str,
    pub seed: u64,
    pub iterations: usize,
    pub warmup_iterations: usize,
    pub candidates_per_iteration: usize,
    pub cases: usize,
    pub english_cases: usize,
    pub bangla_cases: usize,
    pub store_words: usize,
    pub store_pairs: usize,
    pub store_bigrams: usize,
    pub ranked_total: usize,
    pub latency: LatencySummary,
    pub iterations_per_second: f64,
}

impl RankingReport {
    pub fn from_run(run: &RankingRun, seed: u64) -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            mode: "ranking",
            seed,
            iterations: run.iterations,
            warmup_iterations: run.warmup_iterations,
            candidates_per_iteration: run.candidates_per_iteration,
            cases: run.english_cases + run.bangla_cases,
            english_cases: run.english_cases,
            bangla_cases: run.bangla_cases,
            store_words: run.store_words,
            store_pairs: run.store_pairs,
            store_bigrams: run.store_bigrams,
            ranked_total: run.ranked_total,
            latency: run.latency.clone(),
            iterations_per_second: run.latency.iterations_per_second(),
        }
    }
}

/// JSON/CSV schema for `--injection`.
#[derive(Debug, Clone, Serialize)]
pub struct InjectionReport {
    pub schema_version: u32,
    pub mode: &'static str,
    pub path: &'static str,
    pub deferred: bool,
    pub reason: Option<String>,
    pub iterations: usize,
    pub all_correct: bool,
    pub languages: Vec<InjectionLanguageReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InjectionLanguageReport {
    pub language: &'static str,
    pub text_utf16_units: usize,
    pub method: &'static str,
    pub correct: usize,
    pub failed: usize,
    /// Time spent inside `WindowsInjector::inject`.
    pub inject_latency: LatencySummary,
    /// Time until the target's text read back correctly. This includes target
    /// scheduling and the benchmark's polling, so it is a diagnostic, not a
    /// release claim.
    pub readback_latency: Option<LatencySummary>,
}

/// Write a ranking report if a machine-readable format was requested.
pub fn write_ranking(report: &RankingReport, format: Format) -> io::Result<Option<PathBuf>> {
    match format {
        Format::Human => Ok(None),
        Format::Json => write_json("ranking", report).map(Some),
        Format::Csv => {
            let mut output = String::new();
            output.push_str(
                "schema_version,mode,seed,iterations,warmup_iterations,\
                 candidates_per_iteration,cases,english_cases,bangla_cases,\
                 store_words,store_pairs,store_bigrams,ranked_total,count,\
                 p50_ns,p95_ns,p99_ns,max_ns,total_ns,iterations_per_second\n",
            );
            output.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3}\n",
                report.schema_version,
                report.mode,
                report.seed,
                report.iterations,
                report.warmup_iterations,
                report.candidates_per_iteration,
                report.cases,
                report.english_cases,
                report.bangla_cases,
                report.store_words,
                report.store_pairs,
                report.store_bigrams,
                report.ranked_total,
                report.latency.count,
                report.latency.p50_ns,
                report.latency.p95_ns,
                report.latency.p99_ns,
                report.latency.max_ns,
                report.latency.total_ns,
                report.iterations_per_second,
            ));
            write_text("ranking", "csv", &output).map(Some)
        }
    }
}

/// Write an injection report if a machine-readable format was requested.
pub fn write_injection(report: &InjectionReport, format: Format) -> io::Result<Option<PathBuf>> {
    match format {
        Format::Human => Ok(None),
        Format::Json => write_json("injection", report).map(Some),
        Format::Csv => {
            let mut output = String::new();
            output.push_str(
                "schema_version,mode,path,deferred,reason,iterations,all_correct,\
                 language,text_utf16_units,method,correct,failed,inject_count,\
                 inject_p50_ns,inject_p95_ns,inject_p99_ns,inject_max_ns,\
                 inject_total_ns,readback_count,readback_p50_ns,readback_p95_ns,\
                 readback_p99_ns,readback_max_ns,readback_total_ns\n",
            );
            let reason = report.reason.as_deref().unwrap_or("");
            for language in &report.languages {
                let readback = language.readback_latency.as_ref();
                output.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                    report.schema_version,
                    report.mode,
                    report.path,
                    report.deferred,
                    csv_field(reason),
                    report.iterations,
                    report.all_correct,
                    language.language,
                    language.text_utf16_units,
                    language.method,
                    language.correct,
                    language.failed,
                    language.inject_latency.count,
                    language.inject_latency.p50_ns,
                    language.inject_latency.p95_ns,
                    language.inject_latency.p99_ns,
                    language.inject_latency.max_ns,
                    language.inject_latency.total_ns,
                    readback.map(|summary| summary.count).unwrap_or(0),
                    readback.map(|summary| summary.p50_ns).unwrap_or(0),
                    readback.map(|summary| summary.p95_ns).unwrap_or(0),
                    readback.map(|summary| summary.p99_ns).unwrap_or(0),
                    readback.map(|summary| summary.max_ns).unwrap_or(0),
                    readback.map(|summary| summary.total_ns).unwrap_or(0),
                ));
            }
            write_text("injection", "csv", &output).map(Some)
        }
    }
}

fn write_json<T: Serialize>(prefix: &str, report: &T) -> io::Result<PathBuf> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|error| io::Error::other(error.to_string()))?;
    write_text(prefix, "json", &format!("{json}\n"))
}

fn write_text(prefix: &str, extension: &str, contents: &str) -> io::Result<PathBuf> {
    let directory = bench_out_directory()?;
    fs::create_dir_all(&directory)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = directory.join(format!(
        "{prefix}-{stamp}-{}.{extension}",
        std::process::id()
    ));
    fs::write(&path, contents)?;
    Ok(path)
}

fn bench_out_directory() -> io::Result<PathBuf> {
    Ok(std::env::current_dir()?.join(Path::new("bench-out")))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn csv_fields_are_escaped() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
    }
}
