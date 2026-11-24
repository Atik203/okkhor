//! Layout selection for the daemon. One startup decision, no hot-path I/O.
//!
//! Roman layouts (Avro, Phonetic/Banglish, Khipro) share the phonetic engine
//! but compile different rule tables. Physical-key layouts (National, Probhat,
//! and Bijoy which reuses National keys) use the fixed-layout resolver. Any
//! unknown code or failed asset falls back to Avro and reports an allowlisted
//! health code.

use std::sync::Arc;
use std::time::{Duration, Instant};

use okkhor_core::Ranker;
use okkhor_engine::SuggestionEngine;
use okkhor_lang::fixed_layout::{
    FixedLayout, ModifierSet, NATIONAL_ASSET, PROBHAT_ASSET, from_asset_json,
};
use okkhor_lang::layout_tables::{KHIPRO_RULES, PHONETIC_RULES, layout_rules_to_asset_json};

/// The one engine selected by `settings.layout`.
pub enum LayoutEngine {
    Phonetic(Box<SuggestionEngine>),
    Fixed(Box<FixedLayout>),
}

#[cfg_attr(not(test), allow(dead_code))]
impl LayoutEngine {
    /// Resolve a settings layout code. Returns the engine and an optional
    /// allowlisted health code for a fallback. Never fails startup.
    pub fn from_settings(
        code: &str,
        ranker: Arc<dyn Ranker + Send + Sync>,
    ) -> (Self, Option<&'static str>) {
        match code {
            "avro" => match SuggestionEngine::from_embedded() {
                Ok(engine) => (Self::Phonetic(Box::new(engine)), None),
                Err(_) => (
                    Self::Phonetic(Box::new(
                        SuggestionEngine::without_index().expect("rule-only engine"),
                    )),
                    Some("avro_load"),
                ),
            },
            "khipro" => phonetic_table(KHIPRO_RULES, ranker, "khipro_load"),
            "phonetic" | "banglish" => phonetic_table(PHONETIC_RULES, ranker, "phonetic_load"),
            "national" | "bijoy" | "probhat" => {
                let asset = if code == "probhat" {
                    PROBHAT_ASSET
                } else {
                    NATIONAL_ASSET
                };
                match from_asset_json(asset) {
                    Ok(layout) => (Self::Fixed(Box::new(layout)), None),
                    Err(_) => (avro_fallback(ranker), Some("fixed_layout_load")),
                }
            }
            _ => (avro_fallback(ranker), Some("unknown_layout")),
        }
    }

    /// Whether this layout needs physical-key swallowing and no popup.
    pub fn is_fixed(&self) -> bool {
        matches!(self, Self::Fixed(_))
    }

    /// The fixed table, when this layout is a physical-key layout.
    pub fn fixed(&self) -> Option<&FixedLayout> {
        match self {
            Self::Fixed(layout) => Some(layout),
            Self::Phonetic(_) => None,
        }
    }

    /// Take the phonetic engine for the composition session, if any.
    pub fn into_phonetic(self) -> Option<SuggestionEngine> {
        match self {
            Self::Phonetic(engine) => Some(*engine),
            Self::Fixed(_) => None,
        }
    }
}

fn phonetic_table(
    rules: &str,
    ranker: Arc<dyn Ranker + Send + Sync>,
    health: &'static str,
) -> (LayoutEngine, Option<&'static str>) {
    let converted = match layout_rules_to_asset_json(rules) {
        Ok(json) => json,
        Err(_) => return (avro_fallback(ranker), Some(health)),
    };
    match SuggestionEngine::from_embedded_table(&converted) {
        Ok(engine) => {
            let _ = ranker;
            (LayoutEngine::Phonetic(Box::new(engine)), None)
        }
        Err(_) => (avro_fallback(ranker), Some(health)),
    }
}

fn avro_fallback(ranker: Arc<dyn Ranker + Send + Sync>) -> LayoutEngine {
    let engine = SuggestionEngine::from_embedded()
        .or_else(|_| SuggestionEngine::without_index())
        .expect("rule-only engine");
    let _ = ranker;
    LayoutEngine::Phonetic(Box::new(engine))
}

/// How long a possible sequence prefix is held before its single character
/// is injected. Only keys that actually start a sequence are delayed.
pub const SEQUENCE_HOLD: Duration = Duration::from_millis(30);

/// Bounded pending state for physical-key sequences.
#[derive(Debug)]
pub struct FixedComposition {
    keys: Vec<&'static str>,
    pending: Option<(String, Instant)>,
}

impl Default for FixedComposition {
    fn default() -> Self {
        Self {
            keys: Vec::with_capacity(3),
            pending: None,
        }
    }
}

