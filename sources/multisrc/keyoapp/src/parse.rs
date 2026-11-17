//! HTML parsing for the Keyoapp theme.
//!
//! The classic layout was verified on Kewn Scans / Eris Scans / Writerscans
//! (series cards are `<button class="group ...">` with a CSS `background-image`
//! cover inside a "Trending" widget, `/latest/` rows are `div.group`, detail
//! pages carry a `#expand_content` synopsis and label/value rows, chapter pages
//! hold `#pages > img[uid]` images served from the CDN named in a `realUrl`
//! script). The immersive-reader layout was verified on Kitsune Dawn / Rithar
//! Scans (`[wire:key*=serie]` cards, `/search?title=`, `div.chapter-card`
//! rows and a `pages: JSON.parse('...')` payload in the reader's `x-data`).
//!
//! Jsoup-only selector extensions (`:contains`, `:has`, `:eq`, `:containsOwn`)
//! used by the Kotlin theme are re-implemented by hand.

use chrono::{NaiveDate, Utc};
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

/// `url(https://...)` out of a CSS declaration.
fn css_url(style: &str) -> Option<String> {
    let start = style.find("url(")? + 4;
    let rest = style[start..].trim_start();
    let rest = rest.trim_start_matches(['\'', '"']);
    let end = rest.find(['\'', '"', ')'])?;
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn next_element_sibling<'a>(element: ElementRef<'a>) -> Option<ElementRef<'a>> {
    let mut node = element.next_sibling();
    while let Some(current) = node {
        if let Some(element) = ElementRef::wrap(current) {
            return Some(element);
        }
        node = current.next_sibling();
    }
    None
}

/// Parse one card (`button.group`, `div.group`, splide slide, `[wire:key]`).
fn card(element: &ElementRef<'_>, source_id: i64, base: &str) -> Option<MangaInfo> {
    let link = element.select(&selector("a[href]")).next()?;
    let href = attr(&link, "href")?;
    let mut title = attr(&link, "title").unwrap_or_default();
    if title.is_empty() {
        title = text_of(&link);
    }
    let mut cover_url = element
        .select(&selector("[style*=background-image]"))
        .next()
        .and_then(|element| attr(&element, "style"))
        .and_then(|style| css_url(&style))
        .unwrap_or_default();
    if cover_url.is_empty() {
        cover_url = element
            .select(&selector("img"))
            .next()
            .and_then(|image| image_src(&image))
            .map(|src| absolute(base, &src))
            .unwrap_or_default();
    }
    if title.is_empty() {
        return None;
    }
    Some(MangaInfo {
        source_id,
        title,
        author: Vec::new(),
        genre: Vec::new(),
        status: None,
        description: None,
        path: absolute(base, &href),
        cover_url,
    })
}

fn dedupe(mut mangas: Vec<MangaInfo>) -> Vec<MangaInfo> {
    let mut seen: Vec<String> = Vec::new();
    mangas.retain(|manga| {
        if manga.path.is_empty() || seen.contains(&manga.path) {
            return false;
        }
        seen.push(manga.path.clone());
        true
    });
    mangas
}

/// Cards of the immersive-reader layout (`[wire:key*=serie]`).
pub fn wire_cards(document: &Html, source_id: i64, base: &str) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for container in document.select(&selector("[wire\\:key*='serie']")) {
        let Some(link) = container.select(&selector("a[href]")).next() else {
            continue;
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let mut title = attr(&link, "title").unwrap_or_default();
        if title.is_empty() {
            title = attr(&link, "aria-label").unwrap_or_default();
        }
        if title.is_empty() {
            continue;
        }
        // Covers are plain <img> tags in this layout; the per-site Kotlin
        // override picks the one whose alt ends in " cover".
        let cover_url = container
            .select(&selector("img[alt$=' cover']"))
            .next()
            .or_else(|| container.select(&selector("img")).next())
            .and_then(|image| image_src(&image))
            .map(|src| absolute(base, &src))
            .unwrap_or_default();
        out.push(MangaInfo {
            source_id,
            title,
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path: absolute(base, &href),
            cover_url,
        });
    }
    dedupe(out)
}

