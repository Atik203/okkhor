//! Reviewed SIMD dot-product kernels for the next-word runtime.
//!
//! The engine crate forbids `unsafe`, and the projection layer is the hot loop
//! (200k rows × d_model per prediction), so the intrinsics live here behind a
//! safe API. ADR 0025 records this boundary.
//!
//! Every kernel takes the weight row as **little-endian bytes** because the
//! model format stores tensors that way. Unaligned loads are used, so callers
//! never inherit an alignment precondition.
//!
//! Correctness: each kernel has a scalar twin and the tests compare them on
//! deterministic pseudo-random inputs.

#![deny(unsafe_op_in_unsafe_fn)]

/// Dot product of an int8 weight row against an f32 activation vector.
///
/// Iterates over `min(row.len(), x.len())` elements.
pub fn dot_i8_f32(row: &[u8], x: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the branch is guarded by runtime AVX2 detection; the
            // kernel only performs unaligned loads bounded by the slice lengths.
            return unsafe { dot_i8_f32_avx2(row, x) };
        }
    }
    dot_i8_f32_scalar(row, x)
}

/// Dot product of a little-endian f32 weight row against an f32 vector.
///
/// Iterates over `min(x.len(), row.len() / 4)` elements.
pub fn dot_f32_bytes(row: &[u8], x: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by AVX2 detection; reads are unaligned and bounded
            // by the slice lengths.
            return unsafe { dot_f32_bytes_avx2(row, x) };
        }
    }
    dot_f32_bytes_scalar(row, x)
}

/// Project a whole int8 weight matrix against one activation vector.
///
/// One call per prediction instead of one per row: a `#[target_feature]`
/// function cannot be inlined into a plain caller, and 200k tiny calls cost
/// more than the math.
pub fn project_i8_f32(weights: &[u8], width: usize, x: &[f32], out: &mut [f32]) {
    if width == 0 {
        return;
    }
    let rows = out.len().min(weights.len() / width);
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by runtime AVX2 detection; `project_i8_avx2`
            // bounds every load by `rows * width` and `x.len()`.
            unsafe { project_i8_avx2(weights, width, x, &mut out[..rows]) };
            return;
        }
    }
    for (row, slot) in out[..rows].iter_mut().enumerate() {
        *slot = dot_i8_f32_scalar(&weights[row * width..(row + 1) * width], x);
    }
}

/// Project a whole f32 byte-matrix against one activation vector.
pub fn project_f32_bytes(weights: &[u8], width: usize, x: &[f32], out: &mut [f32]) {
    if width == 0 {
        return;
    }
    let rows = out.len().min(weights.len() / (width * 4));
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by runtime AVX2 detection; bounds are checked by
            // the kernel against `rows * width * 4` and `x.len()`.
            unsafe { project_f32_avx2(weights, width, x, &mut out[..rows]) };
            return;
        }
    }
    for (row, slot) in out[..rows].iter_mut().enumerate() {
        *slot = dot_f32_bytes_scalar(&weights[row * width * 4..(row + 1) * width * 4], x);
    }
}

fn dot_i8_f32_scalar(row: &[u8], x: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for (byte, value) in row.iter().zip(x.iter()) {
        sum += f32::from(*byte as i8) * value;
    }
    sum
}

