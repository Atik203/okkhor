#![forbid(unsafe_code)]
//! `okkhor-replay`: deterministic synthetic replay generator and evaluator.
//!
//! Offline only. No network, no user data. Synthetic fixtures only.
//!
//! ```text
//! okkhor-replay --generate --out crates/okkhor-train/data/synthetic-replay.jsonl --seed 20261123 [--cases 240]
//! okkhor-replay --replay <path> --engine [--json | --markdown] [--out <report>]
//! okkhor-replay --replay <path> --smriti [--json | --markdown] [--out <report>]
//! ```

use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use okkhor_core::{Candidate, CandidateSource, Language, Ranker, SuggestionRequest};
use okkhor_engine::{CandidateEngine, EngineConfig};
use okkhor_lang::{Dictionary, DictionaryEntry};
use okkhor_learn::Smriti;
use okkhor_train::{
    RankedOutcome, ReplayCase, compute_metrics, generate_cases, read_replay_str, render_jsonl,
    sha256_hex,
};
use okkhor_transliterate::{RULE_SCHEMA, RuleId, RuleSet, RuleTransliterator, TransliterationRule};

const DEFAULT_CASES: usize = 240;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("okkhor-replay: {message}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "usage:\n  okkhor-replay --generate --out <path> --seed <u64> [--cases <n>]\n  okkhor-replay --replay <path> (--engine | --smriti) [--json | --markdown] [--out <report>]";

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.contains(&"--generate".to_owned()) {
        let out = flag_value(&args, "--out").ok_or("missing --out <path>")?;
        let seed = flag_value(&args, "--seed")
            .ok_or("missing --seed <u64>")?
            .parse::<u64>()
            .map_err(|_| "invalid --seed; want u64".to_owned())?;
        let cases = flag_value(&args, "--cases")
            .map(|raw| {
                raw.parse::<usize>()
                    .map_err(|_| "invalid --cases".to_owned())
            })
            .transpose()?
            .unwrap_or(DEFAULT_CASES);
        if cases < 200 {
            return Err("refusing to generate < 200 cases".to_owned());
        }
        return generate(&out, seed, cases);
    }
    if let Some(replay) = flag_value(&args, "--replay") {
        let ranker = if args.contains(&"--engine".to_owned()) {
            RankerKind::Engine
        } else if args.contains(&"--smriti".to_owned()) {
            RankerKind::Smriti
        } else {
            return Err("missing --engine or --smriti".to_owned());
        };
        let format = if args.contains(&"--json".to_owned()) {
            ReportFormat::Json
        } else {
            ReportFormat::Markdown
        };
        let out = flag_value(&args, "--out");
        return replay_file(&replay, ranker, format, out.as_deref());
    }
    Err("missing --generate or --replay".to_owned())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn generate(out: &str, seed: u64, cases: usize) -> Result<(), String> {
    let replay_cases = generate_cases(seed, cases).map_err(|e| e.to_string())?;
    let document = render_jsonl(&replay_cases);
    let path = PathBuf::from(out);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create dir: {e}"))?;
    }
    fs::write(&path, &document).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!(
        "generated {} cases seed={seed} sha256={} -> {}",
        replay_cases.len(),
        sha256_hex(document.as_bytes()),
        path.display()
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RankerKind {
    Engine,
    Smriti,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportFormat {
    Json,
    Markdown,
}

fn language_name(language: Language) -> &'static str {
    match language {
        Language::Bangla => "bangla",
        Language::English => "english",
    }
}

fn build_dictionaries(cases: &[ReplayCase]) -> Vec<Dictionary> {
    let mut bn: Vec<String> = Vec::new();
    let mut en: Vec<String> = Vec::new();
    for case in cases {
        let pool = case
            .expected
            .iter()
            .chain(case.candidates.iter())
            .chain(case.shown.iter());
        for word in pool {
            match case.language {
                Language::Bangla => {
                    if !bn.iter().any(|w| w == word) {
                        bn.push(word.clone());
                    }
                }
                Language::English => {
                    if !en.iter().any(|w| w == word) {
                        en.push(word.clone());
                    }
                }
            }
        }
    }
    bn.sort();
    en.sort();
    vec![
        dictionary("replay-bn", Language::Bangla, &bn),
        dictionary("replay-en", Language::English, &en),
    ]
}

fn dictionary(id: &str, language: Language, words: &[String]) -> Dictionary {
    let mut entries: Vec<DictionaryEntry> = words
        .iter()
        .map(|word| DictionaryEntry {
            word: word.clone(),
            frequency: 1,
        })
        .collect();
    entries.sort_by(|a, b| a.word.cmp(&b.word));
    entries.dedup_by(|a, b| a.word == b.word);
    Dictionary {
        id: id.to_owned(),
        language,
        entries,
    }
}

fn build_transliterator() -> RuleTransliterator {
    // Fixed synthetic roman -> Bangla rules covering the generator's roman
    // tokens. Deterministic; not tuned on any real corpus.
    let pairs: &[(&str, &str, u16)] = &[
        ("aami", "আমি", 0),
        ("tumi", "তুমি", 0),
        ("bha", "ভালো", 0),
        ("bhalo", "ভালো", 0),
        ("bhai", "ভাই", 0),
        ("bhaat", "ভাত", 0),
        ("bhasha", "ভাষা", 0),
        ("boi", "বই", 0),
        ("kotha", "কথা", 0),
        ("gaan", "গান", 0),
        ("bondhu", "বন্ধু", 0),
        ("shomoy", "সময়", 0),
        ("paani", "পানি", 0),
        ("baari", "বাড়ি", 0),
        ("shohor", "শহর", 0),
        ("alo", "আলো", 0),
        ("notun", "নতুন", 0),
        ("b", "ব", 100),
    ];
    let rules: Vec<TransliterationRule> = pairs
        .iter()
        .enumerate()
        .map(|(index, (input, output, priority))| TransliterationRule {
            id: RuleId::from_code(&format!("r-{index:02}")).expect("synthetic rule id is valid"),
            input: (*input).to_owned(),
            output: (*output).to_owned(),
            language: Language::Bangla,
            priority: *priority,
        })
        .collect();
    RuleTransliterator::new(RuleSet {
        schema: RULE_SCHEMA,
        rules,
    })
    .expect("synthetic transliteration rules are valid")
}

fn request_for(case: &ReplayCase) -> SuggestionRequest {
    let roman = match case.language {
        Language::Bangla if case.prefix.is_ascii() && !case.prefix.is_empty() => {
            Some(case.prefix.clone())
        }
        _ => None,
    };
    SuggestionRequest {
        prefix: case.prefix.clone(),
        language: case.language,
        layout: case.layout.clone(),
        context: case.context.clone(),
        roman_input: roman,
        now_ms: 0,
        limit: 5,
    }
}

fn replay_file(
    replay_path: &str,
    ranker: RankerKind,
    format: ReportFormat,
    out: Option<&str>,
) -> Result<(), String> {
    let document =
        fs::read_to_string(replay_path).map_err(|e| format!("cannot read {replay_path}: {e}"))?;
    let dataset_sha = sha256_hex(document.as_bytes());
    let cases = read_replay_str(&document).map_err(|e| format!("invalid replay: {e}"))?;

    let dictionaries = build_dictionaries(&cases);
    let transliterator = build_transliterator();
    let smriti = Smriti::new();
    let config = EngineConfig::default();
    let engine = CandidateEngine::new(&dictionaries, &transliterator, &smriti, config)
        .map_err(|e| format!("invalid engine: {e}"))?;

    let mut outcomes = Vec::with_capacity(cases.len());
    for case in &cases {
        let request = request_for(case);
        let started = Instant::now();
        let ranked: Vec<String> = match ranker {
            RankerKind::Engine => engine
                .suggest(&request)
                .into_iter()
                .map(|s| s.text)
                .collect(),
            RankerKind::Smriti => {
                let pool: Vec<Candidate> = case
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        let mut candidate = Candidate::new(
                            text.clone(),
                            case.language,
                            CandidateSource::Dictionary,
                        );
                        candidate.features.base_score = 1.0 / (1.0 + index as f32);
                        candidate
                    })
                    .collect();
                smriti
                    .rank(&request, pool)
                    .into_iter()
                    .map(|s| s.text)
                    .collect()
            }
        };
        let latency = started.elapsed();
        black_box(&ranked);
        outcomes.push(RankedOutcome {
            case_id: case.case_id.clone(),
            language: language_name(case.language).to_owned(),
            layout: case.layout.clone().unwrap_or_else(|| "none".to_owned()),
            prefix_len_chars: case.prefix.chars().count(),
            expected_len_chars: case.expected.first().map_or(0, |e| e.chars().count()),
            expected: case.expected.clone(),
            ranked,
            accepted: case.accepted.is_some(),
            latency_ns: u64::try_from(latency.as_nanos()).unwrap_or(u64::MAX),
        });
    }

    let metrics = compute_metrics(&outcomes).map_err(|e| e.to_string())?;
    let ranker_name = match ranker {
        RankerKind::Engine => "engine",
        RankerKind::Smriti => "smriti",
    };
    let report = match format {
        ReportFormat::Json => {
            render_json_report(&cases, &outcomes, &metrics, ranker_name, &dataset_sha)
        }
        ReportFormat::Markdown => {
            render_markdown_report(&cases, &outcomes, &metrics, ranker_name, &dataset_sha)
        }
    };

    if let Some(out_path) = out {
        let path = PathBuf::from(out_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create dir: {e}"))?;
        }
        fs::write(&path, &report).map_err(|e| format!("cannot write report: {e}"))?;
        println!("wrote {}", path.display());
    } else {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        let ext = match format {
            ReportFormat::Json => "json",
            ReportFormat::Markdown => "md",
        };
        let default_path = PathBuf::from(format!("bench-out/replay-{ranker_name}-{stamp}.{ext}"));
        fs::create_dir_all(Path::new("bench-out"))
            .map_err(|e| format!("cannot create bench-out: {e}"))?;
        fs::write(&default_path, &report).map_err(|e| format!("cannot write report: {e}"))?;
        println!("{}", render_summary(&metrics, ranker_name, &dataset_sha));
        println!("wrote {}", default_path.display());
    }
    Ok(())
}

