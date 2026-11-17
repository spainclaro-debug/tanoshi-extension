//! HTML/JSON parsing helpers for the `galleryadults` theme.
//!
//! The Keiyoushi theme is one shared scraper with per-site selector overrides.
//! Every function here takes the site slug from [`crate::Site`] and applies the
//! same selector the Kotlin override uses, so the Rust behaviour stays as close
//! to the theme as possible.

use anyhow::{Result, anyhow};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use tanoshi_lib::prelude::MangaInfo;

use crate::{Site, parse_date_to_epoch};

pub fn sel(pattern: &str) -> Option<Selector> {
    Selector::parse(pattern).ok()
}

/// All elements matching `pattern` (empty when the selector is invalid).
pub fn select<'a>(html: &'a Html, pattern: &str) -> Vec<ElementRef<'a>> {
    match sel(pattern) {
        Some(selector) => html.select(&selector).collect(),
        None => Vec::new(),
    }
}

pub fn select_in<'a>(element: &'a ElementRef<'a>, pattern: &str) -> Vec<ElementRef<'a>> {
    match sel(pattern) {
        Some(selector) => element.select(&selector).collect(),
        None => Vec::new(),
    }
}

pub fn first<'a>(html: &'a Html, pattern: &str) -> Option<ElementRef<'a>> {
    select(html, pattern).into_iter().next()
}

pub fn first_in<'a>(element: &'a ElementRef<'a>, pattern: &str) -> Option<ElementRef<'a>> {
    select_in(element, pattern).into_iter().next()
}

pub fn text_of(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_string()
}

pub fn element_text(element: Option<ElementRef<'_>>) -> Option<String> {
    element.map(|element| {
        let text = text_of(element);
        if text.is_empty() { None } else { Some(text) }
    })?
}

pub fn text_first(html: &Html, pattern: &str) -> Option<String> {
    element_text(first(html, pattern))
}

pub fn attr_first(html: &Html, pattern: &str, attribute: &str) -> Option<String> {
    first(html, pattern)
        .and_then(|element| element.value().attr(attribute))
        .map(str::to_string)
}

/// Text nodes that belong directly to this element (no descendants), which is
/// how jsoup's `ownText()` behaves.
pub fn own_text(element: ElementRef<'_>) -> String {
    use scraper::node::Node;
    let mut out = String::new();
    for child in element.children() {
        if let Node::Text(text) = child.value() {
            out.push_str(&text.text);
        }
    }
    out.trim().to_string()
}

/// Resolve a URL found in the page against the site root. Handles absolute
/// URLs, protocol-relative URLs and site-relative paths.
pub fn abs_url(base: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("//") {
        let scheme = base.split(':').next().unwrap_or("https");
        return Some(format!("{scheme}://{rest}"));
    }
    let scheme = base.split(':').next().unwrap_or("https");
    if raw.starts_with('/') {
        return Some(format!("{scheme}://{}{raw}", host_of(base)));
    }
    Some(format!("{}/{}", base.trim_end_matches('/'), raw))
}

fn host_of(base: &str) -> &str {
    base.split("://").nth(1).unwrap_or(base).split('/').next().unwrap_or("")
}

/// Site-relative path (`/g/123/`) for a link found on the site.
pub fn path_of(base: &str, url: &str) -> String {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) {
        if let Some(index) = rest.find('/') {
            return rest[index..].to_string();
        }
        return "/".to_string();
    }
    if url.starts_with("//") {
        if let Some(index) = url[2..].find('/') {
            return url[2 + index..].to_string();
        }
        return "/".to_string();
    }
    if url.starts_with('/') {
        return url.to_string();
    }
    format!("/{url}")
}

/// The theme's `imgAttr()` order: `data-cfsrc`, `data-src`, `data-lazy-src`,
/// `srcset`, `src`.
pub fn img_attr(element: &ElementRef<'_>, base: &str) -> Option<String> {
    for attribute in ["data-cfsrc", "data-src", "data-lazy-src"] {
        if let Some(value) = element.value().attr(attribute) {
            if let Some(url) = abs_url(base, value) {
                return Some(url);
            }
        }
    }
    if let Some(value) = element.value().attr("srcset") {
        let first = value.split(' ').next().unwrap_or("");
        if let Some(url) = abs_url(base, first) {
            return Some(url);
        }
    }
    element
        .value()
        .attr("src")
        .and_then(|value| abs_url(base, value))
}