fn dot_f32_bytes_scalar(row: &[u8], x: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for (chunk, value) in row.chunks_exact(4).zip(x.iter()) {
        sum += f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) * value;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn dot_i8_f32_avx2(row: &[u8], x: &[f32]) -> f32 {
    use std::arch::x86_64::{
        __m128i, _mm_loadl_epi64, _mm256_cvtepi8_epi32, _mm256_cvtepi32_ps, _mm256_fmadd_ps,
        _mm256_loadu_ps, _mm256_setzero_ps, _mm256_storeu_ps,
    };

    let lanes = row.len().min(x.len());
    let mut accumulator = _mm256_setzero_ps();
    let mut offset = 0usize;
    unsafe {
        // SAFETY: AVX2 is guaranteed by the caller (runtime detection). Every
        // load is unaligned and stays inside the slice it reads from:
        // `offset + 8 <= lanes <= row.len()` and `offset + 8 <= lanes <= x.len()`.
        while offset + 8 <= lanes {
            let packed: __m128i = _mm_loadl_epi64(row.as_ptr().add(offset) as *const __m128i);
            let weights = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(packed));
            let values = _mm256_loadu_ps(x.as_ptr().add(offset));
            accumulator = _mm256_fmadd_ps(weights, values, accumulator);
            offset += 8;
        }
    }

    let mut lanes_out = [0.0f32; 8];
    unsafe {
        // SAFETY: writes exactly eight f32 values into a stack array.
        _mm256_storeu_ps(lanes_out.as_mut_ptr(), accumulator);
    }
    let mut total: f32 = lanes_out.iter().sum();
    total += dot_i8_f32_scalar(&row[offset..lanes], &x[offset..lanes]);
    total
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn dot_f32_bytes_avx2(row: &[u8], x: &[f32]) -> f32 {
    use std::arch::x86_64::{
        _mm256_fmadd_ps, _mm256_loadu_ps, _mm256_setzero_ps, _mm256_storeu_ps,
    };

    let lanes = x.len().min(row.len() / 4);
    let mut accumulator = _mm256_setzero_ps();
    let mut offset = 0usize;
    unsafe {
        // SAFETY: AVX2 is guaranteed by the caller (runtime detection). Every
        // load is unaligned and in bounds: `(offset + 8) * 4 <= row.len()` and
        // `offset + 8 <= lanes <= x.len()`.
        while offset + 8 <= lanes {
            let weights = _mm256_loadu_ps(row.as_ptr().add(offset * 4) as *const f32);
            let values = _mm256_loadu_ps(x.as_ptr().add(offset));
            accumulator = _mm256_fmadd_ps(weights, values, accumulator);
            offset += 8;
        }
    }

    let mut lanes_out = [0.0f32; 8];
    unsafe {
        // SAFETY: writes exactly eight f32 values into a stack array.
        _mm256_storeu_ps(lanes_out.as_mut_ptr(), accumulator);
    }
    let mut total: f32 = lanes_out.iter().sum();
    total += dot_f32_bytes_scalar(&row[offset * 4..], &x[offset..]);
    total
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn project_i8_avx2(weights: &[u8], width: usize, x: &[f32], out: &mut [f32]) {
    use std::arch::x86_64::{
        __m256i, _mm_loadu_si128, _mm256_add_epi32, _mm256_cvtepi8_epi16, _mm256_loadu_si256,
        _mm256_madd_epi16, _mm256_setzero_si256, _mm256_storeu_si256,
    };

    // Quantize the activation once per projection. A per-vector scale uses the
    // full int16 range; the relative error is ~3e-5, far below what the softmax
    // ranking cares about.
    let peak = x.iter().fold(0.0f32, |acc, value| acc.max(value.abs()));
    let scale = if peak > 0.0 { 30_000.0 / peak } else { 0.0 };
    let inverse = if scale > 0.0 { 1.0 / scale } else { 0.0 };
    let quantized: Vec<i16> = x
        .iter()
        .map(|value| (value * scale).round().clamp(-30_000.0, 30_000.0) as i16)
        .collect();
    let lanes = width.min(quantized.len());

    unsafe {
        // SAFETY: AVX2 is guaranteed by the caller. For row `r`, weight loads
        // stay inside `weights[r * width .. (r + 1) * width]`; activation loads
        // stay inside `quantized[..lanes]`; `out` matches the checked row count.
        for (row, slot) in out.iter_mut().enumerate() {
            let base = row * width;
            let mut accumulator = _mm256_setzero_si256();
            let mut offset = 0usize;
            while offset + 16 <= lanes {
                let packed = _mm_loadu_si128(
                    weights.as_ptr().add(base + offset) as *const std::arch::x86_64::__m128i
                );
                let w16 = _mm256_cvtepi8_epi16(packed);
                let x16 = _mm256_loadu_si256(quantized.as_ptr().add(offset) as *const __m256i);
                accumulator = _mm256_add_epi32(accumulator, _mm256_madd_epi16(w16, x16));
                offset += 16;
            }
            let mut sums = [0i32; 8];
            _mm256_storeu_si256(sums.as_mut_ptr() as *mut __m256i, accumulator);
            let mut total: i32 = sums.iter().sum();
            if offset < lanes {
                for index in offset..lanes {
                    total += i32::from(weights[base + index] as i8) * i32::from(quantized[index]);
                }
            }
            *slot = total as f32 * inverse;
        }
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn project_f32_avx2(weights: &[u8], width: usize, x: &[f32], out: &mut [f32]) {
    use std::arch::x86_64::{
        __m256, _mm256_add_ps, _mm256_fmadd_ps, _mm256_loadu_ps, _mm256_setzero_ps,
        _mm256_storeu_ps,
    };

    let lanes = width.min(x.len());
    unsafe {
        // SAFETY: AVX2 is guaranteed by the caller. For row `r`, loads stay
        // inside `weights[r * width * 4 .. (r + 1) * width * 4]` and `x[..lanes]`;
        // `out` matches the checked row count. Four accumulators break the FMA
        // dependency chain (one accumulator is latency-bound at ~1.6 GMAC/s).
        for (row, slot) in out.iter_mut().enumerate() {
            let base = row * width * 4;
            let mut acc0 = _mm256_setzero_ps();
            let mut acc1 = _mm256_setzero_ps();
            let mut acc2 = _mm256_setzero_ps();
            let mut acc3 = _mm256_setzero_ps();
            let mut offset = 0usize;
            while offset + 32 <= lanes {
                acc0 = _mm256_fmadd_ps(
                    _mm256_loadu_ps(weights.as_ptr().add(base + offset * 4) as *const f32),
                    _mm256_loadu_ps(x.as_ptr().add(offset)),
                    acc0,
                );
                acc1 = _mm256_fmadd_ps(
                    _mm256_loadu_ps(weights.as_ptr().add(base + (offset + 8) * 4) as *const f32),
                    _mm256_loadu_ps(x.as_ptr().add(offset + 8)),
                    acc1,
                );
                acc2 = _mm256_fmadd_ps(
                    _mm256_loadu_ps(weights.as_ptr().add(base + (offset + 16) * 4) as *const f32),
                    _mm256_loadu_ps(x.as_ptr().add(offset + 16)),
                    acc2,
                );
                acc3 = _mm256_fmadd_ps(
                    _mm256_loadu_ps(weights.as_ptr().add(base + (offset + 24) * 4) as *const f32),
                    _mm256_loadu_ps(x.as_ptr().add(offset + 24)),
                    acc3,
                );
                offset += 32;
            }
            while offset + 8 <= lanes {
                acc0 = _mm256_fmadd_ps(
                    _mm256_loadu_ps(weights.as_ptr().add(base + offset * 4) as *const f32),
                    _mm256_loadu_ps(x.as_ptr().add(offset)),
                    acc0,
                );
                offset += 8;
            }
            let combined: __m256 =
                _mm256_add_ps(_mm256_add_ps(acc0, acc1), _mm256_add_ps(acc2, acc3));
            let mut lanes_out = [0.0f32; 8];
            _mm256_storeu_ps(lanes_out.as_mut_ptr(), combined);
            let mut total: f32 = lanes_out.iter().sum();
            total += dot_f32_bytes_scalar(
                &weights[base + offset * 4..base + width * 4],
                &x[offset..lanes],
            );
            *slot = total;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(count: usize, seed: u64) -> Vec<u64> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            })
            .collect()
    }

    #[test]
    fn int8_kernel_matches_scalar() {
        for length in [1usize, 7, 16, 33, 64, 513] {
            let numbers = pseudo_random(length, 0x4F4B_4B48);
            let row: Vec<u8> = numbers
                .iter()
                .map(|value| ((value % 255) as i16 - 127) as i8 as u8)
                .collect();
            let x: Vec<f32> = numbers
                .iter()
                .map(|value| ((value % 2001) as f32 - 1000.0) / 997.0)
                .collect();
            let expected = dot_i8_f32_scalar(&row, &x);
            let actual = dot_i8_f32(&row, &x);
            assert!(
                (expected - actual).abs() < 1e-3 * expected.abs().max(1.0),
                "length {length}: {expected} vs {actual}"
            );
        }
    }

    #[test]
    fn f32_kernel_matches_scalar() {
        for length in [1usize, 7, 16, 33, 64, 513] {
            let numbers = pseudo_random(length, 0x0F0F_0F0F);
            let row: Vec<u8> = numbers
                .iter()
                .flat_map(|value| {
                    let scalar = ((value % 2001) as f32 - 1000.0) / 991.0;
                    scalar.to_le_bytes()
                })
                .collect();
            let x: Vec<f32> = numbers.iter().map(|value| (value % 7) as f32).collect();
            let expected = dot_f32_bytes_scalar(&row, &x);
            let actual = dot_f32_bytes(&row, &x);
            assert!(
                (expected - actual).abs() < 1e-2 * expected.abs().max(1.0),
                "length {length}: {expected} vs {actual}"
            );
        }
    }

    #[test]
    fn block_projection_matches_scalar_rows() {
        let width = 512usize;
        let rows = 300usize;
        let numbers = pseudo_random(rows * width, 0x5151_5151);
        let weights: Vec<u8> = numbers
            .iter()
            .map(|value| ((value % 255) as i16 - 127) as i8 as u8)
            .collect();
        let x: Vec<f32> = (0..width)
            .map(|index| (index % 13) as f32 * 0.03 - 0.2)
            .collect();
        let mut projected = vec![0.0f32; rows];
        project_i8_f32(&weights, width, &x, &mut projected);
        for (row, value) in projected.iter().enumerate() {
            let expected = dot_i8_f32_scalar(&weights[row * width..(row + 1) * width], &x);
            assert!(
                (expected - value).abs() < 1e-2 * expected.abs().max(1.0),
                "row {row}: {expected} vs {value}"
            );
        }

        let f32_weights: Vec<u8> = weights
            .iter()
            .flat_map(|byte| ((*byte as i8) as f32 * 0.01).to_le_bytes())
            .collect();
        let mut projected = vec![0.0f32; rows];
        project_f32_bytes(&f32_weights, width, &x, &mut projected);
        for (row, value) in projected.iter().enumerate() {
            let expected =
                dot_f32_bytes_scalar(&f32_weights[row * width * 4..(row + 1) * width * 4], &x);
            assert!(
                (expected - value).abs() < 1e-2 * expected.abs().max(1.0),
                "f32 row {row}: {expected} vs {value}"
            );
        }
    }

    #[test]
    fn empty_and_short_inputs_are_safe() {
        assert_eq!(dot_i8_f32(&[], &[]), 0.0);
        assert_eq!(dot_f32_bytes(&[], &[]), 0.0);
        assert_eq!(dot_i8_f32(&[1, 2], &[1.0]), 1.0);
    }
}
