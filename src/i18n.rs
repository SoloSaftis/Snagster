//! The language of Snagster's own interface: 86 languages, the computer's language by default.
//!
//! The English text in the source is the key. [`t`] returns the current language's text for it
//! (or the English when a pack has no entry), [`tf`] fills in `{name}` placeholders after
//! translating, so a translation may put the words in another order. Packs are JSON files in
//! `assets/i18n/<code>.json`, built into the program. Language names come from
//! `assets/i18n/language-names.json`: every language's name in every language, so the list of
//! languages is always shown in the language the interface is in.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

#[path = "i18n_packs.rs"]
mod packs;
pub use packs::CODES;

/// The interface language right now ("en", "de", "pt-BR"...).
static CURRENT: RwLock<&'static str> = RwLock::new("en");
/// The current language's table (English key → text); None for English.
static TABLE: RwLock<Option<&'static HashMap<String, String>>> = RwLock::new(None);

static NAMES: LazyLock<HashMap<String, HashMap<String, String>>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../assets/i18n/language-names.json")).unwrap_or_default());

#[derive(serde::Deserialize, Default)]
struct Pack {
    #[serde(default)]
    strings: HashMap<String, String>,
}

/// Every language's table, parsed once when first used and kept.
static LOADED: LazyLock<RwLock<HashMap<&'static str, &'static HashMap<String, String>>>> = LazyLock::new(Default::default);

fn table_for(code: &'static str) -> Option<&'static HashMap<String, String>> {
    if code == "en" {
        return None;
    }
    if let Some(t) = LOADED.read().ok().and_then(|m| m.get(code).copied()) {
        return Some(t);
    }
    let text = packs::PACKS.iter().find(|(c, _)| *c == code)?.1;
    let pack: Pack = serde_json::from_str(text).unwrap_or_default();
    // Keep only real translations (empty entries fall back to English).
    let table: HashMap<String, String> = pack.strings.into_iter().filter(|(_, v)| !v.trim().is_empty()).collect();
    let leaked: &'static HashMap<String, String> = Box::leak(Box::new(table));
    if let Ok(mut m) = LOADED.write() {
        m.insert(code, leaked);
    }
    Some(leaked)
}

/// The code from the list for a language tag, or None.
fn known(code: &str) -> Option<&'static str> {
    CODES.iter().copied().find(|c| c.eq_ignore_ascii_case(code))
}

/// The language a locale such as "nb-NO", "pt_BR.UTF-8", "zh-Hant-TW" or "sr-Latn-RS" asks for.
pub fn match_locale(locale: &str) -> &'static str {
    let tag = locale.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let parts: Vec<&str> = tag.split('-').filter(|p| !p.is_empty()).collect();
    let Some(lang) = parts.first().map(|l| l.to_ascii_lowercase()) else { return "en" };
    let rest: Vec<String> = parts.iter().skip(1).map(|p| p.to_ascii_uppercase()).collect();
    let has = |x: &str| rest.iter().any(|r| r.eq_ignore_ascii_case(x));
    let special = match lang.as_str() {
        "nb" | "no" => Some("no"),
        "nn" => Some("nn"),
        "zh" if has("HANT") || has("TW") || has("HK") || has("MO") => Some("zh-Hant"),
        "zh" => Some("zh"),
        "sr" if has("LATN") => Some("sr-Latn"),
        "sr" if has("BA") => Some("sr-Cyrl-BA"),
        "ca" if has("VALENCIA") => Some("ca-ES-valencia"),
        "pt" if has("BR") => Some("pt-BR"),
        "es" if has("MX") || has("419") || has("US") || has("AR") || has("CO") || has("CL") || has("PE") => Some("es-MX"),
        "fr" if has("CA") => Some("fr-CA"),
        "en" if has("GB") || has("AU") || has("NZ") || has("IE") || has("ZA") || has("IN") => Some("en-GB"),
        "tl" => Some("fil"),
        _ => None,
    };
    if let Some(c) = special.and_then(known) {
        return c;
    }
    if rest.first().is_some_and(|r| r.len() == 2)
        && let Some(c) = known(&format!("{lang}-{}", rest[0]))
    {
        return c;
    }
    known(&lang).unwrap_or("en")
}

