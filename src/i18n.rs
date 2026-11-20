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

/// `text` in language `code`, without switching the interface (for tests run side by side).
#[cfg(test)]
pub(crate) fn t_in(code: &'static str, text: &'static str) -> &'static str {
    table_for(code).and_then(|t| t.get(text)).map(|s| s.as_str()).unwrap_or(text)
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
    // Snagster's Norwegian pack is "no", but inside the table of names Norwegian Bokmål has its
    // standard code, "nb" (as Nynorsk has "nn").
    let named = if code == "no" { "nb" } else { code };
    NAMES
        .get(in_lang)
        .and_then(|m| m.get(named))
        .or_else(|| NAMES.get("en").and_then(|m| m.get(named)))
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

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_and_unknown_text_stays() {
        assert_eq!(tf("Saved {file}", &[("file", "a.png")]), "Saved a.png");
        assert_eq!(tr("no such text anywhere"), "no such text anywhere");
        assert!(is_rtl("ar") && is_rtl("he") && !is_rtl("de"));
        assert!(is_cjk("zh-Hant") && !is_cjk("hi"));
    }

    #[test]
    fn every_pack_translates_every_text() {
        let keys: Vec<String> = serde_json::from_str(include_str!("../assets/i18n/strings.json")).unwrap();
        for (code, text) in packs::PACKS {
            let pack: Pack = serde_json::from_str(text).unwrap();
            let missing = keys.iter().filter(|k| pack.strings.get(*k).is_none_or(|v| v.trim().is_empty())).count();
            assert_eq!(missing, 0, "{code} misses {missing} texts");
        }
    }
}

#[cfg(test)]
mod lookup_tests {
    //! These never call [`set`]: the language is shared by every test running at the same time.
    use super::*;

    fn english_texts() -> Vec<String> {
        serde_json::from_str(include_str!("../assets/i18n/strings.json")).unwrap()
    }

