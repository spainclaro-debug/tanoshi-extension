//! HTML parsing for the MangaCatalog theme.
//!
//! Selectors were verified against the live franchise sites: the shared
//! Tailwind layout (`ReadOnePieceMangaOnline`-style), the Bootstrap-based
//! Berserk/One-Punch-Man layout and the WordPress Tower of God layout.

use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::SiteConfig;

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}

/// Collapse whitespace the same way Jsoup's `Element.text()` does.
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
    element
        .value()
        .attr(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolve `src`/`data-src` values that may be protocol-relative, absolute or
/// site-relative against the site root.
fn absolute(base: &str, value: &str) -> String {
    let value = value.trim();
    if value.starts_with("http") {
        return value.to_string();
    }
    if let Some(rest) = value.strip_prefix("//") {
        return format!("https://{rest}");
    }
    format!(
        "{}{}",
        base.trim_end_matches('/'),
        if value.starts_with('/') {
            value.to_string()
        } else {
            format!("/{value}")
        }
    )
}

/// Lazyload-aware image URL: prefer `data-src`/`data-lazy-src` over a
/// placeholder `src`, and skip inline data URIs.
fn image_src(element: &ElementRef<'_>) -> Option<String> {
    ["data-src", "data-lazy-src", "src"]
        .iter()
        .filter_map(|name| attr(element, name))
        .find(|value| !value.starts_with("data:"))
}

/// First matching element's collapsed text.
fn first_text(document: &Html, css: &str) -> Option<String> {
    document
        .select(&selector(css))
        .next()
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
}

fn first_image(document: &Html, css: &str, base: &str) -> Option<String> {
    document
        .select(&selector(css))
        .next()
        .and_then(|image| image_src(&image))
        .map(|src| absolute(base, &src))
}

fn attr_of_first(document: &Html, css: &str, name: &str) -> Option<String> {
    document
        .select(&selector(css))
        .next()
        .and_then(|element| attr(&element, name))
}

/// `Jul 10, 2026` (and a few close variants) to unix milliseconds.
fn date_millis(text: &str) -> i64 {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for format in ["%b %d, %Y", "%B %d, %Y", "%Y-%m-%d", "%d %b %Y"] {
        if let Ok(date) = NaiveDate::parse_from_str(&text, format) {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|value| value.and_utc().timestamp_millis())
                .unwrap_or(0);
        }
    }
    0
}

/// Pull a chapter number out of a label such as `Chapter 12.5` or `Vol. 2 Ch. 7`.
fn chapter_number(label: &str) -> f64 {
    let mut best: Option<f64> = None;
    let mut current = String::new();

    for character in label.chars() {
        if character.is_ascii_digit() || (character == '.' && !current.is_empty() && !current.contains('.')) {
            current.push(character);
        } else {
            if let Ok(value) = current.parse::<f64>() {
                best = Some(value);
            }
            current.clear();
        }
    }
    if let Ok(value) = current.parse::<f64>() {
        best = Some(value);
    }
    best.unwrap_or(-1.0)
}

/// Parse a manga detail page for the configured layout flavour.
pub fn manga_detail(document: &Html, site: &SiteConfig, url: &str) -> MangaInfo {
    let base = site.url.trim_end_matches('/');
    let mut manga = MangaInfo {
        source_id: site.id,
        title: String::new(),
        author: Vec::new(),
        genre: Vec::new(),
        status: None,
        description: None,
        path: url.to_string(),
        cover_url: String::new(),
    };

    match site.variant.as_str() {
        "berserk" | "opm" => {
            manga.title = first_text(document, "h2 > span").unwrap_or_default();
            if site.variant == "opm" {
                manga.title = manga
                    .title
                    .split_once("Manga: ")
                    .map(|(_, title)| title.trim().to_string())
                    .unwrap_or(manga.title);
            }
            let description = document
                .select(&selector("div.card-body > p"))
                .map(|element| text_of(&element))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !description.is_empty() {
                manga.description = Some(description);
            }
            manga.cover_url = first_image(document, ".card-img-right", base).unwrap_or_default();
        }
        "towerofgod" => {
            manga.title = "Tower of God".to_string();
            manga.cover_url = attr_of_first(document, "meta[property='og:image']", "content")
                .unwrap_or_default();
            manga.description = attr_of_first(document, "meta[property='og:description']", "content");
        }
        _ => {
            manga.title = first_text(document, "div.container > h1").unwrap_or_default();
            let info = first_text(document, "div.bg-bg-secondary > div.px-6 > div.flex-col")
                .unwrap_or_default();
            if let Some((_, description)) = info.split_once("Description") {
                let description = description.trim();
                if !description.is_empty() {
                    manga.description = Some(description.to_string());
                }
            }
            manga.cover_url = first_image(document, "div.flex > img", base).unwrap_or_default();
        }
    }

    if manga.title.is_empty() {
        let fallback = url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .replace('-', " ");
        manga.title = if fallback.is_empty() {
            site.name.clone()
        } else {
            fallback
        };
    }

    manga
}

