//! Hyphenation: where a word may be broken at the end of a line, from TeX's
//! patterns (Liang's algorithm, through the `hypher` crate). English and
//! Ukrainian are built in; each more language is a `hypher` feature.

pub use hypher::Lang;

/// The patterns for a book's `dc:language`, such as "en", "en-US" or
/// "ukr". A book that doesn't say is taken as English.
pub fn lang(code: &str) -> Option<Lang> {
    let primary = code.trim().split(['-', '_']).next().unwrap_or("");
    let is = |names: &[&str]| names.iter().any(|n| primary.eq_ignore_ascii_case(n));
    if primary.is_empty() || is(&["en", "eng"]) {
        Some(Lang::English)
    } else if is(&["uk", "ukr"]) {
        Some(Lang::Ukrainian)
    } else {
        None
    }
}

/// An apostrophe, which can be part of a word: "don't", "комп'ютер".
pub fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2bc}')
}

/// Byte offsets inside `word` where it may be broken with a hyphen, in
/// order. Breaks too near either end are left out, as is usual for the
/// language (two letters before and three after, in English), and so are
/// breaks next to an apostrophe. Apostrophes at the ends, which are
/// quotation marks, don't count as part of the word.
pub fn breaks(word: &str, lang: Lang) -> impl Iterator<Item = usize> + '_ {
    let inner = word.trim_start_matches(is_apostrophe);
    let lead = word.len() - inner.len();
    let inner = inner.trim_end_matches(is_apostrophe);
    hypher::hyphenate(inner, lang)
        .scan(lead, |at, syllable| {
            *at += syllable.len();
            Some(*at)
        })
        .filter(move |&at| at < lead + inner.len())
        .filter(move |&at| !word[..at].ends_with(is_apostrophe) && !word[at..].starts_with(is_apostrophe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn languages() {
        assert_eq!(lang("en-US"), Some(Lang::English));
        assert_eq!(lang(" EN "), Some(Lang::English));
        assert_eq!(lang(""), Some(Lang::English));
        assert_eq!(lang("eng"), Some(Lang::English));
        assert_eq!(lang("en_GB"), Some(Lang::English));
        assert_eq!(lang("uk-UA"), Some(Lang::Ukrainian));
        assert_eq!(lang("ukr"), Some(Lang::Ukrainian));
        assert_eq!(lang("ru"), None);
        assert_eq!(lang("de"), None);
        assert_eq!(lang("eo"), None);
    }

    #[test]
    fn breaks_between_syllables() {
        let at = |w: &str, l| breaks(w, l).collect::<Vec<_>>();
        assert_eq!(at("extensive", Lang::English), [2, 5]);
        assert_eq!(at("Hyphenation", Lang::English), [2, 6]);
        assert_eq!(at("cat", Lang::English), []);
        // Byte offsets, two bytes a letter: "уні-вер-си-тет".
        assert_eq!(at("університет", Lang::Ukrainian), [6, 12, 16]);
    }

    #[test]
    fn apostrophes() {
        let at = |w: &str, l| breaks(w, l).collect::<Vec<_>>();
        // "ком-п'ю-тер", but never next to the apostrophe.
        assert_eq!(at("комп'ютер", Lang::Ukrainian), [6, 11]);
        assert_eq!(at("комп’ютер", Lang::Ukrainian), [6, 13]);
        assert_eq!(at("сім'я", Lang::Ukrainian), []);
        assert_eq!(at("o'clock", Lang::English), []);
        // A closing quote is left out of the word.
        assert_eq!(at("students’", Lang::English), at("students", Lang::English));
        assert_eq!(at("'extensive'", Lang::English), [3, 6]);
    }
}