/// Rows of the immersive-reader `/latest` page.
pub fn wire_latest_cards(document: &Html, source_id: i64, base: &str) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for container in document.select(&selector("div.group.latest-poster")) {
        if let Some(manga) = card(&container, source_id, base) {
            out.push(manga);
        }
    }
    dedupe(out)
}

/// Server-rendered result cards of `#searched_series_page`.
pub fn searched_cards(document: &Html, source_id: i64, base: &str) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for container in document.select(&selector("#searched_series_page > *")) {
        if let Some(manga) = card(&container, source_id, base) {
            out.push(manga);
        }
    }
    dedupe(out)
}

/// Cards of the classic home page "Trending"/"Popular" widget.
///
/// Mirrors the theme's `div:contains(<title>) + div <items>` selector: find the
/// heading, take its containing header `div`, then the adjacent sibling `div`
/// that wraps the carousel and select the item elements inside it.
pub fn trending_cards(
    document: &Html,
    source_id: i64,
    base: &str,
    item_selector: &str,
    direct_heading: bool,
) -> Vec<MangaInfo> {
    const TITLES: [&str; 3] = ["Popular", "Popularie", "Trending"];
    let items = selector(item_selector);
    let mut out = Vec::new();

    for heading in document.select(&selector("h2, h3")) {
        let heading_text = text_of(&heading);
        if !TITLES
            .iter()
            .any(|title| heading_text.contains(title))
        {
            continue;
        }
        let Some(header) = heading.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        if direct_heading && header.value().name() != "div" {
            continue;
        }
        let Some(track) = next_element_sibling(header) else {
            continue;
        };
        if track.value().name() != "div" {
            continue;
        }
        for item in track.select(&items) {
            if let Some(manga) = card(&item, source_id, base) {
                out.push(manga);
            }
        }
    }

    dedupe(out)
}

/// Splide slides used by the sites whose home carousel is a splide slider.
pub fn splide_cards(document: &Html, source_id: i64, base: &str) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for slide in document.select(&selector(
        ".series-splide .splide__slide:not(.splide__slide--clone)",
    )) {
        let Some(link) = slide.select(&selector("a[href]")).next() else {
            continue;
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let mut title = attr(&link, "title").unwrap_or_default();
        if title.is_empty() {
            title = text_of(&link);
        }
        let cover_url = slide
            .select(&selector("[style*=background-image]"))
            .next()
            .and_then(|element| attr(&element, "style"))
            .and_then(|style| css_url(&style))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        out.push(MangaInfo {
            source_id,
            title,
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path: absolute(base, &href),
            cover_url,
        });
    }
    dedupe(out)
}

/// Cards of a `/latest/` page.
pub fn latest_cards(document: &Html, source_id: i64, base: &str, item_selector: &str) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    for container in document.select(&selector(item_selector)) {
        if let Some(manga) = card(&container, source_id, base) {
            out.push(manga);
        }
    }
    dedupe(out)
}

