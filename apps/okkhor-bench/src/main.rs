//! Okkhor 0.2 benchmark harness.
//!
//! This is a developer tool, not a daemon component. It runs no network, installs
//! no global keyboard hook, and uses only synthetic English/Bangla fixtures.

mod candidates;
mod corpus;
mod injection;
mod output;
mod ranking;
mod stats;
mod suggestions;

use std::error::Error;
use std::fmt;
use std::io;
use std::process::ExitCode;

use candidates::FixtureComponents;
use corpus::SyntheticCorpus;
use injection::InjectionOptions;
use output::{CandidatesReport, Format, InjectionReport, RankingReport};

const DEFAULT_SEED: u64 = 0x4f4b_4b48_4f52;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(BenchError::Usage(message)) => {
            eprintln!("okkhor-bench: {message}\n");
            print_help();
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("okkhor-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), BenchError> {
    let options = parse_args(std::env::args().skip(1))?;
    if options.help {
        print_help();
        return Ok(());
    }
    if options.version {
        println!("okkhor-bench {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let format = if options.json {
        Format::Json
    } else if options.csv {
        Format::Csv
    } else {
        Format::Human
    };

    if options.ranking {
        let iterations = options
            .iterations
            .unwrap_or(if options.smoke { 100 } else { 5_000 });
        let warmup = options.warmup.unwrap_or_else(|| default_warmup(iterations));
        let corpus = SyntheticCorpus::build(options.seed);
        let measurement = ranking::run(&corpus, iterations, warmup);
        let report = RankingReport::from_run(&measurement, options.seed);
        print_ranking(&report);
        if let Some(path) = output::write_ranking(&report, format)? {
            println!("ranking: wrote {}", path.display());
        }
        if options.smoke {
            validate_smoke(&report)?;
        }
    }

    if options.candidates {
        // Parity always covers the full committed corpus. The timed loop
        // cycles through all cases, or a one-case subset for `--smoke` CI.
        let subset = options.smoke.then_some(1);
        let iterations = options
            .iterations
            .unwrap_or(if options.smoke { 10 } else { 2_000 });
        let corpus =
            candidates::load_corpus().map_err(|error| BenchError::Failed(error.to_string()))?;
        let components = FixtureComponents::build();
        let engine = components
            .engine()
            .map_err(|error| BenchError::Failed(error.to_string()))?;
        let warmup = options.warmup.unwrap_or_else(|| default_warmup(iterations));
        let measurement = candidates::run(&engine, &corpus, iterations, warmup, subset);
        let report = CandidatesReport::from_run(&measurement);
        print_candidates(&report);
        if let Some(path) = output::write_candidates(&report, format)? {
            println!("candidates: wrote {}", path.display());
        }
        if options.smoke {
            validate_candidates_smoke(&report)?;
        }
    }

    if options.suggestions {
        let iterations = options
            .iterations
            .unwrap_or(if options.smoke { 100 } else { 2_000 });
        let warmup = options.warmup.unwrap_or_else(|| default_warmup(iterations));
        let report = suggestions::run(iterations, warmup).map_err(BenchError::Failed)?;
        print_suggestions(&report);
        if options.smoke {
            validate_suggestions_smoke(&report)?;
        }
    }

    if options.injection {
        let iterations = options
            .iterations
            .unwrap_or(if options.smoke { 5 } else { 20 });
        let report = injection::run(InjectionOptions {
            iterations,
            force_sendinput: options.sendinput,
        });
        print_injection(&report);
        if let Some(path) = output::write_injection(&report, format)? {
            println!("injection: wrote {}", path.display());
        }
    }

    Ok(())
}

fn print_suggestions(report: &suggestions::SuggestionReport) {
    println!(
        "suggestions: top1 {}/{} ({:.1}%), top5 {}/{} ({:.1}%)",
        report.top1,
        report.total,
        percent(report.top1, report.total),
        report.top5,
        report.total,
        percent(report.top5, report.total)
    );
    if let Some(latency) = &report.latency {
        println!(
            "suggest latency: p50 {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, max {:.3} ms, {} samples",
            nanos_ms(latency.p50_ns),
            nanos_ms(latency.p95_ns),
            nanos_ms(latency.p99_ns),
            nanos_ms(latency.max_ns),
            latency.count
        );
    }
}

fn validate_suggestions_smoke(report: &suggestions::SuggestionReport) -> Result<(), BenchError> {
    if report.total == 0 || report.top5 == 0 {
        return Err(BenchError::Failed(
            "suggestion smoke produced no hits".to_owned(),
        ));
    }
    let Some(latency) = &report.latency else {
        return Err(BenchError::Failed(
            "suggestion smoke produced no latency samples".to_owned(),
        ));
    };
    if !latency.is_monotonic() {
        return Err(BenchError::Failed(
            "suggestion smoke found non-monotonic percentiles".to_owned(),
        ));
    }
    Ok(())
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

fn nanos_ms(value: u64) -> f64 {
    value as f64 / 1_000_000.0
}

fn validate_smoke(report: &RankingReport) -> Result<(), BenchError> {
    if report.iterations == 0 {
        return Err(BenchError::Failed(
            "smoke check requires at least one iteration".to_owned(),
        ));
    }
    if report.latency.count != report.iterations {
        return Err(BenchError::Failed(format!(
            "smoke check saw {} samples for {} iterations",
            report.latency.count, report.iterations
        )));
    }
    if !report.latency.is_monotonic() {
        return Err(BenchError::Failed(
            "smoke check found non-monotonic percentiles".to_owned(),
        ));
    }
    if report.english_cases == 0 || report.bangla_cases == 0 {
        return Err(BenchError::Failed(
            "smoke check requires both English and Bangla cases".to_owned(),
        ));
    }
    if report.ranked_total == 0 {
        return Err(BenchError::Failed(
            "smoke check returned no ranked suggestions".to_owned(),
        ));
    }
    println!("smoke: ok");
    Ok(())
}

fn print_ranking(report: &RankingReport) {
    println!(
        "ranking: seed={} iterations={} warmup={} candidates/iteration={} cases={} (en={}, bn={})",
        report.seed,
        report.iterations,
        report.warmup_iterations,
        report.candidates_per_iteration,
        report.cases,
        report.english_cases,
        report.bangla_cases
    );
    println!(
        "store: words={} pairs={} bigrams={}",
        report.store_words, report.store_pairs, report.store_bigrams
    );
    println!(
        "latency: p50={} p95={} p99={} max={} (n={})",
        format_ns(report.latency.p50_ns),
        format_ns(report.latency.p95_ns),
        format_ns(report.latency.p99_ns),
        format_ns(report.latency.max_ns),
        report.latency.count
    );
    println!(
        "throughput: {:.0} iterations/sec",
        report.iterations_per_second
    );
}

fn print_candidates(report: &CandidatesReport) {
    println!(
        "candidates: corpus={} cases={} top1={}/{} ({:.3}) top5={}/{} ({:.3})",
        report.corpus,
        report.cases,
        report.top1_hits,
        report.cases,
        report.top1_rate,
        report.top5_hits,
        report.cases,
        report.top5_rate,
    );
    println!(
        "candidates: iterations={} warmup={} subset_cases={} suggestions={}",
        report.iterations, report.warmup_iterations, report.subset_cases, report.suggestions_total
    );
    println!(
        "latency: p50={} p95={} p99={} max={} (n={})",
        format_ns(report.latency.p50_ns),
        format_ns(report.latency.p95_ns),
        format_ns(report.latency.p99_ns),
        format_ns(report.latency.max_ns),
        report.latency.count
    );
    println!(
        "throughput: {:.0} iterations/sec",
        report.iterations_per_second
    );
}

fn validate_candidates_smoke(report: &CandidatesReport) -> Result<(), BenchError> {
    if report.iterations == 0 {
        return Err(BenchError::Failed(
            "smoke check requires at least one iteration".to_owned(),
        ));
    }
    if report.latency.count != report.iterations {
        return Err(BenchError::Failed(format!(
            "smoke check saw {} samples for {} iterations",
            report.latency.count, report.iterations
        )));
    }
    if !report.latency.is_monotonic() {
        return Err(BenchError::Failed(
            "smoke check found non-monotonic percentiles".to_owned(),
        ));
    }
    if report.cases == 0 || report.subset_cases == 0 {
        return Err(BenchError::Failed(
            "smoke check requires at least one golden case".to_owned(),
        ));
    }
    if report.top1_hits > report.cases || report.top5_hits > report.cases {
        return Err(BenchError::Failed(
            "smoke check found parity hits above the case count".to_owned(),
        ));
    }
    if report.suggestions_total == 0 {
        return Err(BenchError::Failed(
            "smoke check returned no suggestions".to_owned(),
        ));
    }
    println!("smoke: ok");
    Ok(())
}

fn print_injection(report: &InjectionReport) {
    println!(
        "injection: path={} iterations={} deferred={}",
        report.path, report.iterations, report.deferred
    );
    if let Some(reason) = &report.reason {
        println!("injection: note: {reason}");
    }

    for language in &report.languages {
        if report.deferred {
            println!(
                "injection: {} construction p50={} p95={} p99={} max={} ({} UTF-16 units)",
                language.language,
                format_ns(language.inject_latency.p50_ns),
                format_ns(language.inject_latency.p95_ns),
                format_ns(language.inject_latency.p99_ns),
                format_ns(language.inject_latency.max_ns),
                language.text_utf16_units
            );
        } else {
            let readback = language
                .readback_latency
                .as_ref()
                .map(|summary| {
                    format!(
                        " readback_p50={} readback_p99={}",
                        format_ns(summary.p50_ns),
                        format_ns(summary.p99_ns)
                    )
                })
                .unwrap_or_default();
            println!(
                "injection: {} method={} correct={}/{} failed={} inject_p50={} inject_p95={} inject_p99={} inject_max={}{}",
                language.language,
                language.method,
                language.correct,
                report.iterations,
                language.failed,
                format_ns(language.inject_latency.p50_ns),
                format_ns(language.inject_latency.p95_ns),
                format_ns(language.inject_latency.p99_ns),
                format_ns(language.inject_latency.max_ns),
                readback
            );
        }
    }

    let status = if report.deferred {
        "deferred"
    } else if report.all_correct {
        "ok"
    } else {
        "failed"
    };
    println!("injection: result={status}");
}

fn format_ns(nanoseconds: u64) -> String {
    if nanoseconds < 1_000 {
        format!("{nanoseconds}ns")
    } else if nanoseconds < 1_000_000 {
        format!("{:.2}us", nanoseconds as f64 / 1_000.0)
    } else {
        format!("{:.2}ms", nanoseconds as f64 / 1_000_000.0)
    }
}

fn default_warmup(iterations: usize) -> usize {
    (iterations / 10).clamp(10, 500).min(iterations)
}

#[derive(Debug, Clone)]
struct Options {
    ranking: bool,
    candidates: bool,
    suggestions: bool,
    injection: bool,
    smoke: bool,
    sendinput: bool,
    json: bool,
    csv: bool,
    iterations: Option<usize>,
    warmup: Option<usize>,
    seed: u64,
    help: bool,
    version: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ranking: false,
            candidates: false,
            suggestions: false,
            injection: false,
            smoke: false,
            sendinput: false,
            json: false,
            csv: false,
            iterations: None,
            warmup: None,
            seed: DEFAULT_SEED,
            help: false,
            version: false,
        }
    }
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Options, BenchError> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--ranking" => options.ranking = true,
            "--candidates" => options.candidates = true,
            "--suggestions" => options.suggestions = true,
            "--injection" => options.injection = true,
            "--smoke" => options.smoke = true,
            "--sendinput" => options.sendinput = true,
            "--json" => options.json = true,
            "--csv" => options.csv = true,
            "--help" | "-h" => {
                options.help = true;
                return Ok(options);
            }
            "--version" => {
                options.version = true;
                return Ok(options);
            }
            "--iterations" => {
                let value = take_value(&mut arguments, "--iterations")?;
                options.iterations = Some(parse_count(&value, "--iterations")?);
            }
            "--warmup" => {
                let value = take_value(&mut arguments, "--warmup")?;
                options.warmup = Some(parse_count_allow_zero(&value, "--warmup")?);
            }
            "--seed" => {
                let value = take_value(&mut arguments, "--seed")?;
                options.seed = parse_seed(&value)?;
            }
            _ if argument.starts_with("--iterations=") => {
                let value = inline_value(&argument, "--iterations")?;
                options.iterations = Some(parse_count(value, "--iterations")?);
            }
            _ if argument.starts_with("--warmup=") => {
                let value = inline_value(&argument, "--warmup")?;
                options.warmup = Some(parse_count_allow_zero(value, "--warmup")?);
            }
            _ if argument.starts_with("--seed=") => {
                let value = inline_value(&argument, "--seed")?;
                options.seed = parse_seed(value)?;
            }
            other => {
                return Err(BenchError::Usage(format!("unknown argument: {other}")));
            }
        }
    }

    if options.json && options.csv {
        return Err(BenchError::Usage(
            "--json and --csv are mutually exclusive".to_owned(),
        ));
    }
    if !options.ranking && !options.candidates && !options.injection {
        if options.smoke {
            options.ranking = true;
        } else {
            return Err(BenchError::Usage(
                "select --ranking, --candidates, --suggestions, --injection, or --smoke".to_owned(),
            ));
        }
    }

    Ok(options)
}