pub fn image_in(element: ElementRef<'_>, pattern: &str, base: &str) -> Option<String> {
    first_in(&element, pattern).and_then(|image| img_attr(&image, base))
}

/// Value of `<input id="<id>">` (the theme's `inputIdValueOf`).
pub fn input_value(html: &Html, id: &str) -> Option<String> {
    let pattern = format!("input[id={id}]");
    first(html, &pattern)
        .and_then(|input| input.value().attr("value"))
        .map(str::to_string)
}

pub fn input_value_or_empty(html: &Html, id: &str) -> String {
    input_value(html, id).unwrap_or_default()
}

/// csrf token used by the thumbnail loader forms.
pub fn csrf_token(html: &Html) -> Option<String> {
    first(html, "[name=csrf-token]")
        .and_then(|element| element.value().attr("content"))
        .map(str::to_string)
}

/// Convert thumbnail URLs to full image URLs: `.../12t.jpg` -> `.../12.jpg`.
pub fn thumbnail_to_full(url: &str) -> String {
    let ext = url.rsplit('.').next().unwrap_or("");
    if ext.is_empty() || ext.contains('/') {
        return url.to_string();
    }
    url.replace(&format!("t.{ext}"), &format!(".{ext}"))
}

/// Parse the theme's `$.parseJSON('...')` page metadata.
pub fn parse_json_script(html: &Html) -> Option<String> {
    for script in select(html, "script") {
        let data = script.text().collect::<String>();
        if let Some(rest) = data.split("$.parseJSON('").nth(1) {
            if let Some(inner) = rest.split("');").next() {
                let inner = inner.trim();
                if !inner.is_empty() {
                    return Some(inner.to_string());
                }
            }
        }
    }
    None
}

/// hentaiera/hentaifox/imhentai page metadata: script embedded JSON object of
/// `{"1":"j,1280,720", ...}` plus `load_dir`/`load_id` inputs.
pub fn parse_json_object(raw: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_object()?;
    Some(value)
}

/// Image extension from the theme's single-letter code.
fn ext_from_code(code: &str) -> &'static str {
    match code {
        "p" => "png",
        "b" => "bmp",
        "g" => "gif",
        "w" => "webp",
        _ => "jpg",
    }
}

/// Build image URLs out of the theme's `{"<index>":"<code>,<w>,<h>"}` object.
pub fn images_from_json_object(value: &Value, images_uri: &str) -> Vec<String> {
    let mut entries: Vec<(i64, String)> = Vec::new();
    if let Some(object) = value.as_object() {
        for (key, entry) in object {
            let Ok(index) = key.parse::<i64>() else {
                continue;
            };
            let text = match entry {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let code = text.split(',').next().unwrap_or("").trim_matches('"');
            entries.push((index, format!("{images_uri}/{index}.{}", ext_from_code(code))));
        }
    }
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, url)| url).collect()
}

/// Reader pages JSON (`[{"page":1,"ext":"webp",...}, ...]`) used by the newer
/// gallery sites; the image URL is `<images_uri>/<page>.<ext>`.
pub fn images_from_reader_json(value: &Value, images_uri: &str) -> Vec<String> {
    let mut entries: Vec<(i64, String)> = Vec::new();
    if let Some(array) = value.as_array() {
        for entry in array {
            let Some(page) = entry.get("page").and_then(Value::as_i64) else {
                continue;
            };
            let ext = entry.get("ext").and_then(Value::as_str).unwrap_or("jpg");
            entries.push((page, format!("{images_uri}/{page}.{ext}")));
        }
    }
    entries.sort_by_key(|(page, _)| *page);
    entries.into_iter().map(|(_, url)| url).collect()
}

/// Reader page JSON stored inside `<script id="readerPagesJson">`.
pub fn reader_pages_json(html: &Html) -> Option<Value> {
    let script = first(html, "script#readerPagesJson")?;
    let data = script.text().collect::<String>();
    serde_json::from_str(data.trim()).ok()
}

/// `img#gimg, img#fimg, img#readerImg` image source used by the reader pages.
pub fn reader_image_url(html: &Html, base: &str) -> Option<String> {
    for pattern in ["img#gimg", "img#fimg", "img#readerImg"] {
        if let Some(image) = first(html, pattern) {
            if let Some(url) = img_attr(&image, base) {
                return Some(url);
            }
        }
    }
    None
}

