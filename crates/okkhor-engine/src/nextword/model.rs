//! `OKNM` v1 next-word model: parse, validate, run, and rank.
//!
//! The container is written by `scripts/export-nextword-model.py`:
//!
//! ```text
//! magic "OKNM" | u16 version | u32 header_len | header JSON | pad to 64
//! vocabulary blob (UTF-8 words, newline separated, id order)
//! tensor blocks, each aligned to 64 bytes
//! ```
//!
//! Tensors are read straight from the embedded blob, so a loaded model costs
//! the file size and nothing more. Inference is pure safe Rust on the daemon
//! worker thread; it never runs in the keyboard hook.

use std::collections::HashMap;

use serde::Deserialize;

use okkhor_core::Language;

const MAGIC: &[u8; 4] = b"OKNM";
const VERSION: u16 = 1;
const PREFIX_LEN: usize = 10;
const ALIGN: usize = 64;
const LAYER_NORM_EPS: f32 = 1e-5;
const MAX_CONTEXT: usize = 64;
const MAX_VOCAB: usize = 1_000_000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("next-word model is truncated")]
    Truncated,
    #[error("next-word model magic is unsupported")]
    BadMagic,
    #[error("next-word model version is unsupported (this build reads OKNM v1)")]
    BadVersion,
    #[error("next-word model header is invalid")]
    BadHeader,
    #[error("next-word model tensor layout is invalid")]
    BadTensor,
    #[error("next-word model contains invalid UTF-8")]
    BadText,
}

#[derive(Debug, Deserialize)]
struct Header {
    version: u16,
    language: String,
    context: usize,
    vocab: usize,
    embed: usize,
    d_model: usize,
    layers: usize,
    heads: usize,
    ff: usize,
    vocab_offset: usize,
    vocab_bytes: usize,
    tensors: Vec<TensorEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct TensorEntry {
    name: String,
    shape: Vec<usize>,
    dtype: String,
    offset: usize,
    bytes: usize,
}

/// One parsed, read-only model.
pub struct NextWordModel {
    blob: &'static [u8],
    prefix_len: usize,
    language: Language,
    context: usize,
    vocab: usize,
    embed: usize,
    d_model: usize,
    layers: usize,
    heads: usize,
    ff: usize,
    words: Vec<&'static str>,
    ids: HashMap<&'static str, u32>,
    tensors: HashMap<String, TensorEntry>,
}

impl std::fmt::Debug for NextWordModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NextWordModel")
            .field("language", &self.language)
            .field("context", &self.context)
            .field("vocab", &self.vocab)
            .field("d_model", &self.d_model)
            .field("layers", &self.layers)
            .finish()
    }
}

