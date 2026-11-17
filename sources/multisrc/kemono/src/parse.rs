//! Helpers shared by the Kemono multisrc: service names, dates, path parsing
//! and image URLs.
//!
//! The API quirks documented here were verified against the live sites:
//! - every `/api/v1/...` request must send `Accept: text/css`, otherwise the
//!   ddos-guard front end answers HTTP 403;
//! - full-size files live on `nX.<site>` servers that are unreachable from the
//!   test network, while `https://img.<site>/thumbnail/data/<path>` (reached
//!   through the site's `/thumbnail/data/<path>` redirect) works and returns a
//!   max-800px image, so pages are served from the thumbnail endpoint.

use anyhow::{Result, anyhow};

use crate::Site;

/// Keiyoushi's `serviceName()` mapping for the creator "type".
pub fn service_name(service: &str) -> String {
    match service {
        "fanbox" => "Pixiv Fanbox".to_string(),
        "subscribestar" => "SubscribeStar".to_string(),
        "dlsite" => "DLsite".to_string(),
        "onlyfans" => "OnlyFans".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Parse one of the API's ISO-8601 date-time strings into Unix seconds.
pub fn parse_iso_seconds(raw: &str) -> Option<i64> {
    use chrono::{NaiveDateTime, TimeZone, Utc};

    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(date.timestamp());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(date) = NaiveDateTime::parse_from_str(raw, format) {
            return Some(Utc.from_utc_datetime(&date).timestamp());
        }
    }
    None
}

/// Split a stored manga path (`/service/user/<id>`) into its two last parts.
pub fn creator_parts(path: &str) -> Result<(String, String)> {
    let segments: Vec<&str> = path
        .trim()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    match segments.as_slice() {
        // `<service>/user/<id>` (stored path) or a full `/<service>/user/<id>/` URL.
        [service, "user", id] => Ok(((*service).to_string(), (*id).to_string())),
        [.., service, "user", id] => Ok(((*service).to_string(), (*id).to_string())),
        _ => Err(anyhow!(
            "expected a /service/user/<id> path, got {path:?}"
        )),
    }
}

/// Site-relative creator path used as `MangaInfo.path` and for later requests.
pub fn creator_path(service: &str, user: &str) -> String {
    format!("/{service}/user/{user}")
}

/// Site-relative post path used as `ChapterInfo.path`.
pub fn post_path(service: &str, user: &str, post_id: &str) -> String {
    format!("/{service}/user/{user}/post/{post_id}")
}

/// `/api/v1` root for the selected site.
pub fn api_url(site: &Site) -> String {
    format!("{}/api/v1", site.base())
}

/// Absolute API URL for a site-relative path.
pub fn api_path(site: &Site, path: &str) -> String {
    format!("{}{}", api_url(site), ensure_leading_slash(path))
}

/// `https://img.<host>` image CDN of the selected site.
pub fn image_cdn(site: &Site) -> String {
    site.base().replacen("//", "//img.", 1)
}

/// Creator avatar on the image CDN.
pub fn creator_icon(site: &Site, service: &str, user: &str) -> String {
    format!("{}/icons/{service}/{user}", image_cdn(site))
}

/// Working image URL for an attachment path: the site's thumbnail endpoint.
pub fn thumbnail_url(site: &Site, path: &str) -> String {
    format!("{}/thumbnail/data{path}", site.base())
}

fn ensure_leading_slash(path: &str) -> String {
    let path = path.trim();
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Ordered image candidates for `get_image_bytes`.
///
/// The Kemono file servers are regularly unreachable and a request to them
/// stalls until the network timeout, so the working thumbnail URL is tried
/// first whenever the input is a `/data/` URL.
pub fn image_candidates(site: &Site, url: &str) -> Vec<String> {
    if let Some(index) = url.find("/data/") {
        if !url[..index].contains("/thumbnail") {
            let path = &url[index + "/data".len()..];
            return vec![thumbnail_url(site, path), url.to_string()];
        }
    }
    vec![url.to_string()]
}
