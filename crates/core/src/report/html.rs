//! A sealed report's readable copy: one self-contained HTML page (no script, nothing fetched),
//! rendered only from what the sealed JSON holds, so `check` renders it again and compares it
//! byte for byte (ADR 0025). Its bytes are part of the seal scheme: a new layout is a new
//! [`super::seal::SCHEME`].

use std::io::{self, Write};

use serde_json::Value;

use super::ReportRow;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A header value as text: a string as it is, nothing for null, anything else as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

const STYLE: &str = "body{font:14px/1.45 system-ui,sans-serif;margin:24px;color:#1b1b1f;background:#fff}\
h1{font-size:20px}dl{display:grid;grid-template-columns:max-content 1fr;gap:4px 16px}dt{color:#666}dd{margin:0}\
table{border-collapse:collapse;width:100%;font-size:13px}th,td{text-align:left;padding:4px 8px;border-bottom:1px solid #ddd;vertical-align:top;overflow-wrap:anywhere}\
td.n{text-align:right;font-variant-numeric:tabular-nums}code{font-size:12px}.s-verified{color:#17803d}\
.s-different,.s-missing,.s-failed,.s-unverifiable{color:#b42318;font-weight:600}footer{margin-top:24px;color:#444}\
@media (prefers-color-scheme:dark){body{color:#e6e6ea;background:#151517}dt{color:#9a9aa3}th,td{border-color:#333}\
.s-verified{color:#4ade80}.s-different,.s-missing,.s-failed,.s-unverifiable{color:#f87171}footer{color:#b5b5bd}}";

pub(super) fn begin(w: &mut dyn Write, head: &Value) -> io::Result<()> {
    let t = &head["transfer"];
    let d = &head["destination"];
    let label = text(&t["label"]);
    let name = if label.is_empty() {
        text(&t["id"])
    } else {
        label.clone()
    };
    write!(
        w,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>filebridge report: {}</title><style>{STYLE}</style></head><body>\n\
         <h1>filebridge report</h1>\n<dl>\n",
        esc(&name)
    )?;
    let fields = [
        ("Transfer", esc(&label)),
        (
            "Transfer id",
            format!("<code>{}</code>", esc(&text(&t["id"]))),
        ),
        (
            "Destination",
            esc(&format!(
                "{} ({}, root {})",
                text(&d["profile"]),
                text(&d["backend"]),
                text(&d["root"])
            )),
        ),
        ("Created", esc(&text(&t["created_at"]))),
        ("Finished", esc(&text(&t["finished_at"]))),
        ("State", esc(&text(&t["state"]))),
        (
            "Files",
            esc(&format!(
                "{} · {} bytes",
                text(&head["counts"]["files"]),
                text(&head["counts"]["bytes"])
            )),
        ),
        ("filebridge", esc(&text(&head["app_version"]))),
    ];
    for (k, v) in fields {
        writeln!(w, "<dt>{k}</dt><dd>{v}</dd>")?;
    }
    w.write_all(b"</dl>\n")?;
    if let Some(warnings) = head["warnings"].as_array().filter(|a| !a.is_empty()) {
        w.write_all(b"<ul>\n")?;
        for warning in warnings {
            writeln!(w, "<li>{}</li>", esc(&text(warning)))?;
        }
        w.write_all(b"</ul>\n")?;
    }
    w.write_all(
        b"<table><thead><tr><th>#</th><th>Source</th><th>Destination</th><th>Size</th>\
          <th>Status</th><th>Renamed by</th><th>SHA-256</th></tr></thead><tbody>\n",
    )
}

/// The `index`-th entry (0-based), shown from 1.
pub(super) fn row(w: &mut dyn Write, index: u64, r: &ReportRow) -> io::Result<()> {
    let class: String = r
        .status
        .chars()
        .filter(|c| c.is_ascii_lowercase() || *c == '_')
        .collect();
    let error = r
        .error
        .as_deref()
        .map(|e| format!("<br><small>{}</small>", esc(e)))
        .unwrap_or_default();
    let dir = if r.is_dir { "/" } else { "" };
    writeln!(
        w,
        "<tr><td class=\"n\">{}</td><td>{}{dir}</td><td>{}</td><td class=\"n\">{}</td>\
         <td class=\"s-{class}\">{}{error}</td><td>{}</td><td><code>{}</code></td></tr>",
        index + 1,
        esc(&r.source_path),
        esc(r.destination_path.as_deref().unwrap_or("")),
        r.size,
        esc(&r.status),
        esc(&r.renamed_by.join(", ")),
        esc(r.sha256.as_deref().unwrap_or("")),
    )
}

pub(super) fn end(w: &mut dyn Write, entries: u64, sha256: &str) -> io::Result<()> {
    write!(
        w,
        "</tbody></table>\n<footer><p>Sealed: {entries} entries, SHA-256 <code>{}</code>.</p>\
         <p>The proof is the .json file with the same name. Any change to either file shows in \
         <code>filebridge report check</code>; keep the SHA-256 above somewhere else to prove \
         both unchanged.</p></footer>\n</body></html>\n",
        esc(sha256)
    )
}
