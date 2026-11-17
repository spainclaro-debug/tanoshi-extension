//! HTML scraping for Buon Dua.
//!
//! Selectors verified against the live markup: the `.blog` card listing on
//! `/`, `/hot` and search pages, the `.article` section of a detail page with
//! its `.article-fulltext` image paragraphs and the `.pagination` nav that
//! exposes the last page number.

use chrono::NaiveDateTime;
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
    ["data-src", "data-lazy-src", "src"]
        .iter()
        .filter_map(|name| attr(img, name))
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty() && !value.starts_with("data:"))
}

fn absolute(url: &str) -> String {
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

/// Parse the manga cards of a listing page.
pub fn listing(document: &Html) -> Vec<MangaInfo> {
    let mut out: Vec<MangaInfo> = Vec::new();

    for element in document.select(&selector(".blog > div")) {
        let Some(link) = element.select(&selector(".item-content .item-link")).next() else {
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

/// Image URLs in the `.article-fulltext` block of one gallery page.
pub fn gallery_images(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for img in document.select(&selector(".article-fulltext img")) {
        if let Some(src) = image_src(&img) {
            let src = absolute(&src);
            if !out.contains(&src) {
                out.push(src);
            }
        }
    }
    out
}

/// Last page number from the `nav.pagination` "End" link (`...?page=N`).
pub fn last_page(document: &Html) -> i64 {
    let Some(nav) = document.select(&selector("nav.pagination")).next() else {
        return 1;
    };
    let links: Vec<ElementRef<'_>> = nav.select(&selector("a.pagination-next")).collect();
    for link in links.iter().rev() {
        if let Some(href) = attr(link, "href")
            && let Some(page) = query_page(&href)
        {
            return page;
        }
    }
    1
}

fn query_page(href: &str) -> Option<i64> {
    href.split(['?', '&'])
        .find_map(|part| part.strip_prefix("page="))
        .and_then(|value| value.trim().parse::<i64>().ok())
}

/// Strip the ` - ( Page 1 / 11 )` suffix the site appends to every page title.
fn strip_page_suffix(title: &str) -> String {
    let title = title.trim();
    match title.rfind(" - ( Page ") {
        Some(position) => title[..position].trim().to_string(),
        None => title.to_string(),
    }
}

/// Parse a manga detail page.
pub fn manga_detail(document: &Html, path: &str) -> MangaInfo {
    let normalized = site_relative(path);

    let title = document
        .select(&selector(".article-header"))
        .next()
        .map(|element| strip_page_suffix(&text_of(&element)))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            normalized
                .rsplit('/')
                .next()
                .unwrap_or(normalized.as_str())
                .replace('-', " ")
        });

    let cover_url = gallery_images(document)
        .into_iter()
        .next()
        .unwrap_or_default();

    let mut genre: Vec<String> = Vec::new();
    for tag in document.select(&selector(".article-tags .tags > .tag")) {
        let name = text_of(&tag);
        let name = name.trim().trim_start_matches('#').trim();
        if !name.is_empty() && !genre.iter().any(|existing| existing == name) {
            genre.push(name.to_string());
        }
    }

    let mut description_parts: Vec<String> = Vec::new();
    if let Some(info) = document.select(&selector(".article-info > strong")).next() {
        let info = text_of(&info).replace("Buondua", "");
        let info = info.trim();
        if !info.is_empty() {
            description_parts.push(info.to_string());
        }
    }
    let download_links: Vec<String> = document
        .select(&selector(".article-links a[href]"))
        .filter_map(|link| {
            let href = attr(&link, "href")?;
            let text = text_of(&link);
            Some(format!("[{text}]({href})"))
        })
        .collect();
    if !download_links.is_empty() {
        description_parts.push(download_links.join("\n"));
    }
    if let Some(password) = document.select(&selector("code")).next() {
        let password = text_of(&password);
        let password = password.trim();
        if !password.is_empty() {
            description_parts.push(password.to_string());
        }
    }
    let description = {
        let description = description_parts.join("\n\n");
        (!description.is_empty()).then_some(description)
    };

    MangaInfo {
        source_id: ID,
        title,
        author: Vec::new(),
        genre,
        status: None,
        description,
        path: normalized,
        cover_url,
    }
}

/// One `Gallery` chapter per photo set: this port merges the site's pages.
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

/// Upload time from `.article-info > small` (`HH:mm dd-MM-yyyy`), as Unix millis.
fn uploaded_millis(document: &Html) -> i64 {
    let Some(element) = document.select(&selector(".article-info > small")).next() else {
        return 0;
    };
    let text = text_of(&element);
    NaiveDateTime::parse_from_str(text.trim(), "%H:%M %d-%m-%Y")
        .map(|date| date.and_utc().timestamp_millis())
        .unwrap_or(0)
}
