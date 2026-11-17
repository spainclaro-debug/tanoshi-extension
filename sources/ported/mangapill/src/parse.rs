//! HTML scraping for MangaPill.
//!
//! Selectors were verified against the live markup: the homepage `Trending
//! Mangas` grid, the `/chapters` listing, the `/search` result grid, a manga
//! detail page and a chapter page. The site renders Tailwind classes, so cards
//! are `<div>` elements without a class inside a `div.grid`, and lazily loaded
//! images carry the real URL in `data-src`.

use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, Input, MangaInfo};

use crate::filters;
use crate::{ID, NAME, URL};

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

/// Prefer the lazyloaded source, falling back to `src`, skipping placeholders.
fn image_src(img: &ElementRef<'_>) -> Option<String> {
    ["data-src", "src"]
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

/// Drop the scheme/host and query string from a stored path or full URL.
fn site_relative(value: &str) -> String {
    let value = value.trim();
    let value = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    // A stored path starts with `manga/` or `chapters/`, whereas a URL's first
    // segment is the host.
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

/// Site-relative manga path (`manga/{id}/{slug}`).
pub fn manga_path(value: &str) -> String {
    let cleaned = site_relative(value);
    if cleaned.is_empty() || cleaned.starts_with("manga/") {
        cleaned
    } else {
        format!("manga/{cleaned}")
    }
}

/// Site-relative chapter path (`chapters/{id}-{chapter}/{slug}`).
pub fn chapter_path(value: &str) -> String {
    let cleaned = site_relative(value);
    if cleaned.is_empty() || cleaned.starts_with("chapters/") {
        cleaned
    } else {
        format!("chapters/{cleaned}")
    }
}

pub fn manga_url(path: &str) -> String {
    format!("{URL}/{}", manga_path(path))
}

pub fn chapter_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{URL}/{}", chapter_path(path))
    }
}

/// `/search` URL with the keyword and the filters the client selected.
pub fn search_url(page: i64, query: &str, filters: Option<&[Input]>) -> String {
    let mut params: Vec<(&str, String)> =
        vec![("page", page.to_string()), ("q", query.to_string())];

    if let Some(filters) = filters {
        let filters = filters::search_filters(filters);
        if let Some(status) = filters.status {
            params.push(("status", status));
        }
        if let Some(kind) = filters.kind {
            params.push(("type", kind));
        }
        for genre in filters.genres {
            params.push(("genre", genre));
        }
    }

    let query_string = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");

    format!("{URL}/search?{query_string}")
}

/// Turn one listing card into a [`MangaInfo`].
///
/// Cards on the homepage, `/chapters` and `/search` share the same shape: an
/// `a[href^="/manga/"]` link, a lazyloaded `img` and a `div.line-clamp-2` title.
fn card(element: &ElementRef<'_>) -> Option<MangaInfo> {
    let link = element.select(&selector(r#"a[href^="/manga/"]"#)).next()?;
    let href = attr(&link, "href")?;
    let path = manga_path(&href);
    if path.is_empty() {
        return None;
    }

    let title = element
        .select(&selector("div.line-clamp-2"))
        .next()
        .map(|title| text_of(&title))
        .filter(|title| !title.is_empty())?;

    let cover_url = element
        .select(&selector("img"))
        .next()
        .and_then(|img| image_src(&img))
        .map(|src| absolute(&src))
        .unwrap_or_default();

    Some(MangaInfo {
        source_id: ID,
        title,
        author: Vec::new(),
        genre: Vec::new(),
        status: None,
        description: None,
        path,
        cover_url,
    })
}

fn push_unique(out: &mut Vec<MangaInfo>, manga: MangaInfo) {
    if !out.iter().any(|existing| existing.path == manga.path) {
        out.push(manga);
    }
}

/// Cards that are direct children of `grid` (there are nested grids on the
/// homepage, so descendants alone would double count).
fn grid_cards(grid: &ElementRef<'_>) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for element in grid.select(&selector("div:not([class])")) {
        let is_direct_child = element
            .parent()
            .and_then(ElementRef::wrap)
            .map(|parent| parent.id() == grid.id())
            .unwrap_or(false);
        if !is_direct_child {
            continue;
        }
        if let Some(manga) = card(&element) {
            push_unique(&mut out, manga);
        }
    }
    out
}

/// Cards from every grid on the page, restricted to direct children.
///
/// Used by `/chapters` and `/search`; both render a single result grid. Any
/// other classless grid children are filtered out by [`card`] requiring a manga
/// link and title.
pub fn listing(document: &Html) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for element in document.select(&selector("div.grid > div:not([class])")) {
        if let Some(manga) = card(&element) {
            push_unique(&mut out, manga);
        }
    }
    out
}

