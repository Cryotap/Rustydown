//! Dependency-free logic (planning, pacing, parsing, formatting).
//!
//! Everything here uses only `std`, so it is unit-tested on any host:
//!     rustc --edition 2021 --test src/logic.rs -o /tmp/logic_tests && /tmp/logic_tests
#![allow(dead_code)]

/// Smallest piece worth giving to a separate connection.
pub const MIN_SPLIT: u64 = 512 * 1024;
/// Sentinel "end" for downloads whose size is unknown (streamed on one connection).
pub const UNKNOWN_END: u64 = u64::MAX;

// ───────────────────────── segment planning ─────────────────────────

/// Initial (start, end) byte ranges for `total` bytes over up to `conns` connections.
pub fn plan_segments(total: u64, conns: u32) -> Vec<(u64, u64)> {
    if total == 0 {
        return vec![];
    }
    let parts = (conns.max(1) as u64).min((total / MIN_SPLIT).max(1));
    let base = total / parts;
    let mut out = Vec::with_capacity(parts as usize);
    let mut s = 0;
    for i in 0..parts {
        let e = if i == parts - 1 { total } else { s + base };
        out.push((s, e));
        s = e;
    }
    out
}

/// Where an idle connection may cut a busy segment `[pos, end)` in half
/// (dynamic re-segmentation). `None` if too little is left to be worth it.
pub fn split_point(pos: u64, end: u64) -> Option<u64> {
    if end == UNKNOWN_END || end <= pos {
        return None;
    }
    let rem = end - pos;
    if rem >= 2 * MIN_SPLIT {
        Some(pos + rem / 2)
    } else {
        None
    }
}

// ───────────────────────── speed limiting ─────────────────────────

/// Virtual-time pacer: tells a caller how long to wait before moving `bytes`
/// so that the long-run rate never exceeds `rate_bps`. Time is injected, so it is testable.
#[derive(Default, Debug, Clone)]
pub struct Pacer {
    next_free_ns: u64,
}

impl Pacer {
    pub fn new() -> Self {
        Self { next_free_ns: 0 }
    }
    /// Reserve `bytes`; returns nanoseconds the caller should sleep first.
    pub fn reserve(&mut self, now_ns: u64, bytes: u64, rate_bps: u64) -> u64 {
        if rate_bps == 0 {
            return 0;
        }
        let start = self.next_free_ns.max(now_ns);
        let cost = (bytes as u128 * 1_000_000_000u128 / rate_bps as u128) as u64;
        self.next_free_ns = start.saturating_add(cost);
        start - now_ns
    }
}

// ───────────────────────── names, URLs ─────────────────────────

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16);
            let l = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn sanitize_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let mut name = cleaned.trim().trim_matches('.').trim().to_string();
    if name.is_empty() {
        return "download".to_string();
    }
    // keep within typical filesystem limit (255 bytes), preserving the extension
    if name.len() > 200 {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_string())
            .unwrap_or_default();
        let keep = 200usize.saturating_sub(ext.len() + 1);
        let mut stem: String = name.chars().take(keep).collect();
        while stem.len() > keep {
            stem.pop();
        }
        name = if ext.is_empty() || ext.len() > 10 {
            stem
        } else {
            format!("{stem}.{ext}")
        };
    }
    name
}

pub fn filename_from_disposition(h: &str) -> Option<String> {
    let lower = h.to_ascii_lowercase();
    if let Some(i) = lower.find("filename*=") {
        let v = h[i + 10..].split(';').next()?.trim();
        let v = v.rsplit('\'').next().unwrap_or(v);
        let d = percent_decode(v.trim_matches('"'));
        if !d.trim().is_empty() {
            return Some(sanitize_filename(&d));
        }
    }
    let mut from = 0;
    while let Some(j) = lower[from..].find("filename=") {
        let i = from + j;
        let boundary = i == 0 || matches!(lower.as_bytes()[i - 1], b';' | b' ' | b'\t');
        let rest = &h[i + 9..];
        if boundary {
            let v = if let Some(q) = rest.strip_prefix('"') {
                q.split('"').next().unwrap_or("")
            } else {
                rest.split(';').next().unwrap_or("").trim()
            };
            if !v.trim().is_empty() {
                return Some(sanitize_filename(&percent_decode(v)));
            }
        }
        from = i + 9;
    }
    None
}

pub fn filename_from_url(url: &str) -> String {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_q = no_frag.split('?').next().unwrap_or(no_frag);
    let rest = no_q.split_once("://").map(|(_, r)| r).unwrap_or(no_q);
    let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("");
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    let decoded = percent_decode(last);
    if decoded.is_empty() {
        "download".to_string()
    } else {
        sanitize_filename(&decoded)
    }
}

