//! The tamper-evident report (ADR 0025): the JSON report with a SHA-256 digest chain over its
//! header and entries, a final SHA-256, and a readable HTML copy beside it. A plain digest, no
//! signing key: it shows any later edit, accidental or not; against someone who rewrites the
//! file and its digests on purpose, the final SHA-256 kept elsewhere is the proof.
//!
//! The chain, with `canonical` the JSON text with keys sorted and no spaces:
//!
//! ```text
//! h0 = SHA-256("filebridge-seal/1\n" ‖ canonical(header))  header: the top-level object without
//!                                                            "entries" and "seal"
//! hi = SHA-256(h(i-1) ‖ canonical(entry i))                 entry i without its "chain" member;
//!                                                            hex(hi) is that member
//! final = SHA-256(hn ‖ "\n" ‖ n in decimal)
//! "seal": {"scheme": "filebridge-seal/1", "header": hex(h0), "entries": n, "sha256": hex(final)}
//! ```
//!
//! Each link is checked against the one stored before it, so an edit is pinned to its entry.

use std::io::{self, BufRead, Read, Write};
use std::path::Path;

use filebridge_model::{ReportCheck, SealProblem};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{ReportRow, hex, html, unhex, write_json_head};

/// The seal's scheme: the chain above and the readable copy's layout ([`html`]).
pub const SCHEME: &str = "filebridge-seal/1";
/// Entries listed by a check; the others are only counted.
const MAX_LISTED: usize = 50;

type Link = [u8; 32];

/// `v` as JSON text with every object's keys sorted and no whitespace: the same bytes whatever
/// order a reader keeps members in.
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        v => out.push_str(&v.to_string()),
    }
}

fn header_link(head: &Value) -> Link {
    let mut h = Sha256::new();
    h.update(SCHEME.as_bytes());
    h.update(b"\n");
    h.update(canonical(head).as_bytes());
    h.finalize().into()
}

fn link(prev: &Link, canonical_entry: &str) -> Link {
    let mut h = Sha256::new();
    h.update(prev);
    h.update(canonical_entry.as_bytes());
    h.finalize().into()
}

fn final_digest(last: &Link, entries: u64) -> Link {
    let mut h = Sha256::new();
    h.update(last);
    h.update(format!("\n{entries}").as_bytes());
    h.finalize().into()
}

fn parse_link(s: Option<&str>) -> Option<Link> {
    s.and_then(unhex)?.try_into().ok()
}

/// Writes a sealed report as its rows come: the JSON to `json`, the readable copy to `html`.
pub struct Sealer<J: Write, H: Write> {
    json: J,
    html: H,
    header: Link,
    last: Link,
    entries: u64,
}

impl<J: Write, H: Write> Sealer<J, H> {
    /// `head`: the report's top-level members ([`super::json_head`]).
    pub fn begin(mut json: J, mut html: H, head: &Value) -> io::Result<Self> {
        write_json_head(&mut json, head)?;
        html::begin(&mut html, head)?;
        let header = header_link(head);
        Ok(Self {
            json,
            html,
            header,
            last: header,
            entries: 0,
        })
    }

    pub fn row(&mut self, mut row: ReportRow) -> io::Result<()> {
        row.chain = None;
        self.last = link(&self.last, &canonical(&serde_json::to_value(&row)?));
        html::row(&mut self.html, self.entries, &row)?;
        row.chain = Some(hex(&self.last));
        if self.entries > 0 {
            self.json.write_all(b",")?;
        }
        self.json.write_all(b"\n")?;
        serde_json::to_writer(&mut self.json, &row)?;
        self.entries += 1;
        Ok(())
    }

    /// Closes both files; gives them back with the final SHA-256 (hex).
    pub fn finish(mut self) -> io::Result<(J, H, String)> {
        let digest = hex(&final_digest(&self.last, self.entries));
        let seal = serde_json::json!({
            "scheme": SCHEME,
            "header": hex(&self.header),
            "entries": self.entries,
            "sha256": digest,
        });
        write!(self.json, "\n],\"seal\":{seal}}}\n")?;
        html::end(&mut self.html, self.entries, &digest)?;
        self.json.flush()?;
        self.html.flush()?;
        Ok((self.json, self.html, digest))
    }
}

/// A writer that compares what is written with a file's bytes instead of writing them.
struct Compare<R: BufRead> {
    file: R,
    same: bool,
    buf: Vec<u8>,
}

