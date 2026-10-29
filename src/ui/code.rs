//! Highlighted code: `kynex_core::highlight` tokens as gpui text styles. Blocks
//! are lexed once per change and cached, so frames only map tokens to colours.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::ops::Range;
use std::rc::Rc;

use gpui::HighlightStyle;
use kynex_core::highlight::{self, Lang, Side, Token};

use crate::theme::Syntax;

type Tokens = Rc<Vec<(Range<usize>, Token)>>;

thread_local! {
    /// Tokens by a hash of the language and the code.
    static CACHE: RefCell<(RandomState, HashMap<u64, Tokens>)> = RefCell::default();
}

/// `code`'s tokens in `lang`, from the cache while it's unchanged.
pub fn tokens(lang: Lang, code: &str) -> Tokens {
    CACHE.with_borrow_mut(|(hasher, cache)| {
        let key = hasher.hash_one((lang, code));
        if let Some(t) = cache.get(&key) {
            return t.clone();
        }
        // Streaming blocks leave a trail of old versions: dropped wholesale now and then.
        if cache.len() >= 2048 {
            cache.clear();
        }
        let t = Rc::new(highlight::highlight(lang, code));
        cache.insert(key, t.clone());
        t
    })
}

thread_local! {
    static DIFFS: RefCell<(RandomState, HashMap<u64, DiffTokens>)> = RefCell::default();
}

type DiffTokens = Rc<Vec<Vec<(Range<usize>, Token)>>>;

/// Each line's tokens for diff `lines` in `lang` (see `highlight::diff`), cached.
pub fn diff_tokens(lang: Lang, lines: &[(Side, &str)]) -> DiffTokens {
    DIFFS.with_borrow_mut(|(hasher, cache)| {
        let key = hasher.hash_one((lang, lines));
        if let Some(t) = cache.get(&key) {
            return t.clone();
        }
        if cache.len() >= 512 {
            cache.clear();
        }
        let t = Rc::new(highlight::diff(lang, lines.iter().copied()));
        cache.insert(key, t.clone());
        t
    })
}

/// Tokens as styles for `StyledText::with_highlights`.
pub fn styles(tokens: &[(Range<usize>, Token)], syntax: &Syntax) -> Vec<(Range<usize>, HighlightStyle)> {
    tokens.iter().map(|(r, t)| (r.clone(), syntax.style(*t))).collect()
}

/// `code` highlighted as `lang` (no styles for an unknown language).
pub fn highlighted(lang: Option<Lang>, code: &str, syntax: &Syntax) -> Vec<(Range<usize>, HighlightStyle)> {
    lang.map(|l| styles(&tokens(l, code), syntax)).unwrap_or_default()
}