impl NextWordModel {
    /// Parse and validate a container. Every offset is bounds-checked; a
    /// corrupt blob is an error, never a panic.
    pub fn parse(blob: &'static [u8]) -> Result<Self, ModelError> {
        if blob.len() < PREFIX_LEN {
            return Err(ModelError::Truncated);
        }
        if &blob[0..4] != MAGIC {
            return Err(ModelError::BadMagic);
        }
        let version = u16_at(blob, 4).ok_or(ModelError::Truncated)?;
        if version != VERSION {
            return Err(ModelError::BadVersion);
        }
        let header_len = u32_at(blob, 6).ok_or(ModelError::Truncated)? as usize;
        let header_end = PREFIX_LEN
            .checked_add(header_len)
            .ok_or(ModelError::Truncated)?;
        if header_end > blob.len() {
            return Err(ModelError::Truncated);
        }
        let header: Header = serde_json::from_slice(&blob[PREFIX_LEN..header_end])
            .map_err(|_| ModelError::BadHeader)?;
        if header.version != VERSION
            || !(1..=MAX_CONTEXT).contains(&header.context)
            || header.vocab == 0
            || header.vocab > MAX_VOCAB
            || header.embed == 0
            || header.d_model == 0
            || header.heads == 0
            || header.d_model % header.heads != 0
            || header.layers == 0
            || header.ff == 0
        {
            return Err(ModelError::BadHeader);
        }
        let language = match header.language.as_str() {
            "bn" => Language::Bangla,
            "en" => Language::English,
            _ => return Err(ModelError::BadHeader),
        };
        let prefix_len = align(header_end);

        let vocab_start = prefix_len
            .checked_add(header.vocab_offset)
            .ok_or(ModelError::Truncated)?;
        let vocab_end = vocab_start
            .checked_add(header.vocab_bytes)
            .ok_or(ModelError::Truncated)?;
        if vocab_end > blob.len() {
            return Err(ModelError::Truncated);
        }
        let vocab_text =
            std::str::from_utf8(&blob[vocab_start..vocab_end]).map_err(|_| ModelError::BadText)?;
        let words: Vec<&'static str> = vocab_text.split('\n').collect();
        if words.len() != header.vocab {
            return Err(ModelError::BadHeader);
        }
        let mut ids: HashMap<&'static str, u32> = HashMap::with_capacity(words.len());
        for (index, word) in words.iter().enumerate() {
            ids.insert(word, index as u32);
        }

        let mut tensors = HashMap::with_capacity(header.tensors.len());
        for entry in &header.tensors {
            let start = prefix_len
                .checked_add(entry.offset)
                .ok_or(ModelError::Truncated)?;
            let end = start
                .checked_add(entry.bytes)
                .ok_or(ModelError::Truncated)?;
            if end > blob.len() {
                return Err(ModelError::Truncated);
            }
            let width = match entry.dtype.as_str() {
                "f32" => 4,
                "i8" => 1,
                _ => return Err(ModelError::BadTensor),
            };
            let count: usize = entry
                .shape
                .iter()
                .try_fold(1usize, |acc, dim| acc.checked_mul(*dim))
                .ok_or(ModelError::BadTensor)?;
            if count.checked_mul(width) != Some(entry.bytes) {
                return Err(ModelError::BadTensor);
            }
            tensors.insert(entry.name.clone(), entry.clone());
        }

        let model = Self {
            blob,
            prefix_len,
            language,
            context: header.context,
            vocab: header.vocab,
            embed: header.embed,
            d_model: header.d_model,
            layers: header.layers,
            heads: header.heads,
            ff: header.ff,
            words,
            ids,
            tensors,
        };
        model.check_layout()?;
        Ok(model)
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn context_len(&self) -> usize {
        self.context
    }

    pub fn vocab_len(&self) -> usize {
        self.vocab
    }

    /// Word for an id, or `<unk>` when out of range.
    pub fn word(&self, id: u32) -> &'static str {
        self.words.get(id as usize).copied().unwrap_or("<unk>")
    }

    /// Id for a word, or `<unk>` (1) when the vocabulary does not cover it.
    pub fn id(&self, word: &str) -> u32 {
        self.ids.get(word).copied().unwrap_or(1)
    }

    /// Most likely next words with softmax probabilities, best first.
    ///
    /// `context` is the recent committed words, oldest first; it is left-padded
    /// with `<pad>` to the trained context length.
    pub fn predict(&self, context: &[u32], limit: usize) -> Vec<(u32, f32)> {
        if limit == 0 || self.vocab == 0 {
            return Vec::new();
        }
        let logits = self.logits(context);
        let max = max_of(&logits);
        let mut sum = 0.0f64;
        for value in &logits {
            sum += f64::from((value - max).exp());
        }
        let sum = if sum > 0.0 { sum as f32 } else { 1.0 };
        let mut ranked: Vec<(u32, f32)> = logits
            .iter()
            .enumerate()
            .map(|(id, value)| (id as u32, (value - max).exp() / sum))
            .collect();
        ranked.sort_unstable_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(limit.min(self.vocab));
        ranked
    }