/// Text of the value cell that follows a label `<span>` (classic detail rows).
fn label_value(document: &Html, label: &str) -> Option<String> {
    for span in document.select(&selector("span")) {
        if !text_of(&span).eq_ignore_ascii_case(label) {
            continue;
        }
        let Some(row) = span.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        let Some(value) = next_element_sibling(row) else {
            continue;
        };
        let text = text_of(&value);
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// Texts of the divs/sections that follow the "Alternative titles" label.
fn alternative_titles(document: &Html) -> Vec<String> {
    let mut out = Vec::new();
    for label in document.select(&selector("div.font-medium")) {
        if !text_of(&label).contains("Alternative titles") {
            continue;
        }
        let Some(container) = label.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        let mut node = container.next_sibling();
        while let Some(current) = node {
            if let Some(element) = ElementRef::wrap(current) {
                if element.value().name() == "div" {
                    for span in element.select(&selector("span.select-all")) {
                        let text = text_of(&span);
                        if !text.is_empty() && !out.contains(&text) {
                            out.push(text);
                        }
                    }
                }
            }
            node = current.next_sibling();
        }
    }
    out
}

fn normalize_status(value: &str) -> Option<String> {
    let lower = value.trim().to_ascii_lowercase();
    let status = if lower.contains("ongoing") {
        "Ongoing"
    } else if lower.contains("completed") {
        "Completed"
    } else if lower.contains("dropped") || lower.contains("cancelled") || lower.contains("canceled") {
        "Cancelled"
    } else if lower.contains("paused") || lower.contains("hiatus") {
        "Hiatus"
    } else if lower.is_empty() {
        return None;
    } else {
        value.trim()
    };
    Some(status.to_string())
}

fn titlecase(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn genre_links(document: &Html) -> Vec<String> {
    // `div:has(>h1) a[href*=genre=]`: genre anchors sit in the same container
    // as the title heading.
    for heading in document.select(&selector("h1")) {
        let parent = heading.parent().and_then(ElementRef::wrap);
        let grandparent = parent.and_then(|element| element.parent().and_then(ElementRef::wrap));
        for ancestor in [parent, grandparent] {
            let Some(container) = ancestor else {
                continue;
            };
            let mut genres = Vec::new();
            for link in container.select(&selector("a[href*='genre=']")) {
                let text = text_of(&link).trim_matches([',', ' ']).to_string();
                if !text.is_empty() && !genres.contains(&text) {
                    genres.push(text);
                }
            }
            if !genres.is_empty() {
                return genres;
            }
        }
    }
    Vec::new()
}

fn wire_meta_value(document: &Html, label: &str) -> Option<String> {
    for term in document.select(&selector("dt")) {
        if !text_of(&term).to_ascii_lowercase().contains(&label.to_ascii_lowercase()) {
            continue;
        }
        let Some(value) = next_element_sibling(term) else {
            continue;
        };
        let text = text_of(&value);
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn wire_alt_titles(document: &Html) -> Vec<String> {
    let mut out = Vec::new();
    for item in document.select(&selector(
        "details[data-testid='series-other-names'] li.select-all",
    )) {
        let text = text_of(&item);
        if !text.is_empty() && !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

fn aria_value(document: &Html, label: &str) -> Option<String> {
    let element = document
        .select(&selector(&format!("a[aria-label='{label}']")))
        .next()?;
    let text = element
        .select(&selector("span"))
        .next()
        .map(|span| text_of(&span))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| text_of(&element));
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
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

    let mut alt_names: Vec<String> = Vec::new();

    match site.detail_mode() {
        "wire" => {
            manga.title = document
                .select(&selector("h1"))
                .next()
                .map(|element| text_of(&element))
                .unwrap_or_default();
            manga.description = document
                .select(&selector("#expand_content"))
                .next()
                .map(|element| text_of(&element))
                .filter(|text| !text.is_empty());
            manga.cover_url = document
                .select(&selector("meta[property='og:image']"))
                .next()
                .and_then(|element| attr(&element, "content"))
                .unwrap_or_default();
            alt_names = wire_alt_titles(document);
            manga.status = aria_value(document, "Status").and_then(|value| normalize_status(&value));
            manga.author = wire_meta_value(document, "Author")
                .map(|value| vec![value])
                .unwrap_or_default();
            let artist = wire_meta_value(document, "Artist").unwrap_or_default();
            if !artist.is_empty() {
                manga.author.push(artist);
            }
            let mut genres = Vec::new();
            if let Some(value) = aria_value(document, "Type") {
                let value = titlecase(&value);
                if !value.is_empty() {
                    genres.push(value);
                }
            }
            for link in document.select(&selector("a[href*='/genres/']")) {
                let text = text_of(&link).trim_matches([',', ' ']).to_string();
                if !text.is_empty() && !genres.contains(&text) {
                    genres.push(text);
                }
            }
            manga.genre = genres;
        }
        "erisscans" => {
            manga.title = document
                .select(&selector("h1"))
                .next()
                .map(|element| text_of(&element))
                .unwrap_or_default();
            manga.description = document
                .select(&selector("#expand_content p"))
                .next()
                .map(|element| text_of(&element))
                .filter(|text| !text.is_empty());
            manga.cover_url = cover_from_style(document, base);
            alt_names = alternative_titles(document);
            manga.status = document
                .select(&selector("div[alt=Status]"))
                .next()
                .map(|element| text_of(&element))
                .and_then(|value| normalize_status(&value));
            manga.author = value_of_selector(document, "div[alt=Author]")
                .map(|value| vec![value])
                .unwrap_or_default();
            if let Some(artist) = value_of_selector(document, "div[alt=Artist]") {
                manga.author.push(artist);
            }
            let mut genres = Vec::new();
            if let Some(value) = value_of_selector(document, "div[alt='Series Type']") {
                let value = titlecase(&value);
                if !value.is_empty() {
                    genres.push(value);
                }
            }
            for genre in genre_links(document) {
                if !genres.contains(&genre) {
                    genres.push(genre);
                }
            }
            manga.genre = genres;
        }
        "nyanukafe" => {
            manga.title = document
                .select(&selector("h1"))
                .next()
                .map(|element| text_of(&element))
                .unwrap_or_default();
            manga.description = document
                .select(&selector("#expand_content p"))
                .next()
                .map(|element| text_of(&element))
                .filter(|text| !text.is_empty());
            manga.cover_url = cover_from_style(document, base);
            alt_names = alternative_titles(document);
            // `div.w-full.flex-wrap > div:eq(N) > div:last-child`: rows of
            // label/value pairs in document order.
            let rows: Vec<ElementRef<'_>> = document
                .select(&selector("div.w-full.flex-wrap > div"))
                .collect();
            let row_value = |index: usize| -> Option<String> {
                let row = rows.get(index)?;
                let mut value = row.select(&selector("div:last-child")).next().map(|element| text_of(&element));
                if value.as_deref().unwrap_or("").is_empty() {
                    value = Some(text_of(row));
                }
                value.filter(|text| !text.is_empty())
            };
            manga.author = row_value(0).map(|value| vec![value]).unwrap_or_default();
            if let Some(artist) = row_value(1) {
                manga.author.push(artist);
            }
            let mut genres = Vec::new();
            if let Some(value) = row_value(2) {
                let value = titlecase(&value);
                if !value.is_empty() {
                    genres.push(value);
                }
            }
            for genre in genre_links(document) {
                if !genres.contains(&genre) {
                    genres.push(genre);
                }
            }
            manga.genre = genres;
            manga.status = row_value(3).and_then(|value| normalize_status(&value));
        }
        _ => {
            manga.title = document
                .select(&selector("h1"))
                .next()
                .map(|element| text_of(&element))
                .unwrap_or_default();
            manga.description = document
                .select(&selector("#expand_content p"))
                .next()
                .map(|element| text_of(&element))
                .filter(|text| !text.is_empty());
            manga.cover_url = cover_from_style(document, base);
            alt_names = alternative_titles(document);
            manga.status = label_value(document, "Status").and_then(|value| normalize_status(&value));
            manga.author = label_value(document, "Author").map(|value| vec![value]).unwrap_or_default();
            if let Some(artist) = label_value(document, "Artist") {
                manga.author.push(artist);
            }
            let mut genres = Vec::new();
            if let Some(value) = label_value(document, "Type") {
                let value = titlecase(&value);
                if !value.is_empty() {
                    genres.push(value);
                }
            }
            for genre in genre_links(document) {
                if !genres.contains(&genre) {
                    genres.push(genre);
                }
            }
            manga.genre = genres;
        }
    }

    if !alt_names.is_empty() {
        let mut description = manga.description.unwrap_or_default();
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative titles\n");
        for name in alt_names {
            description.push_str(&format!("- {name}\n"));
        }
        manga.description = Some(description.trim().to_string());
    }

    if manga.title.is_empty() {
        manga.title = site.name.clone();
    }
    manga
}

fn cover_from_style(document: &Html, base: &str) -> String {
    document
        .select(&selector("div[style*=photoURL]"))
        .next()
        .or_else(|| document.select(&selector("div[class*=photoURL]")).next())
        .and_then(|element| attr(&element, "style"))
        .and_then(|style| css_url(&style))
        .or_else(|| {
            document
                .select(&selector("div[class*=photoURL]"))
                .next()
                .and_then(|element| attr(&element, "style"))
                .and_then(|style| css_url(&style))
        })
        .or_else(|| {
            document
                .select(&selector("meta[property='og:image']"))
                .next()
                .and_then(|element| attr(&element, "content"))
        })
        .map(|value| absolute(base, &value))
        .unwrap_or_default()
}

fn value_of_selector(document: &Html, css: &str) -> Option<String> {
    document
        .select(&selector(css))
        .next()
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
}

/// Relative (`2 days ago`) or absolute (`Aug 3, 2026`) dates to unix millis.
pub fn parse_date(text: &str) -> i64 {
    let text = text.trim();
    if text.is_empty() {
        return 0;
    }
    if text.contains("ago") {
        let lower = text.to_ascii_lowercase();
        let mut words = lower.split_whitespace();
        let amount_text = words.next().unwrap_or("");
        let amount: i64 = match amount_text {
            "a" | "one" => 1,
            value => value.parse().unwrap_or(0),
        };
        let unit = words.next().unwrap_or("");
        let seconds = match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600,
            "d" | "day" | "days" => 86_400,
            "w" | "wk" | "wks" | "week" | "weeks" => 604_800,
            "mo" | "mos" | "month" | "months" => 2_592_000,
            "y" | "yr" | "yrs" | "year" | "years" => 31_536_000,
            _ => return 0,
        };
        return (Utc::now().timestamp_millis()) - amount * seconds * 1_000;
    }
    for format in ["%b %d, %Y", "%B %d, %Y", "%Y-%m-%d"] {
        if let Ok(date) = NaiveDate::parse_from_str(text, format) {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|value| value.and_utc().timestamp_millis())
                .unwrap_or(0);
        }
    }
    0
}

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

/// Parse the chapter list for the configured layout flavour.
pub fn chapters(document: &Html, site: &SiteConfig) -> Vec<ChapterInfo> {
    let mut out: Vec<ChapterInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let base = site.url.trim_end_matches('/');

    let push = |out: &mut Vec<ChapterInfo>, seen: &mut Vec<String>, title: String, url: String, uploaded: i64| {
        if url.is_empty() || seen.contains(&url) {
            return;
        }
        seen.push(url.clone());
        let number = chapter_number(&title);
        out.push(ChapterInfo {
            source_id: site.id,
            title,
            path: url,
            number,
            scanlator: None,
            uploaded,
        });
    };

    if site.chapters_mode() == "wire" {
        for link in document.select(&selector("div.chapter-card a[href*='/read/']")) {
            let Some(href) = attr(&link, "href") else {
                continue;
            };
            let mut title = attr(&link, "title").unwrap_or_default();
            if title.is_empty() {
                title = attr(&link, "aria-label").unwrap_or_default();
            }
            if title.is_empty() {
                continue;
            }
            push(&mut out, &mut seen, title, absolute(base, &href), 0);
        }
        return out;
    }

    for element in document.select(&selector("#chapters > a, #chapters > div")) {
        let element_text = text_of(&element);
        if element_text.to_ascii_lowercase().contains("upcoming") {
            continue;
        }
        if element.select(&selector("img[alt~=Coin]")).next().is_some() {
            continue;
        }
        let link = if element.value().name() == "a" && attr(&element, "href").is_some() {
            element
        } else {
            match element.select(&selector("a[href]")).next() {
                Some(link) => link,
                None => continue,
            }
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let mut title = element
            .select(&selector(".text-sm"))
            .next()
            .map(|element| text_of(&element))
            .unwrap_or_default();
        if title.is_empty() {
            title = attr(&link, "title").unwrap_or_default();
        }
        if title.is_empty() {
            title = text_of(&link);
        }
        if title.is_empty() {
            continue;
        }
        let uploaded = element
            .select(&selector(".text-xs"))
            .next()
            .map(|element| parse_date(&text_of(&element)))
            .unwrap_or(0);
        push(&mut out, &mut seen, title, absolute(base, &href), uploaded);
    }

    out
}

#[derive(serde::Deserialize)]
struct WirePage {
    path: String,
}

/// Unescape a JS single-quoted string literal into its contents.
fn unescape_js_string(raw: &str) -> Option<String> {
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()
}

/// Page image URLs of a chapter reader.
pub fn pages(document: &Html, site: &SiteConfig, chapter_url: &str) -> anyhow::Result<Vec<String>> {
    let base = site.url.trim_end_matches('/');

    if site.pages_mode() == "wire" {
        // The reader embeds its page list in `x-data`:
        //   immersiveReader({ canRead: ..., pages: JSON.parse('[{"path":"..."}]') })
        let mut payload = None;
        for element in document.select(&selector("[x-data]")) {
            if let Some(value) = attr(&element, "x-data") {
                if value.starts_with("immersiveReader") {
                    payload = Some(value);
                    break;
                }
            }
        }
        let Some(payload) = payload else {
            anyhow::bail!("keyoapp: no immersive reader data at {chapter_url}");
        };
        let Some((_, after_parse)) = payload.split_once("pages: JSON.parse('") else {
            // `pages: []` means the chapter is locked for anonymous readers.
            return Ok(Vec::new());
        };
        let Some((raw, _)) = after_parse.split_once("')") else {
            return Ok(Vec::new());
        };
        let unescaped = match unescape_js_string(raw) {
            Some(value) => value,
            None => return Ok(Vec::new()),
        };
        let pages: Vec<WirePage> = serde_json::from_str(&unescaped).unwrap_or_default();
        let mut out = Vec::new();
        for page in pages {
            let path = page.path.trim().to_string();
            if path.is_empty() {
                continue;
            }
            let url = absolute(base, &path);
            if !out.contains(&url) {
                out.push(url);
            }
        }
        return Ok(out);
    }

    // Classic reader: pages are `<img uid>` placeholders hydrated from the
    // CDN host named in the `realUrl` script.
    let mut cdn: Option<String> = None;
    for script in document.select(&selector("script")) {
        let html = script.inner_html();
        if !html.contains("realUrl") {
            continue;
        }
        if let Some(host) = cdn_host_from_script(&html) {
            cdn = Some(host);
            break;
        }
    }

    let mut out = Vec::new();
    if let Some(host) = cdn {
        let prefix = format!("https://{host}/uploads");
        for image in document.select(&selector("#pages img[uid]")) {
            let Some(uid) = attr(&image, "uid") else {
                continue;
            };
            let url = format!("{prefix}/{uid}");
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }

    if out.is_empty() {
        // Older pages serve real CDN URLs directly.
        for image in document.select(&selector("#pages img")) {
            let Some(src) = image_src(&image) else {
                continue;
            };
            if !is_keyoapp_cdn(&src) {
                continue;
            }
            let url = absolute(base, &src);
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }

    Ok(out)
}

/// CDN host out of a reader script such as
/// `let realUrl = `https://cdn.example.org/uploads/${uid}`;`.
fn cdn_host_from_script(html: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(position) = html[search_from..].find("realUrl") {
        let rest = &html[search_from + position + "realUrl".len()..];
        if let Some(scheme) = rest.find("//") {
            let after = &rest[scheme + 2..];
            let end = after
                .find(|character: char| character == '/' || character == '`' || character.is_whitespace())
                .unwrap_or(after.len());
            let mut host = after[..end].to_string();
            // Drop JS template interpolations such as `${subdomain}`.
            while let Some(start) = host.find("${") {
                match host[start..].find('}') {
                    Some(end) => host.replace_range(start..start + end + 1, ""),
                    None => break,
                }
            }
            let host = host.trim().to_string();
            if !host.is_empty() && host.contains('.') {
                return Some(host);
            }
        }
        search_from += position + "realUrl".len();
    }
    None
}

/// Whether an image URL points at the old `cdnN.keyoapp.com` CDN.
fn is_keyoapp_cdn(url: &str) -> bool {
    let rest = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("//");
    let host = rest.split('/').next().unwrap_or("");
    let Some(subdomain) = host.strip_suffix(".keyoapp.com") else {
        return false;
    };
    let digits = subdomain.strip_prefix("cdn").unwrap_or("");
    subdomain == "cdn" || (subdomain.starts_with("cdn") && digits.chars().all(|c| c.is_ascii_digit()))
}

/// Keep only cards whose title matches the query (the classic theme's
/// client-side `matchesQuery` filter).
pub fn filter_by_query(cards: Vec<MangaInfo>, query: &str) -> Vec<MangaInfo> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return cards;
    }
    cards
        .into_iter()
        .filter(|manga| manga.title.to_lowercase().contains(&needle))
        .collect()
}
