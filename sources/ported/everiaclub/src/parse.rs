//! HTML scraping for Everia.club.
//!
//! Selectors verified against the live markup: the `.wli_popular_posts-class`
//! sidebar widget on the homepage and the `.entry-title` / `.post-tags` /
//! `.entry-content` structure of a post page.

use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::ID;
use crate::URL;

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}

fn text_of(element: &ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn attr(element: &ElementRef<'_>, name: &str) -> Option<String> {
    element.value().attr(name).map(str::to_string)
}

/// Prefer a lazyloaded source over the placeholder `src`.
fn image_src(img: &ElementRef<'_>) -> Option<String> {
    ["data-lazy-src", "data-src", "src"]
        .iter()
        .filter_map(|name| attr(img, name))
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty() && !value.starts_with("data:"))
}

pub fn absolute(url: &str) -> String {
    let url = url.trim();
    if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{URL}/{}", url.trim_start_matches('/'))
    }
}

/// Decode the HTML entities WordPress leaves in REST `rendered` fields.
pub fn unescape_html(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    fragment
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip scheme, host, query and surrounding slashes from a stored path or URL.
pub fn site_relative(value: &str) -> String {
    let value = value.trim();
    let value = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    let value = match value.split_once('/') {
        Some((first, rest)) if first.contains('.') => rest,
        _ => value,
    };
    value
        .split(['?', '#'])
        .next()
        .unwrap_or(value)
        .trim_matches('/')
        .to_string()
}

pub fn manga_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{URL}/{}/", site_relative(path))
    }
}

/// Top posts from the homepage's popular-posts widget.
pub fn popular(document: &Html) -> Vec<MangaInfo> {
    let mut out: Vec<MangaInfo> = Vec::new();

    for element in document.select(&selector(".wli_popular_posts-class li")) {
        let Some(link) = element.select(&selector("h3 a")).next() else {
            continue;
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let path = site_relative(&href);
        if path.is_empty() {
            continue;
        }
        let title = text_of(&link);
        if title.is_empty() {
            continue;
        }
        if out.iter().any(|manga| manga.path == path) {
            continue;
        }

        let cover_url = element
            .select(&selector("img"))
            .next()
            .and_then(|img| image_src(&img))
            .map(|src| absolute(&src))
            .unwrap_or_default();

        out.push(MangaInfo {
            source_id: ID,
            title,
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path,
            cover_url,
        });
    }

    out
}

/// Parse a photo-set post page.
pub fn manga_detail(document: &Html, path: &str) -> MangaInfo {
    let normalized = site_relative(path);

    let title = document
        .select(&selector(".entry-title"))
        .next()
        .map(|element| text_of(&element))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            normalized
                .rsplit('/')
                .next()
                .unwrap_or(normalized.as_str())
                .replace('-', " ")
        });

    let cover_url = document
        .select(&selector(".entry-content img"))
        .next()
        .and_then(|img| image_src(&img))
        .map(|src| absolute(&src))
        .unwrap_or_default();

    let mut genre: Vec<String> = Vec::new();
    for tag in document.select(&selector(".post-tags > a")) {
        let name = text_of(&tag);
        if !name.is_empty() && !genre.contains(&name) {
            genre.push(name);
        }
    }

    MangaInfo {
        source_id: ID,
        title: title.clone(),
        author: Vec::new(),
        genre,
        status: Some("Completed".to_string()),
        description: Some(title),
        path: normalized,
        cover_url,
    }
}

/// One `Gallery` chapter per photo set, dated by the post URL.
pub fn chapters(document: &Html, path: &str) -> Vec<ChapterInfo> {
    let canonical = document
        .select(&selector(r#"link[rel="canonical"]"#))
        .next()
        .and_then(|link| attr(&link, "href"))
        .unwrap_or_else(|| manga_url(path));

    vec![ChapterInfo {
        source_id: ID,
        title: "Gallery".to_string(),
        path: site_relative(path),
        number: 0.0,
        scanlator: None,
        uploaded: date_millis_from_url(&canonical),
    }]
}

/// Extract `YYYY/MM/DD` from a WordPress post URL as Unix millis (UTC).
fn date_millis_from_url(url: &str) -> i64 {
    let parts: Vec<&str> = url.split('/').filter(|part| !part.is_empty()).collect();
    for window in parts.windows(3) {
        let (Some(year), Some(month), Some(day)) = (
            (window[0].len() == 4)
                .then(|| window[0].parse::<i32>().ok())
                .flatten(),
            (window[1].len() == 2)
                .then(|| window[1].parse::<u32>().ok())
                .flatten(),
            (window[2].len() == 2)
                .then(|| window[2].parse::<u32>().ok())
                .flatten(),
        ) else {
            continue;
        };
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc().timestamp_millis())
                .unwrap_or(0);
        }
    }
    0
}

/// Image URLs in the post content.
pub fn gallery_images(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for img in document.select(&selector(".entry-content img")) {
        if let Some(src) = image_src(&img) {
            let src = absolute(&src);
            if !src.starts_with("data:") && !out.contains(&src) {
                out.push(src);
            }
        }
    }
    out
}

/// Sub-page URLs of a split photo set (`.page-links a.post-page-numbers`).
pub fn page_links(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for link in document.select(&selector(".page-links a.post-page-numbers")) {
        if let Some(href) = attr(&link, "href") {
            let url = absolute(&href);
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}
