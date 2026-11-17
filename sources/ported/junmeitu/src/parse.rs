//! HTML scraping for Junmeitu.
//!
//! Selectors verified against the live markup: `.pic-list > ul > li` cards on
//! listing pages, the `.title` / `.base-info` / `.picture-details` blocks of a
//! detail page and the `.pages` pager plus `pc_cid` / `pc_id` script of a
//! paginated photo set.

use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::{ID, MAX_GALLERY_PAGES, URL};

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

/// The site lazyloads with `data-original`, falling back to `src`.
fn image_src(element: &ElementRef<'_>) -> Option<String> {
    ["data-original", "data-src", "data-lazy-src", "src"]
        .iter()
        .filter_map(|name| attr(element, name))
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

/// Parse the `.pic-list` cards of a listing page.
pub fn listing(document: &Html) -> Vec<MangaInfo> {
    let mut out: Vec<MangaInfo> = Vec::new();

    for element in document.select(&selector(".pic-list > ul > li")) {
        if let Some(manga) = card(
            &element,
            &selector("a"),
            Some(&selector("p")),
            &selector("img"),
        ) {
            push_unique(&mut out, manga);
        }
    }

    // The "news" section uses a different card layout (no `.pic-list`), so fall
    // back to its `ul.b2_gap > li` posts when the regular parse came up empty.
    if out.is_empty() {
        for element in document.select(&selector("ul.b2_gap > li")) {
            if let Some(manga) = card(&element, &selector("h2 a"), None, &selector("img")) {
                push_unique(&mut out, manga);
            }
        }
    }

    out
}

/// Map one listing card onto a [`MangaInfo`].
fn card(
    element: &ElementRef<'_>,
    link_selector: &Selector,
    title_selector: Option<&Selector>,
    image_selector: &Selector,
) -> Option<MangaInfo> {
    let link = element.select(link_selector).next()?;
    let href = attr(&link, "href")?;
    let path = site_relative(&href);
    if path.is_empty() {
        return None;
    }
    let title = title_selector
        .and_then(|title_selector| element.select(title_selector).next())
        .map(|title| text_of(&title))
        .unwrap_or_else(|| text_of(&link));
    if title.is_empty() {
        return None;
    }

    let cover_url = element
        .select(image_selector)
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

/// Parse a photo-set detail page.
pub fn manga_detail(document: &Html, path: &str) -> MangaInfo {
    let normalized = site_relative(path);

    let title = document
        .select(&selector(".title"))
        .next()
        .or_else(|| document.select(&selector(".news-title")).next())
        .map(|element| text_of(&element))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            normalized
                .rsplit('/')
                .next()
                .unwrap_or(normalized.as_str())
                .trim_end_matches(".html")
                .to_string()
        });

    let cover_url = first_picture(document).unwrap_or_default();

    let mut genre: Vec<String> = Vec::new();
    for tag in document.select(&selector(".relation_tags > a")) {
        let name = text_of(&tag);
        if !name.is_empty() && !genre.contains(&name) {
            genre.push(name);
        }
    }

    let mut description = String::new();
    let info: Vec<String> = document
        .select(&selector(".news-info, .picture-details"))
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
        .collect();
    if !info.is_empty() {
        description.push_str(&info.join(" "));
    }
    description.push('\n');
    if let Some(introduce) = document.select(&selector(".introduce")).next() {
        description.push_str(&text_of(&introduce));
    }
    let description = description.trim();
    let description = (!description.is_empty()).then(|| description.to_string());

    MangaInfo {
        source_id: ID,
        title,
        author: Vec::new(),
        genre,
        status: Some("Completed".to_string()),
        description,
        path: normalized,
        cover_url,
    }
}

/// One `Gallery` chapter per photo set, dated by `.base-info` 日期.
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
    for span in document.select(&selector(".base-info span")) {
        let text = text_of(&span);
        let Some((_, value)) = text.split_once("日期") else {
            continue;
        };
        let value = value.trim_start_matches([':', '：']).trim();
        if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc().timestamp_millis())
                .unwrap_or(0);
        }
    }
    0
}

/// First image of the `.pictures` block (or an inline `.news-body` gallery).
pub fn first_picture(document: &Html) -> Option<String> {
    document
        .select(&selector(".pictures img"))
        .next()
        .or_else(|| document.select(&selector(".news-body img")).next())
        .and_then(|img| image_src(&img))
        .map(|src| absolute(&src))
}

