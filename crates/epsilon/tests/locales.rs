//! The language files in ui/locales must cover every message the engine and the
//! interface use, and every language must have exactly the keys and placeholders of English.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn locale(name: &str) -> BTreeMap<String, Value> {
    let json: Value = serde_json::from_str(&read(&format!("ui/locales/{name}"))).unwrap_or_else(|e| panic!("{name}: {e}"));
    match json {
        Value::Object(m) => m.into_iter().collect(),
        _ => panic!("{name} must be an object"),
    }
}

fn locales() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root().join("ui/locales"))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
}

/// Every `{name}` in a value, across all its plural forms.
fn placeholders(v: &Value) -> BTreeSet<String> {
    let texts: Vec<&str> = match v {
        Value::String(s) => vec![s],
        Value::Object(forms) => forms.values().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    let mut out = BTreeSet::new();
    for t in texts {
        let mut rest = t;
        while let Some(start) = rest.find('{') {
            let Some(len) = rest[start..].find('}') else { break };
            out.insert(rest[start + 1..start + len].to_string());
            rest = &rest[start + len..];
        }
    }
    out
}

/// Every string literal that follows `prefix` in `source`, e.g. the keys in `msg!("...")`.
fn literals_after(source: &str, prefix: &str, quote: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find(prefix) {
        rest = &rest[at + prefix.len()..];
        if let Some(end) = rest.find(quote) {
            out.push(rest[..end].to_string());
        }
    }
    out
}

/// The first string argument of every call to the function `name`, e.g. `t("key")`,
/// skipping longer names that end the same way, such as `split(".")`.
fn calls(source: &str, name: &str) -> Vec<String> {
    let pattern = format!("{name}(\"");
    let mut out = Vec::new();
    for (at, _) in source.match_indices(&pattern) {
        let before = source[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
            continue;
        }
        let rest = &source[at + pattern.len()..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
        }
    }
    out
}

/// Variant names of a Rust enum, in camelCase as serde writes them.
fn enum_variants(source: &str, name: &str) -> Vec<String> {
    let start = source.find(&format!("pub enum {name} {{")).unwrap_or_else(|| panic!("enum {name}"));
    let body = &source[start..start + source[start..].find("\n}").unwrap()];
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && !l.starts_with("pub enum") && l.ends_with(','))
        .map(|l| {
            let v = l.trim_end_matches(',');
            let mut c = v.chars();
            c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

#[test]
fn every_language_matches_english() {
    let en = locale("en.json");
    for name in locales() {
        let other = locale(&name);
        let missing: Vec<&String> = en.keys().filter(|k| !other.contains_key(*k)).collect();
        let extra: Vec<&String> = other.keys().filter(|k| !en.contains_key(*k)).collect();
        assert!(missing.is_empty() && extra.is_empty(), "{name}: missing {missing:?}, extra {extra:?}");
        for (key, value) in &other {
            if let Value::Object(forms) = value {
                assert!(forms.contains_key("other"), "{name}: {key} has plural forms but no \"other\"");
            }
            assert_eq!(placeholders(value), placeholders(&en[key]), "{name}: placeholders of {key} differ from English");
        }
    }
}

#[test]
fn english_has_every_key_in_use() {
    let en = locale("en.json");
    let mut used: BTreeSet<String> = BTreeSet::new();

    // The engine.
    let analyze = read("crates/epsilon/src/analyze.rs");
    used.extend(literals_after(&analyze, "msg!(\n", '"').into_iter().filter(|k| k.contains('.')));
    used.extend(literals_after(&analyze, "msg!(\"", '"'));
    let analyze_lines = analyze.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    used.extend(literals_after(&analyze_lines, "msg!(\n\"", '"'));
    for v in enum_variants(&analyze, "Category") {
        used.insert(format!("category.{v}"));
    }
    for v in enum_variants(&analyze, "SavingKind") {
        used.insert(format!("saving.{v}"));
    }
    let images = read("crates/epsilon/src/images.rs");
    let unsupported = &images[images.find("pub fn unsupported").unwrap()..images.find("pub fn colour_name").unwrap()];
    for code in literals_after(unsupported, "Some(\"", '"').into_iter().chain(literals_after(unsupported, "=> \"", '"')) {
        used.insert(format!("imageReason.{code}"));
    }
    for colour in ["Gray", "RGB", "CMYK", "Indexed", "ICC", "Mask", "Stencil"] {
        used.insert(format!("colour.{colour}"));
    }
    let lib = read("crates/epsilon/src/lib.rs");
    let codes = &lib[lib.find("pub fn code(&self)").unwrap()..lib.find("pub fn detail(&self)").unwrap()];
    for code in literals_after(codes, "=> \"", '"') {
        used.insert(format!("error.{code}"));
    }
    for code in literals_after(&read("src-tauri/src/main.rs"), "code: \"", '"') {
        used.insert(format!("error.{code}"));
    }
    for preset in ["lossless", "print", "balanced", "small"] {
        used.insert(format!("preset.{preset}"));
        used.insert(format!("presetNote.{preset}"));
    }

    // The interface: t("..."), data-i18n="..." and the part labels.
    let app = read("ui/app.js");
    used.extend(calls(&app, "t"));
    used.extend(literals_after(&app, "\"part.", '"').into_iter().map(|k| format!("part.{k}")));
    used.extend(literals_after(&app, "? \"save.", '"').into_iter().map(|k| format!("save.{k}")));
    used.extend(literals_after(&app, ": \"save.", '"').into_iter().map(|k| format!("save.{k}")));
    used.extend(literals_after(&app, "? \"image.", '"').into_iter().map(|k| format!("image.{k}")));
    used.extend(literals_after(&app, ": \"image.", '"').into_iter().map(|k| format!("image.{k}")));
    let html = read("ui/index.html");
    used.extend(literals_after(&html, "data-i18n=\"", '"'));
    used.extend(literals_after(&html, "data-i18n-label=\"", '"'));

    assert!(used.len() > 150, "found only {} keys; the scan is broken", used.len());
    let missing: Vec<&String> = used.iter().filter(|k| !en.contains_key(*k)).collect();
    assert!(missing.is_empty(), "missing from en.json: {missing:?}");
}

/// Names drawn as tags are marked `[[like this]]`. Every mark must close, and each language
/// must tag as many names in a string as English does.
#[test]
fn name_tags_are_closed_and_match_english() {
    fn tags(v: &Value) -> Vec<usize> {
        let texts: Vec<&str> = match v {
            Value::String(s) => vec![s.as_str()],
            Value::Object(forms) => forms.values().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        texts
            .iter()
            .map(|t| {
                let (open, close) = (t.matches("[[").count(), t.matches("]]").count());
                assert_eq!(open, close, "unbalanced [[ ]] in {t:?}");
                assert!(!t.contains("[[[") && !t.contains("]]]"), "doubled brackets in {t:?}");
                open
            })
            .collect()
    }
    let en = locale("en.json");
    for name in locales() {
        for (key, value) in locale(&name) {
            let here: usize = tags(&value).into_iter().max().unwrap_or(0);
            let english: usize = tags(&en[&key]).into_iter().max().unwrap_or(0);
            assert_eq!(here, english, "{name}: {key} tags {here} names, English {english}");
        }
    }
}