impl<R: BufRead> Compare<R> {
    /// The readable copy's lines up to the table's first row, read ahead: the summary above
    /// them is compared once the whole header is known, wherever its members are in the JSON.
    fn top(&mut self) -> Vec<u8> {
        let mut top = Vec::new();
        loop {
            let start = top.len();
            match self.file.read_until(b'\n', &mut top) {
                Ok(0) | Err(_) => {
                    self.same = false;
                    return top;
                }
                Ok(_) if top[start..].ends_with(b"<tbody>\n") => return top,
                Ok(_) => {}
            }
        }
    }

    /// Whether everything written matched and the file has nothing more.
    fn finish(mut self) -> bool {
        self.same && self.file.fill_buf().is_ok_and(|rest| rest.is_empty())
    }
}

impl<R: BufRead> Write for Compare<R> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.same {
            self.buf.resize(data.len(), 0);
            self.same = self.file.read_exact(&mut self.buf).is_ok() && self.buf == data;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What the check gathered while reading the JSON.
struct State<R: BufRead> {
    /// The first entry's canonical text and stored link: checked once the seal's header link is
    /// read, at the end.
    first: Option<(String, Option<Link>, String)>,
    /// The link stored in the previous entry (or computed, when it had none).
    prev: Option<Link>,
    entries: u64,
    chained: bool,
    bad: Vec<(u64, String)>,
    more: u64,
    html: Option<Compare<R>>,
    head: serde_json::Map<String, Value>,
    seal: Option<Value>,
}

impl<R: BufRead> State<R> {
    fn bad(&mut self, index: u64, path: String) {
        if self.bad.len() < MAX_LISTED {
            self.bad.push((index, path));
        } else {
            self.more += 1;
        }
    }

    fn entry(&mut self, mut v: Value) {
        let index = self.entries;
        self.entries += 1;
        let stored = v
            .as_object_mut()
            .and_then(|m| m.remove("chain"))
            .map(|c| {
                self.chained = true;
                parse_link(c.as_str())
            })
            .unwrap_or(None);
        let canon = canonical(&v);
        let row: Option<ReportRow> = serde_json::from_value(v).ok();
        let path = row
            .as_ref()
            .map(|r| r.source_path.clone())
            .unwrap_or_default();
        if let Some(c) = &mut self.html {
            match &row {
                Some(r) => {
                    let _ = html::row(c, index, r);
                }
                None => c.same = false,
            }
        }
        match self.prev {
            None => {
                self.prev = stored;
                self.first = Some((canon, stored, path));
            }
            Some(prev) => {
                let computed = link(&prev, &canon);
                if stored != Some(computed) {
                    self.bad(index, path);
                }
                self.prev = Some(stored.unwrap_or(computed));
            }
        }
    }
}

struct Top<'s, R: BufRead>(&'s mut State<R>);

impl<'de, R: BufRead> DeserializeSeed<'de> for Top<'_, R> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de, R: BufRead> Visitor<'de> for Top<'_, R> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a filebridge report")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut entries = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "entries" if entries => return Err(de::Error::custom("two entries arrays")),
                "entries" => {
                    entries = true;
                    map.next_value_seed(Entries(&mut *self.0))?;
                }
                "seal" => self.0.seal = Some(map.next_value()?),
                _ => {
                    let v = map.next_value()?;
                    self.0.head.insert(key, v);
                }
            }
        }
        if !entries {
            return Err(de::Error::custom("no entries"));
        }
        Ok(())
    }
}

struct Entries<'s, R: BufRead>(&'s mut State<R>);

impl<'de, R: BufRead> DeserializeSeed<'de> for Entries<'_, R> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, R: BufRead> Visitor<'de> for Entries<'_, R> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the report's entries")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            self.0.entry(v);
        }
        Ok(())
    }
}