fn chapter(
    source_id: i64,
    title: String,
    url: String,
    uploaded: i64,
) -> ChapterInfo {
    let number = chapter_number(&title);
    ChapterInfo {
        source_id,
        title,
        path: url,
        number,
        scanlator: None,
        uploaded,
    }
}

/// Parse the chapter list of a detail page for the configured layout flavour.
pub fn chapters(document: &Html, site: &SiteConfig) -> Vec<ChapterInfo> {
    let mut out: Vec<ChapterInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let push = |out: &mut Vec<ChapterInfo>, seen: &mut Vec<String>, chapter: ChapterInfo| {
        if chapter.path.is_empty() || seen.contains(&chapter.path) {
            return;
        }
        seen.push(chapter.path.clone());
        out.push(chapter);
    };

    match site.variant.as_str() {
        "berserk" | "opm" => {
            for row in document.select(&selector("tbody > tr")) {
                let name = row
                    .select(&selector("td:first-child"))
                    .next()
                    .map(|element| text_of(&element))
                    .unwrap_or_default();
                let Some(link) = row.select(&selector("a")).next() else {
                    continue;
                };
                let Some(href) = chapter_href(&link, site) else {
                    continue;
                };
                let uploaded = row
                    .select(&selector("td:nth-child(2)"))
                    .next()
                    .map(|element| date_millis(&text_of(&element)))
                    .unwrap_or(0);
                push(&mut out, &mut seen, chapter(site.id, name, href, uploaded));
            }
        }
        "towerofgod" => {
            for link in document.select(&selector("a[href*='tower-of-god-chapter-']")) {
                let Some(href) = chapter_href(&link, site) else {
                    continue;
                };
                let title = text_of(&link);
                let title = if title.is_empty() {
                    let number = chapter_number(&href).max(0.0);
                    format!("Chapter {number}")
                } else {
                    title
                };
                push(&mut out, &mut seen, chapter(site.id, title, href, 0));
            }
        }
        "aot" => {
            for column in document.select(&selector("div.w-full div.grid div.col-span-4")) {
                let Some(link) = column.select(&selector("a")).next() else {
                    continue;
                };
                let Some(href) = chapter_href(&link, site) else {
                    continue;
                };
                let mut title = text_of(&link);
                if let Some(subtitle) = column
                    .select(&selector("div.text-xs"))
                    .next()
                    .map(|element| text_of(&element))
                    .filter(|text| !text.is_empty())
                {
                    title = format!("{title} - {subtitle}");
                }
                push(&mut out, &mut seen, chapter(site.id, title, href, 0));
            }
        }
        _ => {
            for grid in document.select(&selector("div.w-full > div.bg-bg-secondary > div.grid")) {
                let Some(link) = grid.select(&selector(".col-span-4 > a")).next() else {
                    continue;
                };
                let Some(href) = chapter_href(&link, site) else {
                    continue;
                };
                let mut title = text_of(&link);
                if let Some(subtitle) = grid
                    .select(&selector(".text-xs:not(a)"))
                    .next()
                    .map(|element| text_of(&element))
                    .filter(|text| !text.is_empty())
                {
                    title = format!("{title} - {subtitle}");
                }
                push(&mut out, &mut seen, chapter(site.id, title, href, 0));
            }
        }
    }

    out
}

fn chapter_href(link: &ElementRef<'_>, site: &SiteConfig) -> Option<String> {
    let href = attr(link, "href")?;
    Some(absolute(&site.url.trim_end_matches('/'), &href))
}

/// Parse the page-image URLs of a chapter reader.
pub fn chapter_pages(document: &Html, site: &SiteConfig) -> Vec<String> {
    let base = site.url.trim_end_matches('/');
    let mut out: Vec<String> = Vec::new();

    let selector_value = match site.variant.as_str() {
        "berserk" | "opm" => "div.pages img.pages__img",
        // The Tower of God layout keeps the reader images in the post body;
        // older chapters serve them from bp.blogspot.com through a lazyload
        // placeholder, the newest chapters only carry the chapter cover.
        "towerofgod" => "div.entry-content img",
        _ => "img.js-page, img.pages__img",
    };

    for image in document.select(&selector(selector_value)) {
        let Some(src) = image_src(&image) else {
            continue;
        };
        let src = absolute(base, &src);
        if src.starts_with("data:") || src.contains("lazy_placeholder") {
            continue;
        }
        if !out.contains(&src) {
            out.push(src);
        }
    }

    out
}
