//! Pure edit planning for inline Bangla preedit.
//!
//! The document already contains `rendered`; the next keystroke produced
//! `next`. Only the suffix after the longest common prefix needs to change.
//! The boundary is clamped to a grapheme cluster so a backspace can never land
//! between a Bengali base character and its combining mark.
//!
//! This module performs no I/O and no allocation beyond the returned suffix.

/// One document replacement: erase this many UTF-16 units, then insert `insert`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlan {
    pub erase_utf16: usize,
    pub insert: String,
}

impl EditPlan {
    /// True when the document already matches `next`.
    pub fn is_noop(&self) -> bool {
        self.erase_utf16 == 0 && self.insert.is_empty()
    }

    /// UTF-16 units that will be inserted, including surrogate pairs.
    pub fn insert_utf16(&self) -> usize {
        self.insert.encode_utf16().count()
    }
}

/// Bengali combining marks, nukta, length marks and joiners. A common-prefix
/// boundary immediately before one of these would split a cluster.
pub fn is_join_mark(ch: char) -> bool {
    matches!(
        ch,
        '\u{09BC}'          // nukta
            | '\u{09BE}'..='\u{09CD}' // vowel signs, virama
            | '\u{09D7}'    // au length mark
            | '\u{09E2}'..='\u{09E3}' // vocalic signs
            | '\u{200C}'    // zero-width non-joiner
            | '\u{200D}'    // zero-width joiner
    )
}

/// Longest common prefix length in `char`s.
fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(a, b)| a == b)
        .count()
}

/// Plan the smallest document edit that turns `rendered` into `next`.
///
/// `erase_utf16` counts UTF-16 units because `SendInput` and UI Automation
/// caret offsets use them. The common prefix is clamped backwards whenever the
/// next character is a joining mark, so a base and its mark are always erased
/// together.
pub fn plan_edit(rendered: &str, next: &str) -> EditPlan {
    let mut lcp = common_prefix_chars(rendered, next);
    let next_chars: Vec<char> = next.chars().collect();
    let erase_before_clamp = rendered
        .chars()
        .skip(lcp)
        .map(char::len_utf16)
        .sum::<usize>();
    // Appending a mark after an unchanged base is a pure insert and must stay
    // smooth. Only a replacement that would erase part of a cluster is pulled
    // back to the cluster start, so a base and its marks are retyped together.
    if erase_before_clamp > 0 {
        while lcp > 0 && next_chars.get(lcp).is_some_and(|ch| is_join_mark(*ch)) {
            lcp -= 1;
        }
    }
    let erase_utf16 = rendered
        .chars()
        .skip(lcp)
        .map(char::len_utf16)
        .sum::<usize>();
    let insert: String = next_chars.into_iter().skip(lcp).collect();
    EditPlan {
        erase_utf16,
        insert,
    }
}

#[cfg(test)]
mod tests {
    use super::{EditPlan, is_join_mark, plan_edit};

    #[test]
    fn empty_to_bangla_inserts_without_erasing() {
        assert_eq!(
            plan_edit("", "আ"),
            EditPlan {
                erase_utf16: 0,
                insert: "আ".to_owned()
            }
        );
    }

    #[test]
    fn changing_a_vowel_sign_retypes_that_cluster_only() {
        // া -> ি is a mark change, so base ম and the mark are retyped; the
        // unchanged আ is not.
        assert_eq!(
            plan_edit("আমার", "আমি"),
            EditPlan {
                erase_utf16: 3, // ম + া + র
                insert: "মি".to_owned()
            }
        );
    }

    #[test]
    fn appending_a_vowel_sign_is_a_pure_insert() {
        assert_eq!(
            plan_edit("ক", "কি"),
            EditPlan {
                erase_utf16: 0,
                insert: "ি".to_owned()
            }
        );
    }

    #[test]
    fn identical_text_is_a_noop() {
        assert!(plan_edit("আমার", "আমার").is_noop());
    }

    #[test]
    fn a_join_mark_change_never_splits_the_cluster() {
        // Changing ি -> ো erases and retypes the whole base+mark cluster.
        assert_eq!(
            plan_edit("কি", "কো"),
            EditPlan {
                erase_utf16: 2,
                insert: "কো".to_owned()
            }
        );
        assert!(is_join_mark('ি'));
        assert!(is_join_mark('\u{09CD}'));
        assert!(!is_join_mark('ক'));
    }

    #[test]
    fn surrogate_pairs_count_utf16_units_correctly() {
        // Only the trailing ASCII changes; the emoji is never erased.
        assert_eq!(
            plan_edit("😀", "😀x"),
            EditPlan {
                erase_utf16: 0,
                insert: "x".to_owned()
            }
        );
        assert_eq!(
            plan_edit("😀x", "😀"),
            EditPlan {
                erase_utf16: 1,
                insert: String::new()
            }
        );
        // An emoji replaced by another erases two units, not one char.
        assert_eq!(plan_edit("😀", "😁").erase_utf16, 2);
    }

    #[test]
    fn conjunct_formation_replaces_the_tail() {
        // র + ্ + ম evolves as the virama arrives, appending each mark.
        let first = plan_edit("র", "র্");
        assert_eq!(first.erase_utf16, 0);
        assert_eq!(first.insert, "্");
        let second = plan_edit("র্", "র্ম");
        assert_eq!(second.erase_utf16, 0);
        assert_eq!(second.insert, "ম");
    }
}
