#![forbid(unsafe_code)]
//! `okkhor-train-fit`: fit the deterministic Tier 1 logistic reranker from a
//! replay JSONL file and write the model JSON.
//!
//! Offline only. No network, no user data. Deterministic for a fixed seed,
//! replay, and `--trained-at`.
//!
//! ```text
//! okkhor-train-fit --replay crates/okkhor-train/data/synthetic-replay.jsonl \
//!   --out bench-out/reranker.json [--seed 20261123] [--trained-at 1970-01-01T00:00:00Z]
//! ```

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use okkhor_train::{DEFAULT_SEED, DEFAULT_TRAINED_AT, TrainOptions, fit_replay};

const USAGE: &str = "usage: okkhor-train-fit --replay <path> --out <model.json> [--seed <u64>] [--trained-at <iso8601>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("okkhor-train-fit: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let replay_path =
        flag_value(args, "--replay").ok_or_else(|| "missing --replay <path>".to_owned())?;
    let out_path =
        flag_value(args, "--out").ok_or_else(|| "missing --out <model.json>".to_owned())?;
    let seed = match flag_value(args, "--seed") {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| "invalid --seed; want u64".to_owned())?,
        None => DEFAULT_SEED,
    };
    let trained_at =
        flag_value(args, "--trained-at").unwrap_or_else(|| DEFAULT_TRAINED_AT.to_owned());
    if trained_at.trim().is_empty() {
        return Err("--trained-at must not be blank".to_owned());
    }

    let document = fs::read_to_string(&replay_path)
        .map_err(|error| format!("cannot read {replay_path}: {error}"))?;
    let options = TrainOptions::default()
        .with_seed(seed)
        .with_trained_at(trained_at);
    let model = fit_replay(&document, &options).map_err(|error| error.to_string())?;
    let mut json = model.to_json().map_err(|error| error.to_string())?;
    json.push('\n');

    let path = PathBuf::from(&out_path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(&path, json).map_err(|error| format!("cannot write {}: {error}", path.display()))?;

    let metrics = &model.metrics;
    println!(
        "fitted {} v{} seed={seed} train={} val={} skipped={} top1={:.4} top5={:.4} mrr={:.4} loss={:.4} dataset_sha256={} -> {}",
        model.name,
        model.version,
        metrics.train_cases,
        metrics.validation_cases,
        metrics.skipped_cases,
        metrics.validation_top1,
        metrics.validation_top5,
        metrics.validation_mrr,
        metrics.final_loss,
        &model.dataset_sha256[..16.min(model.dataset_sha256.len())],
        path.display(),
    );
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}
