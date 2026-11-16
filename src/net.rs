//! HTTP plumbing: probing a URL, building agents, fetching pages for the link grabber.

use crate::logic;
use std::time::Duration;

/// Errors are either worth retrying (network blips, 5xx, 429) or not (404, disk full…).
#[derive(Debug, Clone)]
pub enum NetErr {
    Retry(String),
    Fatal(String),
}

impl NetErr {
    pub fn msg(&self) -> &str {
        match self {
            NetErr::Retry(m) | NetErr::Fatal(m) => m,
        }
    }
    pub fn retryable(&self) -> bool {
        matches!(self, NetErr::Retry(_))
    }
}

pub fn status_error(code: u16) -> NetErr {
    let text = match code {
        401 => "401 Unauthorized (add credentials under Advanced)".to_string(),
        403 => "403 Forbidden (try a Referer or Cookie under Advanced)".to_string(),
        404 => "404 Not found".to_string(),
        410 => "410 Gone".to_string(),
        416 => "416 Range not satisfiable".to_string(),
        c => format!("HTTP {c}"),
    };
    if code == 408 || code == 429 || code >= 500 {
        NetErr::Retry(text)
    } else {
        NetErr::Fatal(text)
    }
}

pub fn agent(user_agent: &str) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(user_agent)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .redirects(8)
        .build()
}

#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub total: Option<u64>,
    pub ranges: bool,
    pub name: Option<String>,
    pub etag: String,
    pub last_modified: String,
}

fn apply(mut req: ureq::Request, headers: &[(String, String)]) -> ureq::Request {
    req = req.set("Accept-Encoding", "identity");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    req
}

/// Ask for byte 0 only: a 206 + Content-Range proves range support and reveals the size.
pub fn probe(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
) -> Result<Probe, NetErr> {
    let req = apply(agent.get(url), headers).set("Range", "bytes=0-0");
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(416, r)) => {
            // empty file, or range past the end
            let total = r
                .header("content-range")
                .and_then(logic::parse_content_range)
                .and_then(|t| t.2)
                .unwrap_or(0);
            return Ok(Probe {
                total: Some(total),
                ranges: false,
                name: Some(logic::filename_from_url(url)),
                ..Default::default()
            });
        }
        Err(ureq::Error::Status(c, _)) => return Err(status_error(c)),
        Err(e) => return Err(NetErr::Retry(e.to_string())),
    };

    let name = resp
        .header("content-disposition")
        .and_then(logic::filename_from_disposition)
        .unwrap_or_else(|| logic::filename_from_url(resp.get_url()));
    let etag = resp.header("etag").unwrap_or("").to_string();
    let last_modified = resp.header("last-modified").unwrap_or("").to_string();

    let (total, ranges) = if resp.status() == 206 {
        let t = resp
            .header("content-range")
            .and_then(logic::parse_content_range)
            .and_then(|t| t.2);
        (t, t.is_some())
    } else {
        let t = resp
            .header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        (t, false)
    };
    // Dropping `resp` closes the connection without reading the body.
    drop(resp);
    Ok(Probe {
        total,
        ranges,
        name: Some(name),
        etag,
        last_modified,
    })
}

/// Fetch an HTML page (capped at 3 MB) for the link grabber. Returns (final_url, body).
pub fn fetch_page(agent: &ureq::Agent, url: &str) -> Result<(String, String), String> {
    use std::io::Read;
    let resp = agent
        .get(url)
        .set("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => status_error(c).msg().to_string(),
            e => e.to_string(),
        })?;
    let final_url = resp.get_url().to_string();
    let mut body = Vec::new();
    resp.into_reader()
        .take(3 * 1024 * 1024)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    Ok((final_url, String::from_utf8_lossy(&body).into_owned()))
}

/// Resolve a possibly-relative link against its page; only http(s) results are kept.
pub fn resolve(base: &str, href: &str) -> Option<String> {
    let b = url::Url::parse(base).ok()?;
    let u = b.join(href).ok()?;
    match u.scheme() {
        "http" | "https" => Some(u.to_string()),
        _ => None,
    }
}
