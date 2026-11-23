use std::fmt;

use sha2::{Digest, Sha256};

use crate::request::{AiRequest, AiResponse};

/// SHA-256 identity of a cacheable request.
///
/// The digest covers the system prompt, user prompt, model, prompt version,
/// `max_tokens`, and the IEEE-754 bits of `temperature`, with length-prefixed
/// text fields, explicit numeric tags, and a presence tag for the optional
/// system prompt. The raw prompt is never stored in the key, and [`Debug`] does
/// not expose digest bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AiCacheKey([u8; 32]);

impl AiCacheKey {
    /// Derive the key for a request.
    ///
    /// Only these fields participate: `system`, `prompt`, `model`,
    /// `prompt_version`, `max_tokens`, and the IEEE-754 bits of `temperature`.
    /// Requests that differ only in generation settings must not share a cache
    /// entry.
    pub fn from_request(request: &AiRequest) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"okkhor-ai-cache-v1\0");

        match request.system.as_deref() {
            Some(system) => {
                hasher.update([1u8]);
                hash_field(&mut hasher, system);
            }
            None => hasher.update([0u8]),
        }

        hash_field(&mut hasher, &request.prompt);
        hash_field(&mut hasher, &request.model);
        hash_field(&mut hasher, &request.prompt_version);

        hasher.update(b"max_tokens\0");
        hasher.update(request.max_tokens.to_le_bytes());
        hasher.update(b"temperature\0");
        hasher.update(request.temperature.to_bits().to_le_bytes());

        let digest = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Self(key)
    }

    /// Digest bytes, useful for redacted status output and tests.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for AiCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AiCacheKey(sha256)")
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}

#[derive(Debug)]
struct CacheEntry {
    key: AiCacheKey,
    response: AiResponse,
    bytes: usize,
}

/// Bounded, in-memory LRU cache for provider responses.
///
/// The cache holds at most `max_entries` responses and at most `max_bytes` of
/// response text (UTF-8 bytes). It performs no disk I/O and never logs. The
/// least-recently-used entry is evicted first. A response whose text alone
/// exceeds the byte budget is not cached at all.
///
/// Defaults match ADR 0018: 64 entries and 256 KiB.
#[derive(Debug)]
pub struct AiCache {
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    /// Oldest (least recently used) first, newest last.
    entries: Vec<CacheEntry>,
}

impl AiCache {
    /// Create a cache with the ADR 0018 defaults (64 entries / 256 KiB).
    pub fn new() -> Self {
        Self::with_limits(64, 256 * 1024)
    }

    /// Create a cache with explicit bounds.
    pub fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
            bytes: 0,
            entries: Vec::new(),
        }
    }

    /// Fetch a cached response and mark it most recently used.
    pub fn get(&mut self, key: &AiCacheKey) -> Option<AiResponse> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index);
        let response = entry.response.clone();
        self.entries.push(entry);
        Some(response)
    }

    /// Insert or replace a response.
    ///
    /// A response larger than the total byte budget is rejected and any older
    /// value for the same key remains untouched.
    pub fn insert(&mut self, key: AiCacheKey, response: AiResponse) {
        let bytes = response.text.len();
        if bytes > self.max_bytes || self.max_entries == 0 {
            return;
        }

        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            self.bytes -= self.entries.remove(index).bytes;
        }

        self.entries.push(CacheEntry {
            key,
            response,
            bytes,
        });
        self.bytes += bytes;

        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let evicted = self.entries.remove(0);
            self.bytes -= evicted.bytes;
        }
    }

    /// Number of cached responses.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total response-text bytes currently accounted for.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Drop every cached response.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