fn take_value(
    arguments: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, BenchError> {
    arguments
        .next()
        .ok_or_else(|| BenchError::Usage(format!("{flag} requires a value")))
}

fn inline_value<'a>(argument: &'a str, flag: &str) -> Result<&'a str, BenchError> {
    argument
        .split_once('=')
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BenchError::Usage(format!("{flag} requires a value")))
}

fn parse_count(value: &str, flag: &str) -> Result<usize, BenchError> {
    let count = parse_count_allow_zero(value, flag)?;
    if count == 0 {
        Err(BenchError::Usage(format!(
            "{flag} must be greater than zero"
        )))
    } else {
        Ok(count)
    }
}

fn parse_count_allow_zero(value: &str, flag: &str) -> Result<usize, BenchError> {
    value
        .parse::<usize>()
        .map_err(|_| BenchError::Usage(format!("{flag} expects a non-negative integer")))
}

fn parse_seed(value: &str) -> Result<u64, BenchError> {
    let (digits, radix) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or((value, 10), |digits| (digits, 16));
    u64::from_str_radix(digits, radix)
        .map_err(|_| BenchError::Usage("--seed expects an unsigned 64-bit integer".to_owned()))
}

fn print_help() {
    println!(
        "Okkhor {} benchmark harness\n\n\
         USAGE:\n\
         \x20   okkhor-bench --ranking [--iterations N] [--warmup N] [--seed N] [--json|--csv] [--smoke]\n\
         \x20   okkhor-bench --candidates [--iterations N] [--warmup N] [--json|--csv] [--smoke]\n\
         \x20   okkhor-bench --suggestions [--iterations N] [--warmup N] [--smoke]\n\
         \x20   okkhor-bench --injection [--iterations N] [--sendinput] [--json|--csv]\n\n\
         MODES:\n\
         \x20   --ranking      Run N Smriti::rank calls over synthetic English/Bangla cases.\n\
         \x20   --candidates   Run N CandidateEngine::suggest calls over the committed\n\
         \x20                  synthetic golden corpus; report top-1/top-5 hit rate plus rank latency.\n\
         \x20   --suggestions  Run the live 0.6 engine over everyday words; report top-1/top-5\n\
         \x20                  and suggestion latency.\n\
         \x20   --injection    Inject synthetic text into a private Win32 EDIT window and read it back.\n\
         \x20   --smoke        Short CI-friendly run; validates the percentile report\n\
         \x20                  (ranking default; one-case subset with `--candidates`).\n\n\
         OPTIONS:\n\
         \x20   --sendinput    Force the pure Unicode SendInput path instead of the native WM_CHAR fast path.\n\
         \x20   --iterations N  Measured iterations (default: 5000 ranking, 2000 candidates, 20 injection; 100/10/5 with --smoke).\n\
         \x20   --warmup N      Untimed ranking/candidates iterations (default: 10% of measured, clamped to 10..=500).\n\
         \x20   --seed N        Synthetic corpus seed (default: {}).\n\
         \x20   --json | --csv  Write a report under bench-out/ (git-ignored).\n\n\
         SAFETY:\n\
         \x20   No global keyboard hook. No network. No user data. The injection mode never\n\
         \x20   selects the clipboard fallback; if it cannot focus its own non-elevated window,\n\
         \x20   it reports why and runs the event-construction benchmark instead.",
        env!("CARGO_PKG_VERSION"),
        DEFAULT_SEED
    );
}