/// Info metadata rows: `(label, values)` pairs from the site-specific detail
/// markup, mirroring each site's `getInfoSelector(tag)` override.
pub fn metadata(site: &Site, html: &Html) -> Vec<(String, Vec<String>)> {
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    let mut push = |label: String, values: Vec<String>| {
        let label = label.trim().trim_end_matches(':').trim().to_string();
        let values: Vec<String> = values
            .into_iter()
            .map(|value| value.trim().trim_end_matches(':').trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        if !label.is_empty() && !values.is_empty() {
            rows.push((label, values));
        }
    };

    match site.slug.as_str() {
        "asmhentai" => {
            for container in select(html, ".tags") {
                let label = first_in(&container, "h3").map(text_of).unwrap_or_default();
                let values = select_in(&container, ".tag_list a")
                    .into_iter()
                    .map(|anchor| {
                        // `<span class="badge tag">name <span class=
                        // "gallery_count">(count)</span></span>`
                        first_in(&anchor, ".tag")
                            .map(own_text)
                            .unwrap_or_else(|| own_text(anchor))
                    })
                    .collect();
                push(label, values);
            }
        }
        "hentaienvy" | "hentaizap" => {
            let group_selector = if site.slug == "hentaienvy" {
                "div.hnv-gallery-entity-group"
            } else {
                "div.hz-gallery-entity-group"
            };
            let label_selector = if site.slug == "hentaienvy" {
                ".hnv-gallery-entity-label"
            } else {
                ".hz-gallery-entity-label"
            };
            let value_selector = if site.slug == "hentaienvy" {
                ".hnv-gallery-tag"
            } else {
                ".hz-gallery-tag"
            };
            let name_selector = if site.slug == "hentaienvy" {
                ".hnv-gallery-tag__name"
            } else {
                ".hz-gallery-tag__name"
            };
            for container in select(html, group_selector) {
                let label = first_in(&container, label_selector)
                    .map(text_of)
                    .unwrap_or_default();
                let values = select_in(&container, value_selector)
                    .into_iter()
                    .map(|value| {
                        first_in(&value, name_selector)
                            .map(text_of)
                            .unwrap_or_else(|| own_text(value))
                    })
                    .collect();
                push(label, values);
            }
        }
        "hentaiera" | "imhentai" => {
            for container in select(html, "li") {
                let Some(label_element) = first_in(&container, ".tags_text") else {
                    continue;
                };
                let label = text_of(label_element);
                if site.slug == "hentaiera" {
                    let values = select_in(&container, ".tag .item_name")
                        .into_iter()
                        .map(text_of)
                        .collect();
                    push(label, values);
                } else {
                    // `<a class="tag">name <span class="badge">count</span></a>`
                    let values = select_in(&container, "a.tag")
                        .into_iter()
                        .map(|anchor| own_text(anchor))
                        .collect();
                    push(label, values);
                }
            }
        }
        "hentaifox" => {
            for list in select(html, "ul") {
                let class = list.value().attr("class").unwrap_or("");
                let label = {
                    let from_text = first_in(&list, ".i_text").map(own_text).unwrap_or_default();
                    if !from_text.is_empty() {
                        from_text
                    } else {
                        class.to_string()
                    }
                };
                let values = select_in(&list, "a").into_iter().map(own_text).collect();
                push(label, values);
            }
        }
        "hentainame" => {
            for container in select(html, ".field-name") {
                let label = own_text(container);
                let values = select_in(&container, ".tags a")
                    .into_iter()
                    .map(own_text)
                    .collect();
                push(label, values);
            }
        }
        "hentairox" => {
            for container in select(html, ".hr-gallery-entity-group") {
                let label = first_in(&container, ".hr-gallery-entity-label")
                    .map(text_of)
                    .unwrap_or_default();
                let values = select_in(&container, ".hr-gallery-tag")
                    .into_iter()
                    .map(|value| {
                        first_in(&value, ".hr-gallery-tag__name")
                            .map(text_of)
                            .unwrap_or_else(|| own_text(value))
                    })
                    .collect();
                push(label, values);
            }
        }
        "nhentaito" | "nhentaiwebsite" => {
            let container_selector = if site.slug == "nhentaito" {
                "#info-block .field-name"
            } else {
                "#info .field-name"
            };
            for container in select(html, container_selector) {
                let label = own_text(container);
                let values = select_in(&container, ".tags a")
                    .into_iter()
                    .map(|value| {
                        first_in(&value, ".name")
                            .map(text_of)
                            .unwrap_or_else(|| text_of(value))
                    })
                    .collect();
                push(label, values);
            }
        }
        "nhentaixxx" => {
            for container in select(html, "li.tags") {
                let label = first_in(&container, ".text").map(text_of).unwrap_or_default();
                let values = select_in(&container, "a.tag_btn")
                    .into_iter()
                    .map(|value| {
                        first_in(&value, ".tag_name")
                            .map(text_of)
                            .unwrap_or_else(|| own_text(value))
                    })
                    .collect();
                push(label, values);
            }
        }
        _ => {}
    }
    rows
}

pub fn info_value(rows: &[(String, Vec<String>)], label: &str) -> Option<String> {
    rows.iter()
        .find(|(row_label, _)| row_label.eq_ignore_ascii_case(label))
        .map(|(_, values)| values.join(", "))
}

/// Detail page parsing shared by every site of the theme.
pub fn detail(site: &Site, html: &Html, path: &str) -> MangaInfo {
    let title = text_first(html, "h1")
        .or_else(|| text_first(html, "title"))
        .unwrap_or_else(|| site.name.clone());
    let cover_selector = match site.slug.as_str() {
        "asmhentai" | "hentaifox" | "nhentaixxx" => ".cover img",
        "hentaienvy" => ".hnv-gallery-cover img",
        "hentaiera" | "imhentai" => ".left_cover img",
        "hentainame" | "nhentaito" | "nhentaiwebsite" => "#cover img",
        "hentairox" => ".hr-gallery-cover img",
        "hentaizap" => ".hz-gallery-cover img",
        _ => ".cover img",
    };
    let cover_url =
        first(html, cover_selector).and_then(|image| img_attr(&image, &site.url));

    let rows = metadata(site, html);
    let genre = info_value(&rows, "Tags")
        .map(|value| split_values(&value))
        .unwrap_or_default();
    let author = info_value(&rows, "Artists")
        .or_else(|| info_value(&rows, "Groups"))
        .map(|value| split_values(&value))
        .unwrap_or_default();

    let alternative_title = text_first(html, "h1 + h2")
        .or_else(|| text_first(html, ".subtitle"));
    let pages = match site.slug.as_str() {
        "asmhentai" => text_first(html, ".book_page .pages h3")
            .and_then(|text| text.split_once(':').map(|(_, value)| value.trim().to_string())),
        _ => info_value(&rows, "Pages").or_else(|| {
            text_first(html, "#info > div")
                .filter(|text| text.chars().next().is_some_and(|c| c.is_ascii_digit()))
        }),
    };

    let mut description_parts: Vec<String> = Vec::new();
    for label in [
        "Parodies",
        "Characters",
        "Groups",
        "Languages",
        "Categories",
        "Category",
    ] {
        if let Some(value) = info_value(&rows, label) {
            description_parts.push(format!("**{label}**: {value}"));
        }
    }
    if let Some(pages) = pages.filter(|pages| !pages.is_empty()) {
        description_parts.push(format!("**Pages**: {pages}"));
    }
    if let Some(alternative) = alternative_title.filter(|value| !value.is_empty()) {
        description_parts.push(format!("**Alternative title**: {alternative}"));
    }

    MangaInfo {
        source_id: site.id,
        title,
        author,
        genre,
        status: Some("Completed".to_string()),
        description: if description_parts.is_empty() {
            None
        } else {
            Some(description_parts.join("\n\n"))
        },
        path: path.to_string(),
        cover_url: cover_url.unwrap_or_default(),
    }
}

fn split_values(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Upload timestamp of the single chapter, using each site's override.
pub fn chapter_uploaded(site: &Site, html: &Html) -> i64 {
    let raw = match site.slug.as_str() {
        "hentaifox" => first(html, ".pages")
            .map(own_text)
            .filter(|text| text.starts_with("Posted:"))
            .and_then(|text| text.strip_prefix("Posted:").map(|value| value.trim().to_string())),
        "hentainame" => {
            let text = text_first(html, "#info time");
            return text
                .and_then(|text| parse_date_to_epoch(&text))
                .unwrap_or(0);
        }
        "nhentaixxx" => text_first(html, ".uploaded"),
        "nhentaito" => attr_first(html, ".tags time", "datetime"),
        "nhentaiwebsite" => attr_first(html, "#info time[datetime]", "datetime"),
        "asmhentai" | "hentaienvy" | "hentaiera" | "hentaizap" | "imhentai" | "hentairox" => {
            text_first(html, ".uploaded")
        }
        _ => text_first(html, "time"),
    };
    raw.and_then(|text| parse_date_to_epoch(&text))
        .unwrap_or(0)
}

/// A single-chapter gallery: the chapter path is the gallery detail path.
pub fn chapter(site: &Site, path: &str, html: &Html) -> Vec<tanoshi_lib::prelude::ChapterInfo> {
    let rows = metadata(site, html);
    let scanlator = info_value(&rows, "Groups");
    vec![tanoshi_lib::prelude::ChapterInfo {
        source_id: site.id,
        title: "Chapter".to_string(),
        path: path.to_string(),
        number: 1.0,
        scanlator,
        uploaded: chapter_uploaded(site, html),
    }]
}

/* ------------------------------ listings ------------------------------ */

/// Listing card selectors, mirroring the site overrides of
/// `popularMangaSelector` / `mangaTitle` / `mangaUrl` / `mangaThumbnail`.
fn card_spec(site: &Site) -> (&'static str, &'static str, &'static str, &'static str) {
    match site.slug.as_str() {
        "asmhentai" => (".preview_item", ".caption", ".image a", ".image img"),
        "hentaienvy" => (
            ".hnv-gallery-card",
            ".hnv-gallery-card__title",
            "a.hnv-gallery-card__cover",
            "a.hnv-gallery-card__cover img",
        ),
        "hentaiera" => ("div.thumb", ".gallery_title", ".inner_thumb a", ".inner_thumb img"),
        "hentaifox" => ("div.thumb", ".caption", ".inner_thumb a", ".inner_thumb img"),
        "hentainame" => ("div.gallery", ".caption", "a.cover", "img"),
        "hentairox" => (
            "article.hr-gallery-card",
            ".hr-gallery-card__title",
            ".hr-gallery-card__cover",
            ".hr-gallery-card__media img",
        ),
        "hentaizap" => (
            ".hz-gallery-card",
            ".hz-gallery-card__title a",
            ".hz-gallery-card__cover",
            ".hz-gallery-card__media.thumb img",
        ),
        "imhentai" => ("div.thumb", ".caption", ".inner_thumb a", ".inner_thumb img"),
        "nhentaito" => (".gallery", ".caption", "a.cover", "img"),
        "nhentaiwebsite" => (".gallery", ".caption", "a.cover", "img"),
        "nhentaixxx" => (".gallery_item, .fav_item", ".caption", "a", "img"),
        _ => (".thumb", ".caption", "a", "img"),
    }
}

/// Path stored in [`MangaInfo::path`] (site-relative, like the Kotlin
/// `setUrlWithoutDomain` does).
fn card_path(site: &Site, href: &str) -> String {
    let path = path_of(&site.url, href);
    if site.slug == "nhentaiwebsite" {
        path.replace("/read/", "/g/")
    } else {
        path
    }
}

pub fn listing(site: &Site, html: &Html) -> Vec<MangaInfo> {
    let (card_selector, title_selector, url_selector, thumb_selector) = card_spec(site);
    let mut results: Vec<MangaInfo> = Vec::new();
    for card in select(html, card_selector) {
        let Some(title) = first_in(&card, title_selector).map(text_of).filter(|t| !t.is_empty())
        else {
            continue;
        };
        let Some(href) = first_in(&card, url_selector).and_then(|a| a.value().attr("href")) else {
            continue;
        };
        let cover_url = image_in(card, thumb_selector, &site.url).unwrap_or_default();
        results.push(MangaInfo {
            source_id: site.id,
            title,
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path: card_path(site, href),
            cover_url,
        });
    }
    results
}

/// Search listing plus the theme's language filter.
pub fn search_listing(site: &Site, html: &Html) -> Vec<MangaInfo> {
    let mangalang = crate::mangalang(site);
    let (card_selector, title_selector, url_selector, thumb_selector) = card_spec(site);
    let mut results: Vec<(MangaInfo, Option<String>)> = Vec::new();
    for card in select(html, card_selector) {
        let Some(title) = first_in(&card, title_selector).map(text_of).filter(|t| !t.is_empty())
        else {
            continue;
        };
        let Some(href) = first_in(&card, url_selector).and_then(|a| a.value().attr("href")) else {
            continue;
        };
        let cover_url = image_in(card, thumb_selector, &site.url).unwrap_or_default();
        let lang = item_lang(site, &card);
        results.push((
            MangaInfo {
                source_id: site.id,
                title,
                author: Vec::new(),
                genre: Vec::new(),
                status: None,
                description: None,
                path: card_path(site, href),
                cover_url,
            },
            lang,
        ));
    }

    if mangalang.is_empty() {
        return results.into_iter().map(|(manga, _)| manga).collect();
    }

    let filtered: Vec<MangaInfo> = results
        .iter()
        .filter(|(_, lang)| match lang {
            Some(lang) => {
                lang == mangalang
                    || lang == "speechless"
                    || lang == "translated" && site.slug == "hentaifox"
            }
            None => true,
        })
        .map(|(manga, _)| MangaInfo {
            source_id: manga.source_id,
            title: manga.title.clone(),
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path: manga.path.clone(),
            cover_url: manga.cover_url.clone(),
        })
        .collect();
    if filtered.is_empty() {
        // The Kotlin theme falls back to the first unfiltered result so search
        // never returns an empty page when the site has matches.
        results
            .into_iter()
            .next()
            .map(|(manga, _)| vec![manga])
            .unwrap_or_default()
    } else {
        filtered
    }
}

/// Per-card language, mirroring the site's `Element.mangaLang()` override.
fn item_lang(site: &Site, card: &ElementRef<'_>) -> Option<String> {
    let lang_from_href = |href: Option<&str>| {
        href.map(|href| {
            href.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        })
    };
    match site.slug.as_str() {
        "asmhentai" => {
            for anchor in select_in(card, "a") {
                if first_in(&anchor, ".flag").is_some() {
                    return lang_from_href(anchor.value().attr("href"));
                }
            }
            Some(String::new())
        }
        "hentaienvy" => lang_from_href(
            first_in(card, ".hnv-gallery-card__flag").and_then(|flag| flag.value().attr("href")),
        ),
        "hentaiera" => {
            for anchor in select_in(card, "a") {
                if first_in(&anchor, ".g_flag").is_some() {
                    if let Some(lang) = lang_from_href(anchor.value().attr("href")) {
                        return Some(lang);
                    }
                }
            }
            for flag in select_in(card, ".g_flag") {
                let class = flag.value().attr("class").unwrap_or("");
                for name in class.split_whitespace() {
                    if let Some(code) = name.strip_prefix("flag-") {
                        return Some(lang_code_name(site, code));
                    }
                }
            }
            None
        }
        "hentaifox" => {
            let attr = card.value().attr("data-languages").unwrap_or("");
            let codes = fox_lang_codes(site);
            let mangalang = crate::mangalang(site);
            let parts: Vec<&str> = attr.split(' ').filter(|part| !part.is_empty()).collect();
            if let Some(code) = codes {
                if parts.contains(&code) {
                    return Some(mangalang.to_string());
                }
            }
            if parts.len() > 1 || parts.first().is_some_and(|part| !part.is_empty()) {
                Some("other".to_string())
            } else {
                Some(mangalang.to_string())
            }
        }
        "hentairox" => lang_from_href(
            first_in(card, ".hr-gallery-card__flag").and_then(|flag| flag.value().attr("href")),
        ),
        "hentaizap" => lang_from_href(
            first_in(card, ".hz-gallery-card__flag").and_then(|flag| flag.value().attr("href")),
        ),
        "imhentai" => {
            for anchor in select_in(card, "a") {
                if first_in(&anchor, ".thumb_flag").is_some() {
                    return lang_from_href(anchor.value().attr("href"));
                }
            }
            None
        }
        "nhentaixxx" => {
            let attr = card.value().attr("data-languages").unwrap_or("");
            let code = match site.lang.as_str() {
                "en" => "1",
                "ja" => "2",
                "zh" => "3",
                _ => "",
            };
            if !code.is_empty() && attr == code {
                Some(crate::mangalang(site).to_string())
            } else {
                Some("other".to_string())
            }
        }
        _ => None,
    }
}

fn fox_lang_codes(site: &Site) -> Option<&'static str> {
    match site.lang.as_str() {
        "en" => Some("1"),
        "ja" => Some("5"),
        "zh" => Some("6"),
        "ko" => Some("11"),
        _ => None,
    }
}

fn lang_code_name(_site: &Site, code: &str) -> String {
    match code {
        "us" | "en" => "english",
        "jp" | "ja" => "japanese",
        "es" => "spanish",
        "fr" => "french",
        "kr" => "korean",
        "de" => "german",
        "ru" => "russian",
        "zh" => "chinese",
        _ => code,
    }
    .to_string()
}

/// Access log helper.
pub fn ensure_non_empty(pages: Vec<String>, description: &str) -> Result<Vec<String>> {
    if pages.is_empty() {
        return Err(anyhow!("no pages found ({description})"));
    }
    Ok(pages)
}