pub fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Folder name used when "sort by type" is on.
pub fn category_for(name: &str) -> &'static str {
    match extension(name).as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" | "3gp" | "ts" => "Video",
        "mp3" | "flac" | "wav" | "aac" | "ogg" | "opus" | "m4a" | "wma" => "Music",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "heic" | "tiff" => "Images",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "epub" | "odt"
        | "csv" | "md" => "Documents",
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" | "tgz" => "Archives",
        "apk" | "apks" | "xapk" | "exe" | "msi" | "deb" | "dmg" | "iso" | "appimage" => "Programs",
        _ => "Other",
    }
}

/// `http://x/img[001-003].jpg` → three URLs. Only the first numeric `[a-b]` group expands.
pub fn expand_sequence(url: &str) -> Vec<String> {
    let (Some(l), Some(r)) = (url.find('['), url.find(']')) else {
        return vec![url.to_string()];
    };
    if r < l {
        return vec![url.to_string()];
    }
    let inner = &url[l + 1..r];
    let Some((a, b)) = inner.split_once('-') else {
        return vec![url.to_string()];
    };
    let (Ok(lo), Ok(hi)) = (a.parse::<u64>(), b.parse::<u64>()) else {
        return vec![url.to_string()];
    };
    if lo > hi || hi - lo >= 10_000 {
        return vec![url.to_string()];
    }
    let width = if a.starts_with('0') { a.len() } else { 0 };
    (lo..=hi)
        .map(|n| format!("{}{:0w$}{}", &url[..l], n, &url[r + 1..], w = width))
        .collect()
}

/// Raw `href=` / `src=` values from HTML (not yet resolved against a base URL).
pub fn extract_links(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let b = html.as_bytes();
    let mut out: Vec<String> = Vec::new();
    for attr in ["href=", "src="] {
        let mut from = 0;
        while let Some(j) = lower[from..].find(attr) {
            let at = from + j;
            from = at + attr.len();
            if at > 0 && !(b[at - 1] as char).is_ascii_whitespace() {
                continue;
            }
            let (start, quote) = match b.get(from) {
                Some(b'"') => (from + 1, Some('"')),
                Some(b'\'') => (from + 1, Some('\'')),
                _ => (from, None),
            };
            let rest = &html[start..];
            let end = match quote {
                Some(q) => rest.find(q),
                None => rest.find(|c: char| c.is_whitespace() || c == '>'),
            }
            .unwrap_or(rest.len());
            let v = rest[..end].trim().replace("&amp;", "&");
            let low = v.to_ascii_lowercase();
            if v.is_empty()
                || v.starts_with('#')
                || low.starts_with("javascript:")
                || low.starts_with("mailto:")
                || low.starts_with("data:")
            {
                continue;
            }
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out
}

// ───────────────────────── HTTP helpers ─────────────────────────

/// `bytes 0-0/12345` → (start, end, Some(total)); total is None for `*`.
pub fn parse_content_range(h: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = h.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let total = total.trim().parse::<u64>().ok();
    if range.trim() == "*" {
        return Some((0, 0, total));
    }
    let (s, e) = range.split_once('-')?;
    Some((s.trim().parse().ok()?, e.trim().parse().ok()?, total))
}

pub fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Md5,
    Sha1,
    Sha256,
}

/// Accepts `abcd…`, `sha256:abcd…`, `MD5=abcd…`; algorithm is inferred from length.
pub fn parse_checksum(s: &str) -> Option<(Algo, String)> {
    let t = s.trim();
    let t = t.rsplit(|c| c == ':' || c == '=').next().unwrap_or(t);
    let h: String = t
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        32 => Some((Algo::Md5, h)),
        40 => Some((Algo::Sha1, h)),
        64 => Some((Algo::Sha256, h)),
        _ => None,
    }
}

// ───────────────────────── formatting ─────────────────────────

pub fn format_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", U[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", U[i])
    } else {
        format!("{v:.2} {}", U[i])
    }
}

pub fn format_speed(bps: f64) -> String {
    format!("{}/s", format_bytes(bps.max(0.0) as u64))
}