impl FixedComposition {
    /// Accept one physical key. Returns text to inject immediately, or `None`
    /// when the character is held briefly because it could start a sequence.
    pub fn push(
        &mut self,
        layout: &FixedLayout,
        key: &'static str,
        modifiers: ModifierSet,
        now: Instant,
    ) -> Option<String> {
        if let Some((held, _)) = self.pending.take() {
            self.keys.push(key);
            if let Some(sequence) = layout.resolve_sequence(&self.keys) {
                self.keys.clear();
                return Some(sequence.to_owned());
            }
            // No sequence: emit the held character first, then handle this
            // key as a fresh candidate prefix so ordering stays correct.
            self.keys.clear();
            let single = self.single(layout, key, modifiers);
            self.keys.push(key);
            if layout.is_sequence_prefix(&self.keys) {
                self.pending = Some((single, now));
                return Some(held);
            }
            self.keys.clear();
            return Some(format!("{held}{single}"));
        }

        self.keys.clear();
        self.keys.push(key);
        let single = self.single(layout, key, modifiers);
        if layout.is_sequence_prefix(&self.keys) {
            self.pending = Some((single, now));
            None
        } else {
            self.keys.clear();
            Some(single)
        }
    }

    /// Flush a held character whose sequence window has elapsed.
    pub fn flush(&mut self, now: Instant) -> Option<String> {
        match &self.pending {
            Some((text, at)) if now.duration_since(*at) >= SEQUENCE_HOLD => {
                let text = text.clone();
                self.clear();
                Some(text)
            }
            _ => None,
        }
    }

    /// Drop all pending state after focus change, pause, or mode switch.
    pub fn clear(&mut self) {
        self.keys.clear();
        self.pending = None;
    }

    fn single(&self, layout: &FixedLayout, key: &str, modifiers: ModifierSet) -> String {
        layout
            .resolve(key, modifiers)
            .unwrap_or_default()
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkhor_learn::Smriti;

    fn ranker() -> Arc<dyn Ranker + Send + Sync> {
        Arc::new(Smriti::new())
    }

    #[test]
    fn roman_codes_resolve_to_phonetic_engines() {
        for code in ["avro", "khipro", "phonetic", "banglish"] {
            let (engine, health) = LayoutEngine::from_settings(code, ranker());
            assert!(!engine.is_fixed(), "{code} should be phonetic");
            assert!(engine.into_phonetic().is_some());
            assert_eq!(health, None, "{code} should not fall back");
        }
    }

    #[test]
    fn fixed_codes_resolve_to_fixed_layouts() {
        for code in ["national", "probhat", "bijoy"] {
            let (engine, health) = LayoutEngine::from_settings(code, ranker());
            assert!(engine.is_fixed(), "{code} should be fixed");
            assert!(engine.fixed().is_some());
            assert_eq!(health, None, "{code} should not fall back");
        }
    }

    #[test]
    fn unknown_code_falls_back_to_avro_with_a_health_code() {
        let (engine, health) = LayoutEngine::from_settings("nope", ranker());
        assert!(!engine.is_fixed());
        assert_eq!(health, Some("unknown_layout"));
    }

    #[test]
    fn sequence_prefix_is_held_and_then_replaced() {
        let (engine, _) = LayoutEngine::from_settings("national", ranker());
        let layout = engine.fixed().expect("national");
        let mut composition = FixedComposition::default();
        let now = Instant::now();
        let none = ModifierSet {
            shift: false,
            altgr: false,
        };
        assert_eq!(composition.push(layout, "Key_G", none, now), None);
        assert_eq!(
            composition.push(layout, "Key_S", none, now),
            Some("উ".to_owned())
        );
    }

    #[test]
    fn held_prefix_flushes_after_the_window_and_clear_drops_it() {
        let (engine, _) = LayoutEngine::from_settings("national", ranker());
        let layout = engine.fixed().expect("national");
        let now = Instant::now();
        let none = ModifierSet::default();
        let mut composition = FixedComposition::default();
        assert_eq!(composition.push(layout, "Key_G", none, now), None);
        assert_eq!(composition.flush(now), None);
        assert_eq!(composition.flush(now + SEQUENCE_HOLD), Some("্".to_owned()));

        let mut composition = FixedComposition::default();
        assert_eq!(composition.push(layout, "Key_G", none, now), None);
        composition.clear();
        assert_eq!(composition.flush(now + SEQUENCE_HOLD), None);
    }

    #[test]
    fn roman_tables_are_not_identical() {
        let (avro, _) = LayoutEngine::from_settings("avro", ranker());
        let (khipro, _) = LayoutEngine::from_settings("khipro", ranker());
        let avro = avro.into_phonetic().expect("avro");
        let khipro = khipro.into_phonetic().expect("khipro");
        let smriti = Smriti::new();
        let a = avro.suggest(&okkhor_engine::SuggestRequest::new("nftff", 8), &smriti);
        let k = khipro.suggest(&okkhor_engine::SuggestRequest::new("nftff", 8), &smriti);
        assert_ne!(
            a.first().map(|s| s.text.as_str()),
            k.first().map(|s| s.text.as_str()),
            "Khipro must not silently produce Avro output"
        );
    }
}
