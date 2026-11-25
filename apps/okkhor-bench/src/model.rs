//! Latency and load benchmark for an exported `OKNM` next-word model.

use std::fs;
use std::time::Instant;

use okkhor_engine::nextword::NextWordModel;

pub struct ModelReport {
    pub vocab: usize,
    pub context: usize,
    pub load_ms: f64,
    pub samples: Vec<f64>,
    pub top: Vec<(u32, f32)>,
}

impl ModelReport {
    pub fn percentile(&self, percent: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let rank = ((percent / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
    }
}

pub fn run(path: &str, iterations: usize) -> Result<ModelReport, String> {
    let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());

    let started = Instant::now();
    let model = NextWordModel::parse(leaked).map_err(|error| error.to_string())?;
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let context = model.context_len();

    let mut state: u64 = 0x4F4B_4B48;
    let mut samples = Vec::with_capacity(iterations);
    let mut top = Vec::new();
    for _ in 0..iterations {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let ids: Vec<u32> = (0..context)
            .map(|index| ((state >> (index * 3)) as u32 % 1000) % model.vocab_len() as u32)
            .collect();
        let started = Instant::now();
        let ranked = model.predict(&ids, 5);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        top = ranked;
    }

    Ok(ModelReport {
        vocab: model.vocab_len(),
        context,
        load_ms,
        samples,
        top,
    })
}
