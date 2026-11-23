//! Compatibility re-export: "Bijoy" output means SutonnyMJ ANSI.
//!
//! The 0.5 converter used the wrong legacy table (`Bangla to Bijoy
//! layout`, a **keyboard** map). The actual SutonnyMJ font encoding is
//! `ANSI 1` ("for SutoniMJ font"), implemented in [`crate::sutonnymj`].
//! This module keeps the historic public names ([`to_bijoy`],
//! [`from_bijoy`], [`BijoyError`], [`MAX_INPUT_CHARS`], [`RULE_COUNT`],
//! [`rule_count`]) so existing call sites keep compiling; all of them mean
//! Unicode ↔ SutonnyMJ ANSI. New code should import from
//! [`crate::sutonnymj`] directly.

pub use crate::sutonnymj::{
    BijoyError, MAX_INPUT_CHARS, REVERSE_RULE_COUNT, RULE_COUNT, from_bijoy, reverse_rule_count,
    rule_count, to_bijoy,
};
