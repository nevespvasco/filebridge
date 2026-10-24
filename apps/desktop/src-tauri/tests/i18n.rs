//! The UI's strings (apps/desktop/src/i18n): English and Portuguese have the same keys, the same
//! number of plural forms and the same placeholders, and no string quotes a number of wizard steps
//! other than the wizard's own (`SEND_STEPS` in stores/wizard.ts; UX audit P3).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

const EN: &str = include_str!("../../src/i18n/en.json");
const PT: &str = include_str!("../../src/i18n/pt.json");
const WIZARD: &str = include_str!("../../src/stores/wizard.ts");

/// Every string, by its dotted key.
fn flatten(json: &str) -> BTreeMap<String, String> {
    fn walk(v: &Value, path: &str, out: &mut BTreeMap<String, String>) {
        match v {
            Value::Object(map) => {
                for (k, v) in map {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(v, &p, out);
                }
            }
            Value::String(s) => {
                out.insert(path.to_owned(), s.clone());
            }
            other => panic!("{path}: not a string: {other}"),
        }
    }
    let mut out = BTreeMap::new();
    walk(
        &serde_json::from_str(json).expect("valid JSON"),
        "",
        &mut out,
    );
    out
}

/// `{name}`-style placeholders (not vue-i18n's literal `{'|'}`).
fn placeholders(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let inner = &rest[start + 1..start + len];
        if !inner.starts_with('\'') {
            out.insert(inner.to_owned());
        }
        rest = &rest[start + len + 1..];
    }
    out
}

/// Plural forms are separated by " | ".
fn forms(s: &str) -> usize {
    s.matches(" | ").count() + 1
}

#[test]
fn english_and_portuguese_have_the_same_keys() {
    let (en, pt) = (flatten(EN), flatten(PT));
    let only_en: Vec<_> = en.keys().filter(|k| !pt.contains_key(*k)).collect();
    let only_pt: Vec<_> = pt.keys().filter(|k| !en.contains_key(*k)).collect();
    assert!(
        only_en.is_empty() && only_pt.is_empty(),
        "only in en: {only_en:?}\nonly in pt: {only_pt:?}"
    );
}

#[test]
fn plural_forms_and_placeholders_match() {
    let (en, pt) = (flatten(EN), flatten(PT));
    let mut wrong = Vec::new();
    for (k, e) in &en {
        let Some(p) = pt.get(k) else { continue };
        if forms(e) != forms(p) {
            wrong.push(format!(
                "{k}: {} plural forms in en, {} in pt",
                forms(e),
                forms(p)
            ));
        }
        if placeholders(e) != placeholders(p) {
            wrong.push(format!(
                "{k}: placeholders {:?} in en, {:?} in pt",
                placeholders(e),
                placeholders(p)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn no_string_quotes_another_number_of_steps() {
    let steps: u32 = WIZARD
        .lines()
        .find_map(|l| l.split("export const SEND_STEPS = ").nth(1))
        .and_then(|rest| rest.trim_end_matches(';').trim().parse().ok())
        .expect("SEND_STEPS in stores/wizard.ts");
    let words: [(&str, u32); 14] = [
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("um", 1),
        ("dois", 2),
        ("três", 3),
        ("quatro", 4),
        ("cinco", 5),
        ("seis", 6),
        ("sete", 7),
    ];
    let mut wrong = Vec::new();
    for (lang, json) in [("en", EN), ("pt", PT)] {
        for (k, s) in flatten(json) {
            let lower = s.to_lowercase();
            let tokens: Vec<&str> = lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
                .collect();
            for pair in tokens.windows(2) {
                if !matches!(pair[1], "steps" | "passos") {
                    continue;
                }
                let n = pair[0]
                    .parse::<u32>()
                    .ok()
                    .or_else(|| words.iter().find(|(w, _)| *w == pair[0]).map(|(_, n)| *n));
                if let Some(n) = n.filter(|n| *n != steps) {
                    wrong.push(format!(
                        "{lang} {k}: says {n} steps, the wizard has {steps}: {s}"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
