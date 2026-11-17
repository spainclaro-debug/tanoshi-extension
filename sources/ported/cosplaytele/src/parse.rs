//! HTML scraping for CosplayTele.
//!
//! Selectors verified against the live markup: `main div.box` cards on archive
//! pages, `.entry-title` / `.gallery-item img` on a post page and the
//! `time.updated` element that carries the post date.

use chrono::DateTime;
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

/// Parse the archive cards on `/page/N/`, search and category pages.
pub fn listing(document: &Html) -> Vec<MangaInfo> {
    let mut out: Vec<MangaInfo> = Vec::new();

    for element in document.select(&selector("main div.box")) {
        let Some(link) = element.select(&selector("h5 a")).next() else {
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

/// Post tags and categories (`#main a` links into `/tag/` or `/category/`).
fn tags(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for link in document.select(&selector("#main a")) {
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        if !href.contains("/tag/") && !href.contains("/category/") {
            continue;
        }
        let name = text_of(&link);
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
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
        .or_else(|| {
            document
                .select(&selector(r#"meta[property="og:image"]"#))
                .next()
                .and_then(|meta| attr(&meta, "content"))
                .map(|content| absolute(&content))
        })
        .unwrap_or_default();

    MangaInfo {
        source_id: ID,
        title: title.clone(),
        author: Vec::new(),
        genre: tags(document),
        status: Some("Completed".to_string()),
        description: Some(title),
        path: normalized,
        cover_url,
    }
}

/// One `Gallery` chapter per photo set.
pub fn chapters(document: &Html, path: &str) -> Vec<ChapterInfo> {
    vec![ChapterInfo {
        source_id: ID,
        title: "Gallery".to_string(),
        path: site_relative(path),
        number: 0.0,
        scanlator: None,
        uploaded: uploaded_millis(document),
    }]
}

fn uploaded_millis(document: &Html) -> i64 {
    let Some(time) = document.select(&selector("time.updated")).next() else {
        return 0;
    };
    attr(&time, "datetime")
        .and_then(|value| DateTime::parse_from_rfc3339(value.trim()).ok())
        .map(|date| date.timestamp_millis())
        .unwrap_or(0)
}

/// Gallery image URLs of a post page.
pub fn gallery_images(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for img in document.select(&selector(".gallery-item img")) {
        if let Some(src) = image_src(&img) {
            let src = absolute(&src);
            if !out.contains(&src) {
                out.push(src);
            }
        }
    }
    if out.is_empty() {
        // Some posts embed the photos directly in the content instead.
        for img in document.select(&selector(".entry-content img")) {
            if let Some(src) = image_src(&img) {
                let src = absolute(&src);
                if !out.contains(&src) {
                    out.push(src);
                }
            }
        }
    }
    out
}