/// Trending titles from the homepage.
///
/// The original selects `div:has(h4:contains(Trending)) > .grid > div`. Jsoup's
/// `:has`/`:contains` are not available in `scraper`, so the grid is found by
/// checking whether the heading container above it mentions "Trending".
pub fn popular(document: &Html) -> Vec<MangaInfo> {
    let heading = selector("h4");
    for grid in document.select(&selector("div.grid")) {
        let Some(container) = grid.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        let is_trending = container
            .select(&heading)
            .any(|heading| text_of(&heading).to_ascii_lowercase().contains("trending"));
        if !is_trending {
            continue;
        }
        let mangas = grid_cards(&grid);
        if !mangas.is_empty() {
            return mangas;
        }
    }

    // The heading layout changed; the Trending grid is the last one on the page.
    log::warn!("{NAME}: 'Trending Mangas' heading not found; using the last homepage grid");
    document
        .select(&selector("div.grid"))
        .last()
        .map(|grid| grid_cards(&grid))
        .unwrap_or_default()
}

/// Value cell of the detail page's `Type` / `Status` / `Year` info grid.
fn info_value(document: &Html, wanted: &str) -> Option<String> {
    for label in document.select(&selector("label")) {
        if !text_of(&label).eq_ignore_ascii_case(wanted) {
            continue;
        }
        let Some(row) = label.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        for value in row.select(&selector("div")) {
            let text = text_of(&value);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Mirror the original's `parseStatus`: only publishing/finished are mapped.
fn normalize_status(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    if lower.contains("publishing") {
        Some("Ongoing".to_string())
    } else if lower.contains("finished") {
        Some("Completed".to_string())
    } else {
        None
    }
}

/// Parse a manga detail page.
pub fn manga_detail(document: &Html, path: &str) -> MangaInfo {
    let normalized = manga_path(path);

    let title = document
        .select(&selector("h1"))
        .next()
        .map(|title| text_of(&title))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            normalized
                .rsplit('/')
                .next()
                .unwrap_or(normalized.as_str())
                .replace('-', " ")
        });

    let cover_url = document
        .select(&selector("img[data-src]"))
        .next()
        .and_then(|img| image_src(&img))
        .map(|src| absolute(&src))
        .unwrap_or_default();

    let description = document
        .select(&selector("p.text--secondary"))
        .next()
        .map(|description| text_of(&description))
        .filter(|description| !description.is_empty());

    let status = info_value(document, "Status").and_then(|value| normalize_status(&value));

    let mut genre: Vec<String> = Vec::new();
    for link in document.select(&selector(r#"a[href*="genre"]"#)) {
        let name = text_of(&link);
        if !name.is_empty() && !genre.contains(&name) {
            genre.push(name);
        }
    }

    MangaInfo {
        source_id: ID,
        title,
        author: Vec::new(),
        genre,
        status,
        description,
        path: normalized,
        cover_url,
    }
}

/// Chapter list, newest first, from `#chapters > div > a`.
pub fn chapters(document: &Html) -> Vec<ChapterInfo> {
    let mut links: Vec<ElementRef<'_>> =
        document.select(&selector("#chapters > div > a")).collect();
    if links.is_empty() {
        // The grid wrapper was restyled away; fall back to any chapter link.
        links = document.select(&selector("#chapters a")).collect();
    }

    let mut out: Vec<ChapterInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for link in links {
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        if !href.contains("/chapters/") {
            continue;
        }
        let path = chapter_path(&href);
        if path.is_empty() || seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());

        let raw_title = text_of(&link);
        let number = chapter_number(&raw_title).unwrap_or(-1.0);
        out.push(ChapterInfo {
            source_id: ID,
            title: if raw_title.is_empty() {
                format!("Chapter {number}")
            } else {
                raw_title
            },
            path,
            number,
            scanlator: None,
            uploaded: 0,
        });
    }

    out
}

/// Page image URLs from a chapter page (`picture img[data-src]`).
pub fn chapter_pages(document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_pages(document, "picture img", &mut out);
    if out.is_empty() {
        // Defensive fallback in case the `<picture>` wrapper is dropped.
        collect_pages(document, "img.js-page", &mut out);
    }
    out
}

fn collect_pages(document: &Html, selector_text: &str, out: &mut Vec<String>) {
    for img in document.select(&selector(selector_text)) {
        if let Some(src) = image_src(&img) {
            let src = absolute(&src);
            if !out.contains(&src) {
                out.push(src);
            }
        }
    }
}

/// Pull the chapter number out of a label such as `Chapter 12` or `Chapter 22.3`.
pub fn chapter_number(label: &str) -> Option<f64> {
    let mut best: Option<f64> = None;
    let mut current = String::new();

    for character in label.chars() {
        if character.is_ascii_digit()
            || (character == '.' && !current.is_empty() && !current.contains('.'))
        {
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
    best
}