    /// Raw logits for every vocabulary entry.
    pub fn logits(&self, context: &[u32]) -> Vec<f32> {
        let hidden = self.encode(context);
        let output = self.tensor("out.weight");
        let scale = self.tensor("out.weight.scale");
        let bias = self.tensor("out.bias");
        let width = self.d_model;
        let mut logits = vec![0.0f32; self.vocab];
        if !scale.is_empty() {
            okkhor_simd::project_i8_f32(output, width, &hidden, &mut logits);
            for (id, slot) in logits.iter_mut().enumerate() {
                *slot = *slot * read_f32(scale, id) + read_f32(bias, id);
            }
        } else {
            okkhor_simd::project_f32_bytes(output, width, &hidden, &mut logits);
            for (id, slot) in logits.iter_mut().enumerate() {
                *slot += read_f32(bias, id);
            }
        }
        logits
    }

    /// Final hidden state for the last position.
    fn encode(&self, context: &[u32]) -> Vec<f32> {
        let d = self.d_model;
        let embed = self.embed;
        let mut x = vec![0.0f32; self.context * d];
        let emb = self.tensor("emb.weight");
        let proj_w = self.tensor("proj.weight");
        let proj_b = self.tensor("proj.bias");
        let pad = self.context.saturating_sub(context.len());
        let mut staged = vec![0.0f32; embed];
        for position in 0..self.context {
            let id = if position < pad {
                0
            } else {
                context[position - pad] as usize
            };
            let id = id.min(self.vocab.saturating_sub(1));
            let start = id * embed * 4;
            for (index, slot) in staged.iter_mut().enumerate() {
                *slot = read_f32(emb, start / 4 + index);
            }
            let row = &mut x[position * d..(position + 1) * d];
            okkhor_simd::project_f32_bytes(proj_w, embed, &staged, row);
            for (out, slot) in row.iter_mut().enumerate() {
                *slot += read_f32(proj_b, out);
            }
        }

        for layer in 0..self.layers {
            self.layer(layer, &mut x);
        }
        x[(self.context - 1) * d..].to_vec()
    }

    fn layer(&self, layer: usize, x: &mut [f32]) {
        let d = self.d_model;
        let seq = self.context;
        let prefix = format!("layers.{layer}.");
        let norm1_w = self.tensor(&format!("{prefix}norm1.weight"));
        let norm1_b = self.tensor(&format!("{prefix}norm1.bias"));
        let mut normed = vec![0.0f32; seq * d];
        for position in 0..seq {
            let start = position * d;
            layer_norm(
                &x[start..start + d],
                norm1_w,
                norm1_b,
                &mut normed[start..start + d],
            );
        }
        let attended = self.attention(layer, &normed);
        for index in 0..seq * d {
            x[index] += attended[index];
        }

        let norm2_w = self.tensor(&format!("{prefix}norm2.weight"));
        let norm2_b = self.tensor(&format!("{prefix}norm2.bias"));
        for position in 0..seq {
            let start = position * d;
            layer_norm(
                &x[start..start + d],
                norm2_w,
                norm2_b,
                &mut normed[start..start + d],
            );
        }
        let lin1_w = self.tensor(&format!("{prefix}linear1.weight"));
        let lin1_b = self.tensor(&format!("{prefix}linear1.bias"));
        let lin2_w = self.tensor(&format!("{prefix}linear2.weight"));
        let lin2_b = self.tensor(&format!("{prefix}linear2.bias"));
        let mut hidden = vec![0.0f32; self.ff];
        let mut ffn = vec![0.0f32; d];
        for position in 0..seq {
            let input = &normed[position * d..(position + 1) * d];
            okkhor_simd::project_f32_bytes(lin1_w, d, input, &mut hidden);
            for (out, slot) in hidden.iter_mut().enumerate() {
                *slot = (*slot + read_f32(lin1_b, out)).max(0.0);
            }
            okkhor_simd::project_f32_bytes(lin2_w, self.ff, &hidden, &mut ffn);
            let row = &mut x[position * d..(position + 1) * d];
            for (index, slot) in row.iter_mut().enumerate() {
                *slot += ffn[index] + read_f32(lin2_b, index);
            }
        }
    }