fn render_summary(
    metrics: &okkhor_train::ReplayMetrics,
    ranker: &str,
    dataset_sha: &str,
) -> String {
    format!(
        "ranker={ranker} cases={} top1={:.4} top5={:.4} mrr={:.4} accept={:.4} keystroke={:.4} p50={}ns p95={}ns p99={}ns dataset_sha={}",
        metrics.cases,
        metrics.overall.top1,
        metrics.overall.top5,
        metrics.overall.mrr,
        metrics.overall.acceptance_rate,
        metrics.overall.keystroke_savings,
        metrics.overall.latency.p50_ns,
        metrics.overall.latency.p95_ns,
        metrics.overall.latency.p99_ns,
        &dataset_sha[..16.min(dataset_sha.len())],
    )
}

fn render_json_report(
    cases: &[ReplayCase],
    outcomes: &[RankedOutcome],
    metrics: &okkhor_train::ReplayMetrics,
    ranker: &str,
    dataset_sha: &str,
) -> String {
    let mut by_language = serde_json::Map::new();
    for (key, seg) in &metrics.by_language {
        by_language.insert(key.clone(), segment_json(seg));
    }
    let mut by_layout = serde_json::Map::new();
    for (key, seg) in &metrics.by_layout {
        by_layout.insert(key.clone(), segment_json(seg));
    }
    let value = serde_json::json!({
        "schema": 1,
        "ranker": ranker,
        "cases": cases.len(),
        "dataset_sha256": dataset_sha,
        "synthetic_only": true,
        "overall": segment_json(&metrics.overall),
        "by_language": by_language,
        "by_layout": by_layout,
        "outcomes": outcomes.iter().map(|o| serde_json::json!({
            "case_id": o.case_id,
            "language": o.language,
            "layout": o.layout,
            "top1": o.top1_hit(),
            "top5": o.top5_hit(),
            "mrr": o.reciprocal_rank(),
            "latency_ns": o.latency_ns,
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).expect("report serializes")
}

fn segment_json(seg: &okkhor_train::SegmentMetrics) -> serde_json::Value {
    serde_json::json!({
        "cases": seg.cases,
        "top1": seg.top1,
        "top5": seg.top5,
        "mrr": seg.mrr,
        "acceptance_rate": seg.acceptance_rate,
        "keystroke_savings": seg.keystroke_savings,
        "latency": {
            "count": seg.latency.count,
            "p50_ns": seg.latency.p50_ns,
            "p95_ns": seg.latency.p95_ns,
            "p99_ns": seg.latency.p99_ns,
            "max_ns": seg.latency.max_ns,
        },
    })
}

fn render_markdown_report(
    cases: &[ReplayCase],
    outcomes: &[RankedOutcome],
    metrics: &okkhor_train::ReplayMetrics,
    ranker: &str,
    dataset_sha: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Replay report ({ranker})\n\n"));
    out.push_str(&format!(
        "Cases: {} · dataset sha256: `{dataset_sha}` · **synthetic-only, not a real-world quality claim**.\n\n",
        cases.len(),
    ));
    out.push_str("| Metric | Overall |\n|---|---:|\n");
    out.push_str(&format!("| Top-1 | {:.4} |\n", metrics.overall.top1));
    out.push_str(&format!("| Top-5 | {:.4} |\n", metrics.overall.top5));
    out.push_str(&format!("| MRR | {:.4} |\n", metrics.overall.mrr));
    out.push_str(&format!(
        "| Acceptance | {:.4} |\n",
        metrics.overall.acceptance_rate
    ));
    out.push_str(&format!(
        "| Keystroke savings | {:.4} |\n",
        metrics.overall.keystroke_savings
    ));
    out.push_str(&format!(
        "| Rank p50 | {} ns |\n",
        metrics.overall.latency.p50_ns
    ));
    out.push_str(&format!(
        "| Rank p95 | {} ns |\n",
        metrics.overall.latency.p95_ns
    ));
    out.push_str(&format!(
        "| Rank p99 | {} ns |\n",
        metrics.overall.latency.p99_ns
    ));
    out.push_str(
        "\n## By language\n\n| Language | n | Top-1 | Top-5 | MRR |\n|---|---:|---:|---:|---:|\n",
    );
    for (key, seg) in &metrics.by_language {
        out.push_str(&format!(
            "| {key} | {} | {:.4} | {:.4} | {:.4} |\n",
            seg.cases, seg.top1, seg.top5, seg.mrr
        ));
    }
    out.push_str(
        "\n## By layout\n\n| Layout | n | Top-1 | Top-5 | MRR |\n|---|---:|---:|---:|---:|\n",
    );
    for (key, seg) in &metrics.by_layout {
        out.push_str(&format!(
            "| {key} | {} | {:.4} | {:.4} | {:.4} |\n",
            seg.cases, seg.top1, seg.top5, seg.mrr
        ));
    }
    out.push_str("\n## Cases\n\n| case_id | lang | layout | top-1 | top-5 | latency ns |\n|---|---|---|:---:|:---:|---:|\n");
    for outcome in outcomes {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            outcome.case_id,
            outcome.language,
            outcome.layout,
            if outcome.top1_hit() { "hit" } else { "miss" },
            if outcome.top5_hit() { "hit" } else { "miss" },
            outcome.latency_ns,
        ));
    }
    let _ = cases;
    out
}
