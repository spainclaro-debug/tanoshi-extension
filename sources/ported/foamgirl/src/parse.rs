//! HTML scraping for FoamGirl.
//!
//! Selectors verified against the live markup: `.update_area .i_list` cards on
//! listing pages, the `.imageclick-imgbox` anchors that carry full image URLs
//! and the `.page-numbers[title="Next page"]` pager on a photo set.

use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::{ID, URL};

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
    ["data-original", "data-src", "src"]
        .iter()
        .filter_map(|name| attr(img, name))
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty() && !value.starts_with("data:"))
}

fn absolute(url: &str) -> String {
    let url = url.trim();
    if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{URL}/{}", url.trim_start_matches('/'))
    }
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
        format!("{URL}/{}", site_relative(path))
    }
}

/// Parse listing cards (`.update_area .i_list`).
pub fn listing(document: &Html) -> Vec<MangaInfo> {
    let mut out: Vec<MangaInfo> = Vec::new();

    for element in document.select(&selector(".update_area .i_list")) {
        let Some(link) = element.select(&selector("a.meta-title")).next() else {
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

/// Strip the site's trailing `(90P)` photo counter from a title.
fn strip_photo_count(title: &str) -> String {
    let title = title.trim();
    let Some(position) = title.rfind('(') else {
        return title.to_string();
    };
    let suffix = &title[position + 1..];
    let is_count = suffix.ends_with("P)")
        && suffix[..suffix.len().saturating_sub(2)]
            .chars()
            .all(|character| character.is_ascii_digit() || character == '+')
        && suffix.len() > 2;
    if is_count {
        title[..position].trim().to_string()
    } else {
        title.to_string()
    }
}

/// Parse a photo-set detail page.
pub fn manga_detail(document: &Html, path: &str) -> MangaInfo {
    let normalized = site_relative(path);

    let title = document
        .select(&selector(".item_title h1"))
        .next()
        .or_else(|| document.select(&selector("h1")).next())
        .map(|element| strip_photo_count(&text_of(&element)))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            normalized
                .rsplit('/')
                .next()
                .unwrap_or(normalized.as_str())
                .trim_end_matches(".html")
                .to_string()
        });

    let cover_url = document
        .select(&selector(".image_div img"))
        .next()
        .or_else(|| document.select(&selector(".imageclick-imgbox img")).next())
        .and_then(|img| image_src(&img))
        .map(|src| absolute(&src))
        .unwrap_or_default();

    let mut genre: Vec<String> = Vec::new();
    for category in document.select(&selector(".image-info-cat a")) {
        let name = text_of(&category);
        if !name.is_empty() && !genre.contains(&name) {
            genre.push(name);
        }
    }

    MangaInfo {
        source_id: ID,
        title,
        author: Vec::new(),
        genre,
        status: Some("Completed".to_string()),
        description: None,
        path: normalized,
        cover_url,
    }
}

/// One `GALLERY` chapter per photo set, dated by `.image-info-time`.
pub fn chapters(document: &Html, path: &str) -> Vec<ChapterInfo> {
    vec![ChapterInfo {
        source_id: ID,
        title: "GALLERY".to_string(),
        path: site_relative(path),
        number: 0.0,
        scanlator: None,
        uploaded: uploaded_millis(document),
    }]
}

fn uploaded_millis(document: &Html) -> i64 {
    let Some(element) = document.select(&selector(".image-info-time")).next() else {
        return 0;
    };
    let text = text_of(&element);
    // The element starts with a private-use icon glyph before the date.
    let Some(start) = text.find(|character: char| character.is_ascii_digit()) else {
        return 0;
    };
    let date = text[start..].trim();
    NaiveDate::parse_from_str(date, "%Y.%m.%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| date.and_utc().timestamp_millis())
        .unwrap_or(0)
}

/// Full image URLs of one gallery page.
pub fn gallery_images(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for link in document.select(&selector(".imageclick-imgbox")) {
        if let Some(href) = attr(&link, "href") {
            let url = absolute(&href);
            if url.starts_with("http") && !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

/// Next gallery page URL from `.page-numbers[title="Next page"]`.
pub fn next_page(document: &Html) -> Option<String> {
    let link = document
        .select(&selector(r#".page-numbers[title="Next page"]"#))
        .next()?;
    let href = attr(&link, "href")?;
    let url = absolute(&href);
    is_paged_gallery(&url).then_some(url)
}

/// Only follow `.../<id>_<page>.html` style URLs, as the original does.
fn is_paged_gallery(url: &str) -> bool {
    let Some(segment) = url.split('/').next_back() else {
        return false;
    };
    let bytes = segment.as_bytes();
    bytes
        .windows(3)
        .any(|window| window[0].is_ascii_digit() && window[1] == b'_' && window[2].is_ascii_digit())
}
