#![forbid(unsafe_code)]
//! `okkhor-train-dataset`: verify a dataset manifest v1 against its data file.
//!
//! Offline only. No network, no user data.
//!
//! ```text
//! okkhor-train-dataset --verify crates/okkhor-train/data/fixtures/mini-corpus.manifest.json
//! ```

use std::process::ExitCode;

use okkhor_train::verify_manifest;

const USAGE: &str = "usage: okkhor-train-dataset --verify <manifest>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(manifest_path) = flag_value(&args, "--verify") else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match verify_manifest(&manifest_path) {
        Ok(verified) => {
            let manifest = &verified.manifest;
            let languages = manifest.languages.join(",");
            let short_sha = &verified.sha256[..16.min(verified.sha256.len())];
            println!(
                "ok {} version={} split={} rows={} languages={} license={} source={} sha256={short_sha}.. file={}",
                manifest.name,
                manifest.version,
                manifest.split.as_str(),
                verified.rows,
                languages,
                manifest.license,
                manifest.source,
                verified.data_path.display(),
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("invalid dataset: {error}");
            ExitCode::FAILURE
        }
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}