impl Default for AiCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::AiUsage;

    fn response(text: &str) -> AiResponse {
        AiResponse {
            text: String::from(text),
            model: String::from("mock-model"),
            usage: AiUsage::default(),
            latency_ms: 0,
        }
    }

    fn request(prompt: &str) -> AiRequest {
        AiRequest::new(prompt, "mock-model")
    }

    #[test]
    fn cache_hit_and_miss() {
        let mut cache = AiCache::new();
        let key = AiCacheKey::from_request(&request("hello"));

        assert_eq!(cache.get(&key), None);
        cache.insert(key, response("world"));
        assert_eq!(
            cache.get(&key).map(|value| value.text),
            Some(String::from("world"))
        );
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());
    }

    #[test]
    fn least_recently_used_entry_is_evicted() {
        let mut cache = AiCache::with_limits(2, 1024);
        let first = AiCacheKey::from_request(&request("first"));
        let second = AiCacheKey::from_request(&request("second"));
        let third = AiCacheKey::from_request(&request("third"));

        cache.insert(first, response("one"));
        cache.insert(second, response("two"));
        // Touch the first entry so the second becomes least recently used.
        assert!(cache.get(&first).is_some());
        cache.insert(third, response("three"));

        assert!(cache.get(&first).is_some());
        assert_eq!(cache.get(&second), None);
        assert!(cache.get(&third).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn byte_budget_evicts_least_recently_used() {
        let mut cache = AiCache::with_limits(8, 10);
        let first = AiCacheKey::from_request(&request("first"));
        let second = AiCacheKey::from_request(&request("second"));

        cache.insert(first, response("123456"));
        assert_eq!(cache.bytes(), 6);
        cache.insert(second, response("abcdef"));

        assert_eq!(cache.bytes(), 6);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&first), None);
        assert!(cache.get(&second).is_some());
    }

    #[test]
    fn oversized_response_is_not_cached() {
        let mut cache = AiCache::with_limits(8, 5);
        let key = AiCacheKey::from_request(&request("hello"));

        cache.insert(key, response("0123456789"));

        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.get(&key), None);
    }

    #[test]
    fn replacing_a_key_keeps_one_entry_and_correct_byte_count() {
        let mut cache = AiCache::with_limits(8, 1024);
        let key = AiCacheKey::from_request(&request("hello"));

        cache.insert(key, response("short"));
        cache.insert(key, response("a longer response"));

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), "a longer response".len());
        assert_eq!(
            cache.get(&key).map(|value| value.text),
            Some(String::from("a longer response"))
        );
    }

    #[test]
    fn clear_empties_entries_and_bytes() {
        let mut cache = AiCache::new();
        cache.insert(AiCacheKey::from_request(&request("a")), response("b"));
        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn key_is_stable_for_identical_fields() {
        let first = request("hello");
        let second = request("hello");

        assert_eq!(
            AiCacheKey::from_request(&first),
            AiCacheKey::from_request(&second)
        );
    }

    #[test]
    fn key_changes_with_model_and_prompt_version() {
        let base = request("hello");

        let mut other_model = base.clone();
        other_model.model = String::from("other-model");
        assert_ne!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&other_model)
        );

        let mut other_version = base.clone();
        other_version.prompt_version = String::from("v2");
        assert_ne!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&other_version)
        );
    }

    #[test]
    fn key_changes_with_generation_settings() {
        let base = request("hello");

        let mut other_max_tokens = base.clone();
        other_max_tokens.max_tokens = base.max_tokens + 1;
        assert_ne!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&other_max_tokens)
        );

        let mut other_temperature = base.clone();
        other_temperature.temperature = base.temperature + 0.5;
        assert_ne!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&other_temperature)
        );

        let same = base.clone();
        assert_eq!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&same)
        );
    }

    #[test]
    fn key_changes_with_prompt_and_system() {
        let base = request("hello");

        let mut other_prompt = base.clone();
        other_prompt.prompt = String::from("hello!");
        assert_ne!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&other_prompt)
        );

        let mut with_system = base.clone();
        with_system.system = Some(String::from("be brief"));
        assert_ne!(
            AiCacheKey::from_request(&base),
            AiCacheKey::from_request(&with_system)
        );
    }

    #[test]
    fn key_length_prefixing_prevents_field_boundary_collisions() {
        let mut first = request("c");
        first.system = Some(String::from("ab"));

        let mut second = request("bc");
        second.system = Some(String::from("a"));

        assert_ne!(
            AiCacheKey::from_request(&first),
            AiCacheKey::from_request(&second)
        );
    }

    #[test]
    fn key_debug_does_not_expose_digest_or_text() {
        let key = AiCacheKey::from_request(&request("secret prompt"));
        assert_eq!(format!("{key:?}"), "AiCacheKey(sha256)");
    }
}