pub fn format_eta(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_cover_everything_exactly_once() {
        for &(total, conns) in &[
            (1u64, 8u32),
            (600_000, 8),
            (100 << 20, 16),
            (7, 1),
            (3 << 20, 32),
        ] {
            let segs = plan_segments(total, conns);
            assert_eq!(segs.first().unwrap().0, 0);
            assert_eq!(segs.last().unwrap().1, total);
            for w in segs.windows(2) {
                assert_eq!(w[0].1, w[1].0);
            }
            assert!(segs.len() as u32 <= conns.max(1));
        }
        assert!(plan_segments(0, 8).is_empty());
        // tiny files are never over-split
        assert_eq!(plan_segments(MIN_SPLIT - 1, 16).len(), 1);
    }

    #[test]
    fn split_only_when_worthwhile() {
        assert_eq!(split_point(0, 4 * MIN_SPLIT), Some(2 * MIN_SPLIT));
        assert_eq!(split_point(10, 10 + 2 * MIN_SPLIT - 1), None);
        assert_eq!(split_point(0, UNKNOWN_END), None);
        assert_eq!(split_point(5, 5), None);
    }

    #[test]
    fn pacer_limits_rate() {
        let mut p = Pacer::new();
        // 1000 B/s: first 500 B free, second must wait 0.5 s, third 1.0 s
        assert_eq!(p.reserve(0, 500, 1000), 0);
        assert_eq!(p.reserve(0, 500, 1000), 500_000_000);
        assert_eq!(p.reserve(0, 500, 1000), 1_000_000_000);
        // after the backlog elapses there is no wait
        assert_eq!(p.reserve(5_000_000_000, 500, 1000), 0);
        // unlimited
        assert_eq!(Pacer::new().reserve(0, 1 << 30, 0), 0);
    }

    #[test]
    fn filenames() {
        assert_eq!(
            filename_from_url("https://a.b/dir/My%20File.zip?x=1#f"),
            "My File.zip"
        );
        assert_eq!(filename_from_url("https://a.b/"), "download");
        assert_eq!(filename_from_url("https://a.b"), "download");
        assert_eq!(
            filename_from_disposition("attachment; filename=\"report 1.pdf\"").unwrap(),
            "report 1.pdf"
        );
        assert_eq!(
            filename_from_disposition("attachment; filename*=UTF-8''na%C3%AFve.txt").unwrap(),
            "naïve.txt"
        );
        assert_eq!(
            filename_from_disposition(
                "attachment; filename=\"fallback.txt\"; filename*=UTF-8''real.txt"
            )
            .unwrap(),
            "real.txt"
        );
        assert_eq!(filename_from_disposition("inline"), None);
        assert_eq!(sanitize_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_filename("a:b*c.txt"), "a_b_c.txt");
        assert_eq!(sanitize_filename("   "), "download");
        let long = format!("{}.mp4", "x".repeat(400));
        let s = sanitize_filename(&long);
        assert!(s.len() <= 200 && s.ends_with(".mp4"));
    }

    #[test]
    fn percent_decode_edges() {
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%4"), "%4");
    }

    #[test]
    fn categories() {
        assert_eq!(category_for("a.MKV"), "Video");
        assert_eq!(category_for("x.tar.gz"), "Archives");
        assert_eq!(category_for("noext"), "Other");
        assert_eq!(category_for("app.apk"), "Programs");
    }

    #[test]
    fn sequences() {
        assert_eq!(
            expand_sequence("http://x/img[08-11].jpg"),
            vec![
                "http://x/img08.jpg",
                "http://x/img09.jpg",
                "http://x/img10.jpg",
                "http://x/img11.jpg"
            ]
        );
        assert_eq!(
            expand_sequence("http://x/p[1-3]"),
            vec!["http://x/p1", "http://x/p2", "http://x/p3"]
        );
        assert_eq!(expand_sequence("http://x/[a-b]"), vec!["http://x/[a-b]"]);
        assert_eq!(expand_sequence("http://x/[5-1]"), vec!["http://x/[5-1]"]);
        assert_eq!(expand_sequence("http://x/plain"), vec!["http://x/plain"]);
        assert_eq!(expand_sequence("http://x/[0-99999]").len(), 1);
    }

    #[test]
    fn link_extraction() {
        let html = r##"<a href="/a.zip">A</a> <a HREF='b.pdf?x=1&amp;y=2'>B</a>
            <img src=c.png alt=x> <a href="#top">t</a> <a href="javascript:void(0)">j</a>
            <a href="/a.zip">dup</a> data-href="nope""##;
        let l = extract_links(html);
        assert_eq!(l, vec!["/a.zip", "b.pdf?x=1&y=2", "c.png"]);
    }

    #[test]
    fn content_range() {
        assert_eq!(
            parse_content_range("bytes 0-0/12345"),
            Some((0, 0, Some(12345)))
        );
        assert_eq!(
            parse_content_range("bytes 100-199/*"),
            Some((100, 199, None))
        );
        assert_eq!(parse_content_range("bytes */500"), Some((0, 0, Some(500))));
        assert_eq!(parse_content_range("garbage"), None);
    }

    #[test]
    fn encoding() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b""), "");
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }

    #[test]
    fn checksums() {
        let md5 = "d41d8cd98f00b204e9800998ecf8427e";
        assert_eq!(parse_checksum(md5), Some((Algo::Md5, md5.to_string())));
        assert_eq!(
            parse_checksum(&format!("MD5={}", md5.to_uppercase())),
            Some((Algo::Md5, md5.to_string()))
        );
        let s256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(
            parse_checksum(&format!("sha256:{s256}")),
            Some((Algo::Sha256, s256.to_string()))
        );
        assert_eq!(parse_checksum("xyz"), None);
        assert_eq!(parse_checksum("abcd"), None);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.00 GB");
        assert_eq!(format_eta(59), "59s");
        assert_eq!(format_eta(125), "2m 05s");
        assert_eq!(format_eta(3725), "1h 02m");
        assert_eq!(format_speed(2048.0), "2.00 KB/s");
    }
}