    fn attention(&self, layer: usize, input: &[f32]) -> Vec<f32> {
        let d = self.d_model;
        let seq = self.context;
        let heads = self.heads;
        let head_dim = d / heads;
        let prefix = format!("layers.{layer}.self_attn.");
        let in_w = self.tensor(&format!("{prefix}in_proj_weight"));
        let in_b = self.tensor(&format!("{prefix}in_proj_bias"));
        let out_w = self.tensor(&format!("{prefix}out_proj.weight"));
        let out_b = self.tensor(&format!("{prefix}out_proj.bias"));

        let mut qkv = vec![0.0f32; seq * 3 * d];
        for position in 0..seq {
            let input = &input[position * d..(position + 1) * d];
            let row = &mut qkv[position * 3 * d..(position + 1) * 3 * d];
            okkhor_simd::project_f32_bytes(in_w, d, input, row);
            for (out, slot) in row.iter_mut().enumerate() {
                *slot += read_f32(in_b, out);
            }
        }

        let scale = 1.0 / (head_dim as f32).sqrt();
        let mut context = vec![0.0f32; seq * d];
        let mut scores = vec![0.0f32; seq];
        for head in 0..heads {
            for position in 0..seq {
                let query_start = position * 3 * d + head * head_dim;
                let query = &qkv[query_start..query_start + head_dim];
                for (other, score) in scores.iter_mut().enumerate() {
                    let key_start = other * 3 * d + d + head * head_dim;
                    *score = dot_f32(&qkv[key_start..key_start + head_dim], query) * scale;
                }
                softmax(&mut scores);
                let target_start = position * d + head * head_dim;
                let target = &mut context[target_start..target_start + head_dim];
                target.fill(0.0);
                for (other, weight) in scores.iter().enumerate() {
                    let value_start = other * 3 * d + 2 * d + head * head_dim;
                    let value = &qkv[value_start..value_start + head_dim];
                    for index in 0..head_dim {
                        target[index] += weight * value[index];
                    }
                }
            }
        }

        let mut output = vec![0.0f32; seq * d];
        for position in 0..seq {
            let input = &context[position * d..(position + 1) * d];
            let row = &mut output[position * d..(position + 1) * d];
            okkhor_simd::project_f32_bytes(out_w, d, input, row);
            for (out, slot) in row.iter_mut().enumerate() {
                *slot += read_f32(out_b, out);
            }
        }
        output
    }

    fn check_layout(&self) -> Result<(), ModelError> {
        let d = self.d_model;
        let mut expected: Vec<(String, Vec<usize>)> = vec![
            ("emb.weight".to_owned(), vec![self.vocab, self.embed]),
            ("proj.weight".to_owned(), vec![d, self.embed]),
            ("proj.bias".to_owned(), vec![d]),
        ];
        for layer in 0..self.layers {
            let prefix = format!("layers.{layer}.");
            expected.push((format!("{prefix}self_attn.in_proj_weight"), vec![3 * d, d]));
            expected.push((format!("{prefix}self_attn.in_proj_bias"), vec![3 * d]));
            expected.push((format!("{prefix}self_attn.out_proj.weight"), vec![d, d]));
            expected.push((format!("{prefix}self_attn.out_proj.bias"), vec![d]));
            expected.push((format!("{prefix}linear1.weight"), vec![self.ff, d]));
            expected.push((format!("{prefix}linear1.bias"), vec![self.ff]));
            expected.push((format!("{prefix}linear2.weight"), vec![d, self.ff]));
            expected.push((format!("{prefix}linear2.bias"), vec![d]));
            expected.push((format!("{prefix}norm1.weight"), vec![d]));
            expected.push((format!("{prefix}norm1.bias"), vec![d]));
            expected.push((format!("{prefix}norm2.weight"), vec![d]));
            expected.push((format!("{prefix}norm2.bias"), vec![d]));
        }
        expected.push(("out.bias".to_owned(), vec![self.vocab]));
        for (name, shape) in expected {
            let entry = self.tensors.get(&name).ok_or(ModelError::BadTensor)?;
            if entry.shape != shape {
                return Err(ModelError::BadTensor);
            }
        }
        let output = self
            .tensors
            .get("out.weight")
            .ok_or(ModelError::BadTensor)?;
        if output.shape != vec![self.vocab, d] {
            return Err(ModelError::BadTensor);
        }
        if output.dtype == "i8" {
            let scale = self
                .tensors
                .get("out.weight.scale")
                .ok_or(ModelError::BadTensor)?;
            if scale.shape != vec![self.vocab] || scale.dtype != "f32" {
                return Err(ModelError::BadTensor);
            }
        }
        Ok(())
    }