#[derive(Debug)]
enum BenchError {
    Usage(String),
    Io(io::Error),
    Failed(String),
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Failed(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for BenchError {}

impl From<io::Error> for BenchError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::BenchError;
    use super::{DEFAULT_SEED, default_warmup, parse_args};

    fn parse(values: &[&str]) -> Result<super::Options, BenchError> {
        parse_args(values.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn ranking_is_the_smoke_default() {
        let options = parse(&["--smoke"]).expect("valid smoke arguments");
        assert!(options.smoke);
        assert!(options.ranking);
        assert!(!options.injection);
        assert_eq!(options.seed, DEFAULT_SEED);
    }

    #[test]
    fn reads_iteration_warmup_and_seed_values() {
        let options = parse(&[
            "--ranking",
            "--iterations",
            "123",
            "--warmup=7",
            "--seed",
            "42",
            "--json",
        ])
        .expect("valid arguments");
        assert_eq!(options.iterations, Some(123));
        assert_eq!(options.warmup, Some(7));
        assert_eq!(options.seed, 42);
        assert!(options.json);
        assert!(!options.csv);

        let hexadecimal = parse(&["--ranking", "--seed=0x10"]).expect("valid hex seed");
        assert_eq!(hexadecimal.seed, 16);
    }

    #[test]
    fn candidates_mode_parses_with_iterations_and_format() {
        let options = parse(&["--candidates", "--iterations", "2000", "--json"])
            .expect("valid candidates arguments");
        assert!(options.candidates);
        assert!(!options.ranking);
        assert!(!options.injection);
        assert_eq!(options.iterations, Some(2000));
        assert!(options.json);
        let smoke = parse(&["--candidates", "--smoke"]).expect("valid candidates smoke arguments");
        assert!(smoke.candidates);
        assert!(smoke.smoke);
    }

    #[test]
    fn rejects_invalid_combinations() {
        assert!(parse(&["--ranking", "--json", "--csv"]).is_err());
        assert!(parse(&["--ranking", "--iterations", "0"]).is_err());
        assert!(parse(&["--ranking", "--warmup", "0"]).is_ok());
        assert!(parse(&["--ranking", "--iterations"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn warmup_is_bounded_to_the_measured_run() {
        assert_eq!(default_warmup(100), 10);
        assert_eq!(default_warmup(5_000), 500);
        assert_eq!(default_warmup(3), 3);
    }
}