/// Checks a sealed report read from `json`, and its readable copy `html` when given. `path` and
/// `readable` only name them in the answer.
pub fn check<R: BufRead>(
    json: impl Read,
    html: Option<R>,
    path: String,
    readable: Option<String>,
) -> ReportCheck {
    let mut html = html.map(|file| Compare {
        file,
        same: true,
        buf: Vec::new(),
    });
    let top = html.as_mut().map(Compare::top);
    let mut st = State {
        first: None,
        prev: None,
        entries: 0,
        chained: false,
        bad: Vec::new(),
        more: 0,
        html,
        head: serde_json::Map::new(),
        seal: None,
    };
    let mut out = ReportCheck {
        intact: false,
        path,
        readable,
        entries: 0,
        sha256: None,
        problems: Vec::new(),
        more: 0,
    };
    let mut de = serde_json::Deserializer::from_reader(io::BufReader::new(json));
    let parsed = Top(&mut st).deserialize(&mut de).and_then(|()| de.end());
    out.entries = st.entries;
    if let Err(e) = parsed {
        out.problems.push(SealProblem::Unreadable {
            detail: e.to_string(),
        });
        return out;
    }
    let Some(seal) = st.seal.take() else {
        out.problems.push(if st.chained {
            SealProblem::Seal
        } else {
            SealProblem::NotSealed
        });
        return out;
    };
    if seal["scheme"] != SCHEME {
        out.problems.push(SealProblem::Unreadable {
            detail: format!("unknown seal scheme {}", seal["scheme"]),
        });
        return out;
    }
    out.sha256 = seal["sha256"].as_str().map(str::to_owned);
    let head = Value::Object(std::mem::take(&mut st.head));
    let header = header_link(&head);
    let stored_header = parse_link(seal["header"].as_str());
    if stored_header != Some(header) {
        out.problems.push(SealProblem::Header);
    }
    // The chain starts at the stored header link, so an edited header isn't also blamed on the
    // first entry.
    let start = stored_header.unwrap_or(header);
    if let Some((canon, stored, path)) = st.first.take() {
        let computed = link(&start, &canon);
        if stored != Some(computed) {
            st.bad.insert(0, (0, path));
            if st.bad.len() > MAX_LISTED {
                st.bad.pop();
                st.more += 1;
            }
        }
    }
    for (index, source_path) in st.bad.drain(..) {
        out.problems.push(SealProblem::Entry { index, source_path });
    }
    out.more = st.more;
    let last = st.prev.unwrap_or(start);
    let final_ok = seal["entries"].as_u64() == Some(st.entries)
        && parse_link(out.sha256.as_deref()) == Some(final_digest(&last, st.entries));
    if !final_ok {
        out.problems.push(SealProblem::Seal);
    }
    if let Some(mut c) = st.html.take() {
        let mut expected = Vec::new();
        let _ = html::begin(&mut expected, &head);
        c.same &= top.is_some_and(|top| top == expected);
        let _ = html::end(&mut c, st.entries, out.sha256.as_deref().unwrap_or(""));
        if !c.finish() {
            out.problems.push(SealProblem::Readable);
        }
    }
    out.intact = out.problems.is_empty();
    out
}