/// The computer's own language, matched to one of the 86.
pub fn system() -> &'static str {
    match_locale(&sys_locale::get_locale().unwrap_or_default())
}

/// The language a setting means: a code, or "" for the computer's language.
pub fn resolve(setting: &str) -> &'static str {
    if setting.is_empty() { system() } else { known(setting).unwrap_or_else(system) }
}

/// Switch the interface language (affects every [`t`] from now on).
pub fn set(code: &str) {
    let code = known(code).unwrap_or("en");
    let table = table_for(code);
    if let Ok(mut c) = CURRENT.write() {
        *c = code;
    }
    if let Ok(mut t) = TABLE.write() {
        *t = table;
    }
}

pub fn current() -> &'static str {
    CURRENT.read().map(|c| *c).unwrap_or("en")
}

/// Whether the language is written right to left.
pub fn is_rtl(code: &str) -> bool {
    matches!(code.split('-').next().unwrap_or(""), "ar" | "fa" | "he" | "ur" | "ug")
}

/// Whether the language uses Chinese, Japanese or Korean characters.
pub fn is_cjk(code: &str) -> bool {
    matches!(code.split('-').next().unwrap_or(""), "zh" | "ja" | "ko")
}

/// `code`'s name in the language `in_lang` ("Deutsch" for de in de, "German" in en).
pub fn name_in(code: &str, in_lang: &str) -> String {
    NAMES
        .get(in_lang)
        .and_then(|m| m.get(code))
        .or_else(|| NAMES.get("en").and_then(|m| m.get(code)))
        .cloned()
        .unwrap_or_else(|| code.to_string())
}

/// The language's name in itself.
pub fn native_name(code: &str) -> String {
    name_in(code, code)
}

/// The language's name in the current interface language.
pub fn name(code: &str) -> String {
    name_in(code, current())
}

/// Translate an interface text (the English text is the key).
pub fn t(en: &'static str) -> &'static str {
    match TABLE.read().ok().and_then(|t| *t) {
        Some(table) => table.get(en).map(|s| s.as_str()).unwrap_or(en),
        None => en,
    }
}

/// Translate a text made at run time (falls back to the text itself).
pub fn tr(en: &str) -> String {
    match TABLE.read().ok().and_then(|t| *t) {
        Some(table) => table.get(en).cloned().unwrap_or_else(|| en.to_string()),
        None => en.to_string(),
    }
}

/// Translate, then fill in `{name}` placeholders.
pub fn tf(en: &'static str, args: &[(&str, &str)]) -> String {
    let mut s = t(en).to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// How many entries the language's pack has (0 for English), for the settings list.
pub fn coverage(code: &str) -> usize {
    known(code).and_then(table_for).map(|t| t.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_match() {
        assert_eq!(match_locale("nb-NO"), "no");
        assert_eq!(match_locale("pt_BR.UTF-8"), "pt-BR");
        assert_eq!(match_locale("pt-PT"), "pt");
        assert_eq!(match_locale("zh-Hant-TW"), "zh-Hant");
        assert_eq!(match_locale("zh-CN"), "zh");
        assert_eq!(match_locale("sr-Latn-RS"), "sr-Latn");
        assert_eq!(match_locale("en-US"), "en");
        assert_eq!(match_locale("en-AU"), "en-GB");
        assert_eq!(match_locale("es-419"), "es-MX");
        assert_eq!(match_locale("xx-YY"), "en");
        assert_eq!(match_locale(""), "en");
    }

    #[test]
    fn every_language_has_names() {
        assert_eq!(CODES.len(), 86);
        for c in CODES {
            for d in CODES {
                assert!(!name_in(c, d).is_empty());
            }
        }
        assert_eq!(name_in("de", "en"), "German");
    }

    #[test]
    fn every_pack_parses() {
        for (code, text) in packs::PACKS {
            let p: Result<serde_json::Value, _> = serde_json::from_str(text);
            assert!(p.is_ok(), "{code} pack isn't valid JSON");
        }
    }
}
