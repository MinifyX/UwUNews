//! The rules of a filter feed.
//!
//! A collection either takes everything from its sources or only what its
//! rules let through. The rules are three lists of words, matched without
//! regard to case or accents, as whole words or word prefixes ("klima"
//! matches "Klimawandel", not "Reklima"). Plain lists on purpose: they read
//! well on a phone and sync as a few strings.

/// Where words are looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchIn {
    #[default]
    Title,
    TitleAndContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rules {
    /// At least one of these. Empty: no requirement.
    pub any: Vec<String>,
    /// Every one of these.
    pub all: Vec<String>,
    /// None of these.
    pub exclude: Vec<String>,
    pub search_in: SearchIn,
}

impl Rules {
    /// No words at all: the collection takes everything.
    pub fn is_empty(&self) -> bool {
        [&self.any, &self.all, &self.exclude]
            .iter()
            .all(|list| list.iter().all(|w| w.trim().is_empty()))
    }

    /// Whether an item passes, given its title and its text (summary and
    /// content; only looked at for [`SearchIn::TitleAndContent`]).
    pub fn matches(&self, title: &str, text: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let mut haystack = fold(title);
        if self.search_in == SearchIn::TitleAndContent {
            haystack.push(' ');
            haystack.push_str(&fold(text));
        }
        let words: Vec<&str> = haystack
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let has = |needle: &String| {
            let needle = fold(needle.trim());
            if needle.is_empty() {
                return None;
            }
            let parts: Vec<&str> = needle
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            Some(contains_phrase(&words, &parts))
        };
        let any: Vec<bool> = self.any.iter().filter_map(has).collect();
        if !any.is_empty() && !any.iter().any(|&m| m) {
            return false;
        }
        if self.all.iter().filter_map(has).any(|m| !m) {
            return false;
        }
        !self.exclude.iter().filter_map(has).any(|m| m)
    }
}

/// Whether the words contain the phrase: consecutive words, the last one as a
/// prefix, the others whole.
fn contains_phrase(words: &[&str], phrase: &[&str]) -> bool {
    let Some((last, head)) = phrase.split_last() else {
        return false;
    };
    if words.len() < phrase.len() {
        return false;
    }
    (0..=words.len() - phrase.len()).any(|at| {
        head.iter().zip(&words[at..]).all(|(p, w)| p == w)
            && words[at + head.len()].starts_with(last)
    })
}

/// Lowercase, umlauts and common accents folded to their base letter, so
/// "Über" matches "uber" and "café" matches "cafe".
fn fold(text: &str) -> String {
    text.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'ä' | 'á' | 'à' | 'â' | 'å' | 'ã' => 'a',
            'ö' | 'ó' | 'ò' | 'ô' | 'õ' | 'ø' => 'o',
            'ü' | 'ú' | 'ù' | 'û' => 'u',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ç' => 'c',
            'ñ' => 'n',
            'ß' => 's',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(any: &[&str], all: &[&str], exclude: &[&str]) -> Rules {
        let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        Rules {
            any: v(any),
            all: v(all),
            exclude: v(exclude),
            search_in: SearchIn::Title,
        }
    }

    #[test]
    fn empty_rules_take_everything() {
        assert!(rules(&[], &["  "], &[]).matches("Irgendwas", ""));
    }

    #[test]
    fn any_all_exclude() {
        let r = rules(&["rust", "swift"], &["release"], &["beta"]);
        assert!(r.matches("Rust 2.0 Release", ""));
        assert!(!r.matches("Rust 2.0 Beta Release", ""));
        assert!(!r.matches("Rust 2.0 kommt", ""));
        assert!(!r.matches("Go Release", ""));
    }

    #[test]
    fn prefixes_accents_and_phrases() {
        assert!(rules(&["klima"], &[], &[]).matches("Der Klimawandel", ""));
        assert!(!rules(&["klima"], &[], &[]).matches("Reklimatisierung", ""));
        assert!(rules(&["uber"], &[], &[]).matches("Über uns", ""));
        assert!(rules(&["apple watch"], &[], &[]).matches("Die neue Apple Watches", ""));
        assert!(!rules(&["apple watch"], &[], &[]).matches("Apple und die Watch", ""));
    }

    #[test]
    fn content_only_when_asked() {
        let mut r = rules(&["linux"], &[], &[]);
        assert!(!r.matches("Neuer Kernel", "für Linux"));
        r.search_in = SearchIn::TitleAndContent;
        assert!(r.matches("Neuer Kernel", "für Linux"));
    }
}