/// Checks a sealed report file: `path` is the JSON or its readable `.html` copy; the other one,
/// beside it with the same name, is checked too when it is there.
pub fn check_file(path: &Path) -> io::Result<ReportCheck> {
    let is_html = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html"));
    let (json, readable) = if is_html {
        (path.with_extension("json"), path.to_owned())
    } else {
        (path.to_owned(), path.with_extension("html"))
    };
    let file = std::fs::File::open(&json)?;
    let html = match std::fs::File::open(&readable) {
        Ok(f) => Some(io::BufReader::new(f)),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !is_html => None,
        Err(e) => return Err(e),
    };
    let readable = html.as_ref().map(|_| readable.display().to_string());
    Ok(check(file, html, json.display().to_string(), readable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<ReportRow> {
        ["Fotos/a.jpg", "Fotos/b <c>.jpg", "notes.txt"]
            .iter()
            .enumerate()
            .map(|(i, p)| ReportRow {
                source_path: (*p).into(),
                source_path_hex: None,
                destination_path: Some(p.replace(['<', '>'], "_")),
                is_dir: false,
                size: 10 + i as u64,
                mtime: Some(1_700_000_000),
                category: "image".into(),
                status: "verified".into(),
                renamed_by: if i == 1 {
                    vec!["forbidden-chars".into()]
                } else {
                    Vec::new()
                },
                sha256: Some(format!("{i:064x}")),
                error: None,
                chain: None,
            })
            .collect()
    }

    fn head() -> Value {
        serde_json::json!({
            "format": "filebridge-report/1",
            "transfer": {"id": "j1", "label": "Fotos & co", "state": "completed",
                         "created_at": "2026-10-21T21:58:02Z", "finished_at": null,
                         "options": {"verify": true}},
            "destination": {"profile": "Home", "transport": "rclone", "backend": "sftp",
                            "root": "demo", "integration": null},
            "counts": {"files": 3, "bytes": 33},
            "verify": {"algo": "sha256", "rounds": 0},
            "warnings": ["one <b>warning</b>"],
            "app_version": "0.0.1",
        })
    }

    fn sealed(rows: Vec<ReportRow>) -> (String, String, String) {
        let mut s = Sealer::begin(Vec::new(), Vec::new(), &head()).unwrap();
        for r in rows {
            s.row(r).unwrap();
        }
        let (json, html, digest) = s.finish().unwrap();
        (
            String::from_utf8(json).unwrap(),
            String::from_utf8(html).unwrap(),
            digest,
        )
    }

    fn run(json: &str, html: Option<&str>) -> ReportCheck {
        check(
            json.as_bytes(),
            html.map(str::as_bytes),
            "r.json".into(),
            None,
        )
    }

    #[test]
    fn an_untouched_report_and_its_copy_check_out() {
        let (json, html, digest) = sealed(rows());
        let c = run(&json, Some(&html));
        assert!(c.intact, "{c:?}");
        assert_eq!(c.entries, 3);
        assert_eq!(c.sha256.as_deref(), Some(digest.as_str()));
        assert!(html.contains(&digest), "the copy shows the seal");
        assert!(html.contains("Fotos/b &lt;c&gt;.jpg"), "escaped");
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["entries"][2]["chain"].as_str().unwrap().len(), 64);
        assert_eq!(v["seal"]["entries"], 3);
    }

    #[test]
    fn the_same_content_seals_the_same() {
        assert_eq!(sealed(rows()).2, sealed(rows()).2);
        assert_ne!(sealed(rows()).2, sealed(rows()[..2].to_vec()).2);
    }

    #[test]
    fn reformatting_the_members_order_still_checks_out() {
        // A reader that rewrites the JSON with other member order and spacing changes no
        // content: the chain is over canonical text.
        let (json, _, _) = sealed(rows());
        let v: Value = serde_json::from_str(&json).unwrap();
        let pretty = serde_json::to_string_pretty(&v).unwrap();
        assert!(run(&pretty, None).intact);
    }

    #[test]
    fn an_edited_entry_is_pinned_to_its_index() {
        let (json, html, _) = sealed(rows());
        let edited = json.replacen("\"size\":11", "\"size\":12", 1);
        let c = run(&edited, Some(&html));
        assert!(!c.intact);
        assert_eq!(
            c.problems,
            [
                SealProblem::Entry {
                    index: 1,
                    source_path: "Fotos/b <c>.jpg".into()
                },
                SealProblem::Readable,
            ],
            "the copy no longer matches the edited JSON either"
        );
    }

    #[test]
    fn a_removed_entry_a_changed_header_and_a_cut_end_are_found() {
        let (json, _, _) = sealed(rows());
        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["entries"].as_array_mut().unwrap().remove(1);
        let c = run(&v.to_string(), None);
        assert_eq!(
            c.problems,
            [
                SealProblem::Entry {
                    index: 1,
                    source_path: "notes.txt".into()
                },
                SealProblem::Seal
            ]
        );

        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["destination"]["root"] = "elsewhere".into();
        assert_eq!(run(&v.to_string(), None).problems, [SealProblem::Header]);

        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["entries"].as_array_mut().unwrap().pop();
        assert_eq!(run(&v.to_string(), None).problems, [SealProblem::Seal]);

        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["entries"][0]["note"] = "added".into();
        assert!(matches!(
            run(&v.to_string(), None).problems[..],
            [SealProblem::Entry { index: 0, .. }]
        ));
    }

    #[test]
    fn an_edited_copy_is_found() {
        let (json, html, _) = sealed(rows());
        let c = run(&json, Some(&html.replace("verified", "verifled")));
        assert_eq!(c.problems, [SealProblem::Readable]);
        let c = run(&json, Some(&format!("{html}<!-- -->")));
        assert_eq!(
            c.problems,
            [SealProblem::Readable],
            "extra bytes at the end"
        );
    }

    #[test]
    fn a_plain_report_or_garbage_is_not_intact() {
        let plain = "{\"format\":\"filebridge-report/1\",\"entries\":[\n{\"a\":1}\n]}";
        assert_eq!(run(plain, None).problems, [SealProblem::NotSealed]);
        assert!(matches!(
            run("{\"entries\": [", None).problems[..],
            [SealProblem::Unreadable { .. }]
        ));
        assert!(matches!(
            run("[]", None).problems[..],
            [SealProblem::Unreadable { .. }]
        ));
    }

    #[test]
    fn canonical_text_sorts_keys_at_every_level() {
        let v = serde_json::json!({"b": [{"z": 1, "a": "x\"y"}], "a": null});
        assert_eq!(canonical(&v), r#"{"a":null,"b":[{"a":"x\"y","z":1}]}"#);
    }
}