    /// The `{name}` placeholders in a text, sorted.
    fn placeholders(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            rest = &rest[i + 1..];
            match rest.find('}') {
                Some(j) if j > 0 && rest[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                    out.push(rest[..j].to_string());
                    rest = &rest[j + 1..];
                }
                _ => {}
            }
        }
        out.sort();
        out
    }

    #[test]
    fn more_locales_match() {
        assert_eq!(match_locale("de_DE.UTF-8"), "de");
        assert_eq!(match_locale("de-AT"), "de");
        assert_eq!(match_locale("DE"), "de");
        assert_eq!(match_locale("no_NO"), "no");
        assert_eq!(match_locale("nn-NO"), "nn");
        assert_eq!(match_locale("zh_TW"), "zh-Hant");
        assert_eq!(match_locale("zh-HK"), "zh-Hant");
        assert_eq!(match_locale("zh-Hans-CN"), "zh");
        assert_eq!(match_locale("sr-RS"), "sr");
        assert_eq!(match_locale("sr-Cyrl-BA"), "sr-Cyrl-BA");
        assert_eq!(match_locale("ca-ES-valencia"), "ca-ES-valencia");
        assert_eq!(match_locale("ca-ES"), "ca");
        assert_eq!(match_locale("es-ES"), "es");
        assert_eq!(match_locale("es-AR"), "es-MX");
        assert_eq!(match_locale("fr-CA"), "fr-CA");
        assert_eq!(match_locale("fr-BE"), "fr");
        assert_eq!(match_locale("en-GB"), "en-GB");
        assert_eq!(match_locale("en_IE"), "en-GB");
        assert_eq!(match_locale("tl-PH"), "fil");
        assert_eq!(match_locale("fil-PH"), "fil");
        assert_eq!(match_locale("sv-SE@euro"), "sv");
        assert_eq!(match_locale("C"), "en");
        assert_eq!(match_locale("POSIX"), "en");
        assert_eq!(match_locale("--"), "en");
    }

    #[test]
    fn every_code_matches_itself() {
        for c in CODES {
            assert_eq!(match_locale(c), c);
            assert_eq!(known(&c.to_ascii_uppercase()), Some(c));
        }
    }

    #[test]
    fn settings_resolve_to_codes() {
        assert_eq!(resolve("de"), "de");
        assert_eq!(resolve("PT-br"), "pt-BR");
        // Empty, or a language that doesn't exist, means the computer's language.
        assert_eq!(resolve(""), system());
        assert_eq!(resolve("klingon"), system());
        assert!(CODES.contains(&system()));
    }

    #[test]
    fn english_has_no_table() {
        assert!(table_for("en").is_none());
        assert!(table_for("not-a-language").is_none());
    }

    #[test]
    fn a_pack_is_loaded_once_and_translates() {
        let de = table_for("de").unwrap();
        assert!(std::ptr::eq(de, table_for("de").unwrap()), "the table is kept");
        let key = "%Y year  %m month  %d day  %H hour  %M minute  %S second  %n counter  %t title";
        assert!(de[key].contains("Jahr"));
        assert!(de.values().all(|v| !v.trim().is_empty()), "empty entries fall back to English");
    }

    #[test]
    fn translations_keep_the_placeholders() {
        let keys = english_texts();
        for (code, _) in packs::PACKS {
            let table = table_for(code).unwrap();
            for k in &keys {
                if let Some(v) = table.get(k) {
                    assert_eq!(placeholders(v), placeholders(k), "{code}: {k:?} became {v:?}");
                }
            }
        }
    }

    #[test]
    fn placeholder_finder_works() {
        assert_eq!(placeholders("a {x} b {name} {x}"), ["name", "x", "x"]);
        assert!(placeholders("{ } {} {a b} no").is_empty());
    }

    #[test]
    fn placeholders_fill_in_any_order_and_repeat() {
        assert_eq!(tf("{a} and {b}", &[("b", "2"), ("a", "1")]), "1 and 2");
        assert_eq!(tf("{a}{a}", &[("a", "x")]), "xx");
        assert_eq!(tf("{a} {missing}", &[("a", "x")]), "x {missing}", "an unknown name stays as it is");
        assert_eq!(tf("no names", &[("a", "x")]), "no names");
        assert_eq!(tf("{a}", &[("a", "{b}"), ("b", "y")]), "y", "replacements are applied one after another");
        assert_eq!(tf("{e}", &[("e", "")]), "");
    }

    #[test]
    fn writing_direction_and_scripts() {
        for rtl in ["ar", "fa", "he", "ur", "ug", "ar-EG"] {
            assert!(is_rtl(rtl), "{rtl}");
        }
        for ltr in ["en", "de", "hi", "zh", "", "arn"] {
            assert!(!is_rtl(ltr), "{ltr}");
        }
        for cjk in ["zh", "zh-Hant", "ja", "ko"] {
            assert!(is_cjk(cjk), "{cjk}");
        }
        for not in ["en", "th", "vi", "", "kok"] {
            assert!(!is_cjk(not), "{not}");
        }
    }

    #[test]
    fn norwegian_is_called_by_its_name_not_its_code() {
        assert_eq!(native_name("no"), "Norsk bokmål");
        assert_eq!(name_in("no", "en"), "Norwegian Bokmål");
        assert_eq!(name_in("no", "de"), "Norwegisch (Bokmål)");
        assert_eq!(native_name("nn"), "Norsk nynorsk");
        // Every language in the list has a real name in itself, never just its code.
        for code in CODES {
            assert_ne!(native_name(code), *code, "{code} has no name in itself");
        }
    }

    #[test]
    fn language_names() {
        assert_eq!(native_name("de"), "Deutsch");
        assert_eq!(native_name("ja"), "日本語");
        assert_eq!(name_in("ja", "en"), "Japanese");
        assert_eq!(name_in("de", "fr"), "Allemand");
        // An unknown interface language falls back to English, an unknown code to itself.
        assert_eq!(name_in("de", "xx"), "German");
        assert_eq!(name_in("xx", "de"), "xx");
        assert!(!name("de").is_empty());
    }

    #[test]
    fn packs_cover_every_language_but_english() {
        let codes: Vec<&str> = CODES.iter().copied().filter(|c| *c != "en").collect();
        let packs: Vec<&str> = packs::PACKS.iter().map(|(c, _)| *c).collect();
        assert_eq!(packs, codes, "PACKS and CODES must list the same languages in the same order");
        let unique: std::collections::HashSet<&str> = CODES.iter().copied().collect();
        assert_eq!(unique.len(), CODES.len());
        for (code, text) in packs::PACKS {
            assert!(!text.trim().is_empty(), "{code}");
        }
    }
}