/// Image URLs out of an `<img>` HTML snippet (used for AJAX `pic` payloads).
pub fn first_image_src(html: &str) -> Option<String> {
    if html.trim().is_empty() {
        return None;
    }
    let fragment = Html::parse_fragment(html);
    fragment
        .select(&selector("img"))
        .next()
        .and_then(|img| image_src(&img))
        .map(|src| absolute(&src))
}

pub fn is_ajax_page(url: &str) -> bool {
    url.contains("/ajax_") || url.contains("ajax=1")
}

pub fn looks_like_image(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp", ".avif"]
        .iter()
        .any(|extension| path.ends_with(extension))
}

/// URLs of every page of a photo set.
///
/// Inline galleries (`.news-body`) return their image URLs directly. Paginated
/// sets return one URL per page — AJAX endpoints when the `pc_cid`/`pc_id`
/// script is present, plain `-N.html` pages otherwise — so that the (often 100+)
/// image lookups happen lazily in `get_image_bytes` instead of during
/// `get_pages`.
pub fn pages(document: &Html, base_url: &str) -> Vec<String> {
    if let Some(news_body) = document.select(&selector(".news-body")).next() {
        let mut out: Vec<String> = Vec::new();
        for img in news_body.select(&selector("img")) {
            if let Some(src) = image_src(&img) {
                let src = absolute(&src);
                if !out.contains(&src) {
                    out.push(src);
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    let page_count = page_count(document).clamp(1, MAX_GALLERY_PAGES);

    let trimmed = base_url
        .split(['?', '#'])
        .next()
        .unwrap_or(base_url)
        .trim_end_matches('/');
    let rest = trimmed
        .strip_prefix(URL)
        .unwrap_or(trimmed)
        .trim_start_matches('/');
    let mut segments: Vec<String> = rest
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();
    if segments.is_empty() {
        return Vec::new();
    }

    let category = segments[0].clone();
    let last = segments.last().cloned().unwrap_or_default();
    let slug = slug_of(&last);

    if let Some(script) = find_script(document)
        && let (Some(cid), Some(conid)) =
            (script_var(&script, "pc_cid"), script_var(&script, "pc_id"))
    {
        let last_index = segments.len() - 1;
        segments[0] = format!("ajax_{category}");
        return (1..=page_count)
            .map(|page| {
                let mut path_segments = segments.clone();
                path_segments[last_index] = format!("{slug}-{page}.html");
                format!(
                    "{URL}/{}?ajax=1&catid={cid}&conid={conid}",
                    path_segments.join("/")
                )
            })
            .collect();
    }

    // Photo sets without the AJAX helper paginate as plain `-N.html` pages.
    let last_index = segments.len() - 1;
    (1..=page_count)
        .map(|page| {
            let mut path_segments = segments.clone();
            path_segments[last_index] = if page == 1 {
                format!("{slug}.html")
            } else {
                format!("{slug}-{page}.html")
            };
            format!("{URL}/{}", path_segments.join("/"))
        })
        .collect()
}

/// Highest page number in the `.pages` pager (second-to-last anchor).
fn page_count(document: &Html) -> i64 {
    let Some(pages) = document.select(&selector(".pages")).next() else {
        return 1;
    };
    let links: Vec<ElementRef<'_>> = pages.select(&selector("a")).collect();
    if let Some(second_last) = links
        .len()
        .checked_sub(2)
        .and_then(|index| links.get(index))
        && let Ok(count) = text_of(second_last).trim().parse::<i64>()
    {
        return count;
    }
    links
        .iter()
        .filter_map(|link| text_of(link).trim().parse::<i64>().ok())
        .max()
        .unwrap_or(1)
}

/// Script block that defines `pc_cid` / `pc_id`.
fn find_script(document: &Html) -> Option<String> {
    document.select(&selector("script")).find_map(|script| {
        let data = script.text().collect::<String>();
        data.contains("pc_cid").then_some(data)
    })
}

/// Read a JS assignment such as `pc_cid = 6;` from a script body.
fn script_var(script: &str, name: &str) -> Option<String> {
    let start = script.find(name)?;
    let rest = script[start + name.len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let value: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect();
    (!value.is_empty()).then_some(value)
}

/// `046_hanari_…_126p.html` → `046_hanari_…_126p` (also strips a trailing `-N`).
fn slug_of(last_segment: &str) -> String {
    let slug = last_segment.trim_end_matches(".html");
    if let Some((head, tail)) = slug.rsplit_once('-')
        && !head.is_empty()
        && tail.chars().all(|character| character.is_ascii_digit())
    {
        return head.to_string();
    }
    slug.to_string()
}