    fn tensor(&self, name: &str) -> &'static [u8] {
        match self.tensors.get(name) {
            Some(entry) => {
                let start = self.prefix_len + entry.offset;
                &self.blob[start..start + entry.bytes]
            }
            None => &[],
        }
    }
}

fn align(position: usize) -> usize {
    position.div_ceil(ALIGN) * ALIGN
}

fn read_f32(bytes: &[u8], index: usize) -> f32 {
    let start = index * 4;
    match bytes.get(start..start + 4) {
        Some(chunk) => f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
        None => 0.0,
    }
}

fn layer_norm(input: &[f32], weight: &[u8], bias: &[u8], out: &mut [f32]) {
    let count = input.len() as f32;
    let mean = input.iter().sum::<f32>() / count;
    let variance = input
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f32>()
        / count;
    let denom = (variance + LAYER_NORM_EPS).sqrt();
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = (input[index] - mean) / denom * read_f32(weight, index) + read_f32(bias, index);
    }
}

fn softmax(values: &mut [f32]) {
    let mut max = f32::NEG_INFINITY;
    for value in values.iter() {
        if *value > max {
            max = *value;
        }
    }
    let mut sum = 0.0f32;
    for value in values.iter_mut() {
        *value = (*value - max).exp();
        sum += *value;
    }
    if sum > 0.0 {
        for value in values.iter_mut() {
            *value /= sum;
        }
    }
}

fn max_of(values: &[f32]) -> f32 {
    values.iter().copied().fold(f32::NEG_INFINITY, f32::max)
}

/// Dot product of a little-endian f32 row against an activation vector.
fn dot_f32_row(row: &[u8], right: &[f32]) -> f32 {
    let mut sums = [0.0f32; 4];
    let mut chunks = row.chunks_exact(16);
    let mut values = right.chunks_exact(4);
    for (bytes, act) in (&mut chunks).zip(&mut values) {
        sums[0] += f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) * act[0];
        sums[1] += f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) * act[1];
        sums[2] += f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) * act[2];
        sums[3] += f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) * act[3];
    }
    let mut total = sums[0] + sums[1] + sums[2] + sums[3];
    for (bytes, value) in chunks
        .remainder()
        .chunks_exact(4)
        .zip(values.remainder().iter())
    {
        total += f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) * value;
    }
    total
}

/// Dot product of an int8 row against an activation vector.
fn dot_i8_row(row: &[u8], right: &[f32]) -> f32 {
    let mut sums = [0.0f32; 4];
    let mut chunks = row.chunks_exact(4);
    let mut values = right.chunks_exact(4);
    for (bytes, act) in (&mut chunks).zip(&mut values) {
        sums[0] += f32::from(bytes[0] as i8) * act[0];
        sums[1] += f32::from(bytes[1] as i8) * act[1];
        sums[2] += f32::from(bytes[2] as i8) * act[2];
        sums[3] += f32::from(bytes[3] as i8) * act[3];
    }
    let mut total = sums[0] + sums[1] + sums[2] + sums[3];
    for (byte, value) in chunks.remainder().iter().zip(values.remainder().iter()) {
        total += f32::from(*byte as i8) * value;
    }
    total
}

