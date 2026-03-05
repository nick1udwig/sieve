use sieve_types::SinkKey;
use std::collections::BTreeSet;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum SinkCanonicalizationError {
    #[error("invalid URL sink: {0}")]
    Invalid(#[from] url::ParseError),
}

pub fn canonicalize_sink_key(raw: &str) -> Result<String, SinkCanonicalizationError> {
    let mut url = Url::parse(raw)?;
    url.set_query(None);
    url.set_fragment(None);

    let normalized = Url::parse(url.as_str())?;
    let mut out = normalized;

    let is_http_default = out.scheme() == "http" && out.port() == Some(80);
    let is_https_default = out.scheme() == "https" && out.port() == Some(443);
    if is_http_default || is_https_default {
        let _ = out.set_port(None);
    }

    let host = out.host_str().unwrap_or_default();
    let port_part = out.port().map(|p| format!(":{p}")).unwrap_or_default();
    let raw_path = if out.path().is_empty() {
        "/"
    } else {
        out.path()
    };
    let path = normalize_percent_encoding(raw_path);

    Ok(format!("{}://{}{}{}", out.scheme(), host, port_part, path))
}

fn normalize_percent_encoding(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1];
            let h2 = bytes[i + 2];
            if let (Some(n1), Some(n2)) = (hex_val(h1), hex_val(h2)) {
                let value = (n1 << 4) | n2;
                if is_unreserved(value) {
                    out.push(value as char);
                } else {
                    out.push('%');
                    out.push(hex_upper(n1));
                    out.push(hex_upper(n2));
                }
                i += 3;
                continue;
            }
        }

        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_upper(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => '0',
    }
}

fn is_unreserved(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~')
}

pub fn canonicalize_sink_set(sinks: &BTreeSet<SinkKey>) -> BTreeSet<SinkKey> {
    sinks
        .iter()
        .map(|sink| {
            let normalized = canonicalize_sink_key(&sink.0).unwrap_or_else(|_| sink.0.clone());
            SinkKey(normalized)
        })
        .collect()
}
