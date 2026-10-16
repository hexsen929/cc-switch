use crate::error::AppError;
use futures::StreamExt;
use regex::Regex;
use reqwest::{Client, Response};
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;
use std::time::Duration;
use url::Url;

pub(super) fn parse_url(raw: &str) -> Result<Url, AppError> {
    let url = Url::parse(raw).map_err(|_| AppError::InvalidInput("Invalid HTTP(S) URL".into()))?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::InvalidInput(
            "URL must use HTTP(S), without credentials or fragments".into(),
        ));
    }
    Ok(url)
}

pub(super) fn client_for(url: &str) -> Result<Client, AppError> {
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent("CC-Switch-Web-Search/1.0");
    if crate::proxy::http_client::is_loopback_url(url) {
        builder = builder.no_proxy();
    } else if let Some(proxy) = crate::proxy::http_client::get_current_proxy_url() {
        builder = builder.proxy(
            reqwest::Proxy::all(&proxy)
                .map_err(|_| AppError::Message("Invalid search proxy configuration".into()))?,
        );
    }
    builder
        .build()
        .map_err(|_| AppError::Message("Cannot initialize search HTTP client".into()))
}

pub(super) async fn limited_bytes(response: Response, maximum: usize) -> Result<Vec<u8>, AppError> {
    if response
        .content_length()
        .is_some_and(|size| size > maximum as u64)
    {
        return Err(AppError::Message(
            "Web response exceeds the size limit".into(),
        ));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::Message("Cannot read web response".into()))?;
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(AppError::Message(
                "Web response exceeds the size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Deny non-public destinations, including IPv4-mapped IPv6. DNS is validated
/// and pinned separately on every redirect so a second resolution cannot rebind.
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let segments = ip.segments();
            // Only globally routable unicast 2000::/3, excluding documentation,
            // Teredo and 6to4 (which may tunnel to private IPv4 destinations).
            (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && (segments[1] <= 0x01ff || segments[1] == 0x0db8))
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && segments[1] <= 0x0fff)
        }
    }
}

async fn public_client(url: &Url) -> Result<Client, AppError> {
    let host = url
        .host_str()
        .ok_or_else(|| AppError::InvalidInput("URL has no host".into()))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = url
        .port_or_known_default()
        .ok_or_else(|| AppError::InvalidInput("URL has no port".into()))?;
    let addresses: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| AppError::Message("DNS lookup timed out".into()))?
        .map_err(|_| AppError::Message("Cannot resolve page host".into()))?
        .collect()
    };
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(AppError::InvalidInput(
            "web_fetch only permits public internet addresses".into(),
        ));
    }
    // no_proxy prevents environment proxies from resolving an unchecked host.
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent("CC-Switch-Web-Fetch/1.0")
        .build()
        .map_err(|_| AppError::Message("Cannot initialize page HTTP client".into()))
}

pub(super) fn truncate(text: &str, maximum: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(maximum).collect();
    if chars.next().is_some() {
        result.push_str("\n[truncated]");
    }
    result
}

fn html_text(html: &str) -> String {
    static SCRIPTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<script\b[^>]*>.*?</script\s*>").expect("static script regex")
    });
    static STYLES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<style\b[^>]*>.*?</style\s*>").expect("static style regex")
    });
    static TAGS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<[^>]+>").expect("static tag regex"));
    let without_scripts = SCRIPTS.replace_all(html, " ");
    let without_styles = STYLES.replace_all(&without_scripts, " ");
    TAGS.replace_all(&without_styles, " ")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) async fn fetch(raw: &str) -> Result<String, AppError> {
    let mut url = parse_url(raw)?;
    for hop in 0..=5 {
        let response = public_client(&url)
            .await?
            .get(url.clone())
            .header("accept", "text/html,text/plain,application/json")
            .send()
            .await
            .map_err(|_| AppError::Message("Page request failed".into()))?;
        if response.status().is_redirection() {
            if hop == 5 {
                return Err(AppError::Message("Too many page redirects".into()));
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    AppError::Message("Page redirect has no valid destination".into())
                })?;
            let next = url
                .join(location)
                .map_err(|_| AppError::InvalidInput("Invalid page redirect".into()))?;
            url = parse_url(next.as_str())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(AppError::Message(format!(
                "Page returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !["text/", "application/json", "application/xhtml+xml"]
            .iter()
            .any(|kind| content_type.starts_with(kind))
        {
            return Err(AppError::InvalidInput(
                "web_fetch supports textual pages only".into(),
            ));
        }
        let bytes = limited_bytes(response, 2 * 1024 * 1024).await?;
        let text = String::from_utf8_lossy(&bytes);
        let body = if content_type.contains("html") {
            html_text(&text)
        } else {
            text.into_owned()
        };
        // The caller masks credentials before applying its character budget.
        return Ok(format!("URL: {url}\n\n{body}"));
    }
    Err(AppError::Message("Page fetch failed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_rejects_private_reserved_and_tunnel_destinations() {
        for raw in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.100.100.200",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(!public_ip(raw.parse().unwrap()), "{raw}");
        }
        for raw in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(raw.parse().unwrap()));
        }
        for raw in [
            "file:///etc/passwd",
            concat!("http://user:", "pass", "@example.invalid"),
            "https://example.com/#secret",
        ] {
            assert!(parse_url(raw).is_err());
        }
    }

    #[test]
    fn page_text_removes_executable_markup_and_truncates_unicode_safely() {
        assert_eq!(
            html_text("<script>secret()</script><style>x</style><h1>A &amp; B</h1>"),
            "A & B"
        );
        assert_eq!(truncate("中文abc", 2), "中文\n[truncated]");
    }
}