/// Dot product of two f32 slices.
fn dot_f32(left: &[f32], right: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for (a, b) in left.iter().zip(right.iter()) {
        sum += a * b;
    }
    sum
}

fn u16_at(raw: &[u8], offset: usize) -> Option<u16> {
    let bytes = raw.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(raw: &[u8], offset: usize) -> Option<u32> {
    let bytes = raw.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB: &[u8] = include_bytes!("../../assets/nextword/test-model.oknm");
    const GOLDEN: &str = include_str!("../../assets/nextword/test-model.golden.json");

    fn golden() -> serde_json::Value {
        serde_json::from_str(GOLDEN).expect("golden json")
    }

    #[test]
    fn fixture_parses() {
        let model = NextWordModel::parse(BLOB).expect("fixture parses");
        assert_eq!(model.context_len(), 6);
        assert_eq!(model.vocab_len(), 48);
        assert_eq!(model.language(), Language::Bangla);
    }

    #[test]
    fn golden_logits_match() {
        let model = NextWordModel::parse(BLOB).expect("fixture parses");
        let golden = golden();
        let inputs = golden["inputs"].as_array().expect("inputs");
        let top_ids = golden["top_ids"].as_array().expect("top ids");
        let top_logits = golden["top_logits"].as_array().expect("top logits");
        for (row, input) in inputs.iter().enumerate() {
            let context: Vec<u32> = input
                .as_array()
                .expect("row")
                .iter()
                .map(|value| value.as_u64().unwrap_or(0) as u32)
                .collect();
            let logits = model.logits(&context);
            let gold_max = max_of(
                &top_logits[row]
                    .as_array()
                    .expect("scores")
                    .iter()
                    .map(|value| value.as_f64().unwrap_or(0.0) as f32)
                    .collect::<Vec<f32>>(),
            );
            let _ = gold_max;
            let expected_max = top_logits[row][0].as_f64().unwrap_or(0.0) as f32;
            let our_max = max_of(&logits);
            assert!(
                (our_max - expected_max).abs() < 5e-3,
                "row {row}: max {our_max} vs {expected_max}"
            );
            let mut ranked: Vec<u32> = (0..logits.len() as u32).collect();
            ranked.sort_unstable_by(|a, b| {
                logits[*b as usize]
                    .partial_cmp(&logits[*a as usize])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let expected: Vec<u32> = top_ids[row]
                .as_array()
                .expect("ids")
                .iter()
                .map(|value| value.as_u64().unwrap_or(0) as u32)
                .collect();
            assert_eq!(&ranked[..5], &expected[..], "row {row}");
        }
    }

    #[test]
    fn predict_is_normalized_and_ordered() {
        let model = NextWordModel::parse(BLOB).expect("fixture parses");
        let ranked = model.predict(&[3, 4, 5], 5);
        assert_eq!(ranked.len(), 5);
        assert!(ranked[0].1 >= ranked[1].1);
        assert!(
            ranked
                .iter()
                .all(|(_, probability)| (0.0..=1.0).contains(probability))
        );
        assert!(ranked.iter().map(|(_, p)| p).sum::<f32>() <= 1.001);
    }

    #[test]
    fn bad_magic_is_rejected() {
        static BAD: &[u8] = b"NOPE000000000000";
        assert_eq!(NextWordModel::parse(BAD).unwrap_err(), ModelError::BadMagic);
    }

    #[test]
    fn truncation_is_rejected() {
        static SHORT: &[u8] = b"OKNM";
        assert_eq!(
            NextWordModel::parse(SHORT).unwrap_err(),
            ModelError::Truncated
        );
    }

    #[test]
    fn words_round_trip() {
        let model = NextWordModel::parse(BLOB).expect("fixture parses");
        assert_eq!(model.word(3), "w3");
        assert_eq!(model.id("w7"), 7);
        assert_eq!(model.id("not-in-vocab"), 1);
    }
}
