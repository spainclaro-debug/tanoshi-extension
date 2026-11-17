//! Parsing for the ZeistManga (Blogger) theme.
//!
//! Selectors and endpoints were verified against the live blogs: the home
//! "PopularPosts" widget, the Blogger JSON feeds (`?alt=json`, label filters),
//! the series page and the chapter reader page.

use chrono::DateTime;
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::SiteConfig;

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

fn image_src(element: &ElementRef<'_>) -> Option<String> {
    ["data-src", "data-lazy-src", "src"]
        .iter()
        .filter_map(|name| attr(element, name))
        .find(|value| !value.starts_with("data:"))
}

fn first_text(document: &Html, css: &str) -> Option<String> {
    document
        .select(&selector(css))
        .next()
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
}

fn normalize_status(value: &str) -> Option<String> {
    let lower = value.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }
    const ONGOING: [&str; 8] = [
        "ongoing",
        "en curso",
        "en emisión",
        "em lançamento",
        "activo",
        "ativo",
        "lançando",
        "مستمر",
    ];
    const COMPLETED: [&str; 5] = ["completed", "completo", "finalizado", "مكتمل", "مكتملة"];
    const HIATUS: [&str; 2] = ["hiatus", "pausado"];
    const CANCELLED: [&str; 5] = [
        "cancelled",
        "dropped",
        "dropado",
        "abandonado",
        "cancelado",
    ];

    if ONGOING.iter().any(|entry| lower.contains(entry)) {
        Some("Ongoing".to_string())
    } else if COMPLETED.iter().any(|entry| lower.contains(entry)) {
        Some("Completed".to_string())
    } else if HIATUS.iter().any(|entry| lower.contains(entry)) {
        Some("Hiatus".to_string())
    } else if CANCELLED.iter().any(|entry| lower.contains(entry)) {
        Some("Cancelled".to_string())
    } else {
        Some(value.trim().to_string())
    }
}

// ============================== Listing ==============================

/// Cards of the home page "PopularPosts" widget.
pub fn popular_mangas(document: &Html, site: &SiteConfig, base: &str) -> Vec<MangaInfo> {
    let card_selector = site
        .popular_selector
        .as_deref()
        .unwrap_or("div.PopularPosts div.grid > figure");
    let title_selector = site.popular_title_selector.as_deref().unwrap_or("figcaption > a");
    let url_selector = site.popular_url_selector.as_deref();

    let mut out: Vec<MangaInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for card in document.select(&selector(card_selector)) {
        let url = match url_selector {
            Some(selection) if !selection.trim().is_empty() => card
                .select(&selector(selection))
                .next()
                .and_then(|link| attr(&link, "href")),
            // Empty selector: the card element is the link itself.
            Some(_) => attr(&card, "href"),
            None => card
                .select(&selector("figcaption > a"))
                .next()
                .and_then(|link| attr(&link, "href")),
        };
        let Some(href) = url else {
            continue;
        };
        let mut title = card
            .select(&selector(title_selector))
            .next()
            .map(|element| text_of(&element))
            .unwrap_or_default();
        if title.is_empty() {
            title = attr(&card, "title").unwrap_or_default();
        }
        if title.is_empty() {
            continue;
        }
        let path = absolute(base, &href);
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        let cover_url = card
            .select(&selector("img"))
            .next()
            .and_then(|image| image_src(&image))
            .map(|src| absolute(base, &src))
            .unwrap_or_default();
        out.push(MangaInfo {
            source_id: site.id,
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

/// Rewrite a Blogger thumbnail to the theme's 600px listing size.
fn cover_size(url: &str) -> String {
    let url = url.to_string();
    if let Some(start) = url.find("/s") {
        if let Some(end) = url[start..].find("-c/") {
            let end = start + end + "-c/".len();
            return format!("{}/w600/{}", &url[..start], &url[end..]);
        }
    }
    if let Some(start) = url.rfind("=s") {
        if url.ends_with("-c") {
            return format!("{}=w600", &url[..start]);
        }
    }
    url
}

fn entry_content_image(entry: &serde_json::Value, base: &str) -> Option<String> {
    let content = entry.get("content")?.get("$t")?.as_str()?;
    let fragment = Html::parse_fragment(content);
    fragment
        .select(&selector("img"))
        .next()
        .and_then(|image| image_src(&image))
        .map(|src| absolute(base, &src))
}

/// Convert a Blogger feed payload to manga entries.
pub fn feed_mangas(feed: &serde_json::Value, base: &str, source_id: i64) -> Vec<MangaInfo> {
    let mut out = Vec::new();
    let Some(entries) = feed.pointer("/feed/entry").and_then(|entries| entries.as_array()) else {
        return out;
    };
    for entry in entries {
        let Some(title) = entry
            .pointer("/title/$t")
            .and_then(|title| title.as_str())
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
        else {
            continue;
        };
        let Some((_, link)) = entry_title_link(entry) else {
            continue;
        };
        let path = link
            .strip_prefix(base.trim_end_matches('/'))
            .unwrap_or(&link)
            .to_string();
        let cover_url = entry
            .pointer("/media$thumbnail/url")
            .and_then(|url| url.as_str())
            .map(cover_size)
            .or_else(|| entry_content_image(entry, base))
            .unwrap_or_default();
        out.push(MangaInfo {
            source_id,
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

// ============================== Detail ==============================

pub fn detail_title(document: &Html, site: &SiteConfig) -> String {
    if let Some(css) = site.detail_title.as_deref() {
        if let Some(title) = first_text(document, css) {
            return title;
        }
    }
    for css in [
        "h1.entry-title",
        "h1.post-title",
        ".post-title a",
        "#post-title",
        "article h1",
        "main h1",
    ] {
        if let Some(title) = first_text(document, css) {
            return title;
        }
    }
    if let Some(title) = document
        .select(&selector("meta[property='og:title']"))
        .next()
        .and_then(|element| attr(&element, "content"))
    {
        return clean_title(&title);
    }
    if let Some(title) = first_text(document, "title") {
        return title;
    }
    site.name.clone()
}

fn parse_detail_status(value: &str) -> Option<String> {
    normalize_status(value)
}

/// Parse a series page.
pub fn manga_detail(document: &Html, site: &SiteConfig, url: &str) -> MangaInfo {
    let base = site.url.trim_end_matches('/');
    let profile_selector = site.detail_selector.as_deref().unwrap_or(".grid.gtc-235fr");
    let profile = document
        .select(&selector(profile_selector))
        .next()
        .or_else(|| document.select(&selector("main")).next())
        .or_else(|| document.select(&selector("body")).next());

    let mut manga = MangaInfo {
        source_id: site.id,
        title: detail_title(document, site),
        author: Vec::new(),
        genre: Vec::new(),
        status: None,
        description: None,
        path: url.to_string(),
        cover_url: String::new(),
    };

    let Some(profile) = profile else {
        return manga;
    };

    let thumb_selector = site.detail_thumb.as_deref().unwrap_or("img");
    manga.cover_url = profile
        .select(&selector(thumb_selector))
        .next()
        .and_then(|image| image_src(&image))
        .map(|src| absolute(base, &src))
        .unwrap_or_default();

    let desc_selector = site.detail_desc.as_deref().unwrap_or("#synopsis");
    let description = profile
        .select(&selector(desc_selector))
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !description.is_empty() {
        manga.description = Some(description);
    } else {
        manga.description = document
            .select(&selector("meta[property='og:description'], meta[name='description']"))
            .next()
            .and_then(|element| attr(&element, "content"))
            .filter(|text| text.len() > 40);
    }

    let alt_selector = site.detail_altname.as_deref().unwrap_or("header > p");
    if let Some(alt) = profile
        .select(&selector(alt_selector))
        .next()
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
    {
        let mut description = manga.description.unwrap_or_default();
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative names: ");
        description.push_str(&alt);
        manga.description = Some(description);
    }

    let genre_selector = site.detail_genres.as_deref().unwrap_or("div.mt-15 > a[rel=tag]");
    for element in profile.select(&selector(genre_selector)) {
        let genre = text_of(&element).trim_matches([',', ' ']).to_string();
        if !genre.is_empty() && !manga.genre.contains(&genre) {
            manga.genre.push(genre);
        }
    }
    if manga.genre.is_empty() {
        for element in document.select(&selector("a[rel=tag][href*='search/label']")) {
            let genre = text_of(&element).trim_matches([',', ' ']).to_string();
            if !genre.is_empty() && !manga.genre.contains(&genre) {
                manga.genre.push(genre);
            }
        }
    }

    let author_selector = site.detail_author.as_deref().unwrap_or("span#author");
    if let Some(author) = profile
        .select(&selector(author_selector))
        .next()
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
    {
        manga.author.push(author);
    }
    let artist_selector = site.detail_artist.as_deref().unwrap_or("span#artist");
    if let Some(artist) = profile
        .select(&selector(artist_selector))
        .next()
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty())
    {
        if !manga.author.contains(&artist) {
            manga.author.push(artist);
        }
    }

    let status_selector = site.detail_status.as_deref().unwrap_or("span[data-status]");
    manga.status = profile
        .select(&selector(status_selector))
        .next()
        .map(|element| text_of(&element))
        .and_then(|value| parse_detail_status(&value));

    // The theme's fallback: walk the info rows for status/author/artist when the
    // dedicated selectors are missing.
    let info_selector = site.detail_info.as_deref().unwrap_or(".y6x11p");
    let info_title_selector = site.detail_info_title.as_deref().unwrap_or("strong");
    let info_desc_selector = site.detail_info_desc.as_deref().unwrap_or("span.dt");
    for info in profile.select(&selector(info_selector)) {
        let info_text = info
            .select(&selector(info_title_selector))
            .next()
            .map(|element| text_of(&element))
            .unwrap_or_default();
        let desc_text = info
            .select(&selector(info_desc_selector))
            .map(|element| text_of(&element))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if desc_text.is_empty() {
            continue;
        }
        if manga.status.is_none()
            && ["Status", "Estado", "الحالة"]
                .iter()
                .any(|label| info_text.contains(label))
        {
            manga.status = parse_detail_status(&desc_text);
        } else if manga.author.is_empty()
            && ["Author", "Autor", "Mangaka", "الكاتب", "Yazar"]
                .iter()
                .any(|label| info_text.contains(label))
        {
            manga.author.push(desc_text);
        } else if (manga.author.len() < 2)
            && ["Artist", "Artista", "الرسام", "Çizer"]
                .iter()
                .any(|label| info_text.contains(label))
        {
            manga.author.push(desc_text);
        }
    }

    if manga.status.is_none() {
        // Status badges across the newer layouts: a `span.capitalize` chip.
        for element in document.select(&selector("span.capitalize")) {
            let value = text_of(&element);
            if let Some(status) = normalize_status(&value) {
                if !value.trim().is_empty() && status != value.trim() {
                    manga.status = Some(status);
                    break;
                }
            }
        }
    }

    manga
}

// ============================== Feed entries ==============================

/// The alternate link of a Blogger feed entry, plus its title.
pub fn entry_title_link(entry: &serde_json::Value) -> Option<(String, String)> {
    let title = entry
        .pointer("/title/$t")
        .and_then(|title| title.as_str())
        .map(|title| title.trim().to_string())?;
    let links = entry.get("link")?.as_array()?;
    let href = links
        .iter()
        .find(|link| link.get("rel").and_then(|rel| rel.as_str()) == Some("alternate"))
        .or_else(|| links.first())?
        .get("href")?
        .as_str()?
        .to_string();
    Some((title, href))
}

pub fn entry_title_text(entry: &serde_json::Value) -> String {
    entry
        .pointer("/title/$t")
        .and_then(|title| title.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn entry_has_label(entry: &serde_json::Value, label: &str) -> bool {
    entry
        .get("category")
        .and_then(|categories| categories.as_array())
        .map(|categories| {
            categories.iter().any(|category| {
                category
                    .get("term")
                    .and_then(|term| term.as_str())
                    .map(|term| term == label)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Chapter date: the theme's `yyyy-MM-dd` format first, then a full ISO instant.
pub fn entry_date(entry: &serde_json::Value, prefer_updated: bool) -> i64 {
    let field = if prefer_updated { "updated" } else { "published" };
    let raw = entry
        .pointer(&format!("/{field}/$t"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            entry
                .pointer("/published/$t")
                .and_then(|value| value.as_str())
        })
        .or_else(|| entry.pointer("/updated/$t").and_then(|value| value.as_str()))
        .unwrap_or("");
    let raw = raw.trim();
    if raw.is_empty() {
        return 0;
    }
    if let Ok(date) = DateTime::parse_from_rfc3339(raw) {
        return date.timestamp_millis();
    }
    if let Some(day) = raw.get(..10) {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|value| value.and_utc().timestamp_millis())
                .unwrap_or(0);
        }
    }
    0
}

/// The chapter's stored path (site-relative when possible).
pub fn chapter_path(link: &str, site: &SiteConfig) -> String {
    link.trim()
        .strip_prefix(site.url.trim_end_matches('/'))
        .unwrap_or(link.trim())
        .to_string()
}

/// Pull a chapter number out of a label such as `Chapter 12.5` or `الفصل 3`.
pub fn chapter_number(label: &str) -> f64 {
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

// ============================== Chapter feed ==============================

fn script_html_with(document: &Html, needle: &str) -> Option<String> {
    document
        .select(&selector("script"))
        .find(|script| script.inner_html().contains(needle))
        .map(|script| script.inner_html())
}


/// Extract the first single/double quoted argument of `<marker>(...)`.
fn quoted_argument(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)? + marker.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix('(').unwrap_or(rest).trim_start();
    let mut characters = rest.chars();
    let quote = characters.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let body = &rest[quote.len_utf8()..];
    let end = body.find(quote)?;
    let value = body[..end].trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Extract `label = '...'`.
fn label_assignment(text: &str) -> Option<String> {
    let start = text.find("label")? + "label".len();
    let rest = &text[start..];
    let equals = rest.find('=')? + 1;
    let rest = rest[equals..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let body = &rest[quote.len_utf8()..];
    let end = body.find(quote)?;
    let value = body[..end].trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// The `#myUL` legacy chapter feed: the script's `src` embeds the feed URL.
pub fn old_feed_url(document: &Html) -> Option<String> {
    let src = document
        .select(&selector("#myUL script[src], script[src]"))
        .filter_map(|script| attr(&script, "src"))
        .find(|src| src.contains("/feeds/posts/"))?;
    let feed = src.split('?').next().unwrap_or(&src);
    let feed = feed.trim_matches(['\'', '"', ' ']);
    if feed.is_empty() {
        None
    } else {
        Some(feed.to_string())
    }
}

/// Label of the newer chapter-grid script (`label = '...'`).
fn new_script_label(document: &Html) -> Option<String> {
    document
        .select(&selector("#latest script"))
        .next()
        .map(|script| script.inner_html())
        .and_then(|html| label_assignment(&html))
        .or_else(|| {
            let html = script_html_with(document, "label =")?;
            label_assignment(&html)
        })
}

/// Label of the current reader script (`clwd.run('...')` / custom variant).
/// A chapter label embedded as a `data-label`/`data-labelchapter` attribute on
/// one of the theme's chapter-list containers.
pub fn data_label(document: &Html) -> Option<String> {
    for css in [
        "#nextPrevJS",
        "#nextPrevJSbottom",
        "#nPL2",
        "#chapterlist",
        ".chapter_get",
        "ul.series-chapterlist",
    ] {
        let Ok(container) = Selector::parse(css) else {
            continue;
        };
        for element in document.select(&container) {
            for name in ["data-label", "data-labelchapter"] {
                if let Some(value) = attr(&element, name) {
                    if !value.is_empty() {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

pub fn script_feed_label(document: &Html, site: &SiteConfig) -> Option<String> {
    if site.chapter_feed_mode == "custom_regex" {
        let needle = site.feed_regex.as_deref().unwrap_or("clwd.run");
        if let Some(script) = site.feed_script_selector.as_deref().and_then(|css| {
            document
                .select(&selector(css))
                .find(|element| element.inner_html().contains(needle))
                .or_else(|| document.select(&selector(css)).next())
                .map(|element| element.inner_html())
        }) {
            return quoted_argument(&script, needle);
        }
        return None;
    }

    document
        .select(&selector("#clwd script"))
        .next()
        .map(|script| script.inner_html())
        .or_else(|| script_html_with(document, "clwd.run"))
        .and_then(|script| quoted_argument(&script, "clwd.run"))
        .or_else(|| new_script_label(document))
        .or_else(|| data_label(document))
}

/// Percent-decode a label and drop the Arabic characters some sites encode into it.
pub fn decode_label(label: &str) -> String {
    let decoded = urlencoding::decode(label)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| label.to_string());
    let stripped: String = decoded
        .chars()
        .filter(|character| !('\u{0600}'..='\u{06FF}').contains(character))
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Label out of `#chapterlist[data-label]` / `.chapter_get[data-labelchapter]`.
pub fn custom_feed_label(document: &Html, site: &SiteConfig) -> Option<String> {
    let (css, name) = (
        site.feed_label_selector.as_deref()?,
        site.feed_label_attr.as_deref()?,
    );
    document
        .select(&selector(css))
        .next()
        .and_then(|element| attr(&element, name))
}

// ============================== Chapters (page mode) ==============================

/// Chapters taken from a series-page list (sites without a chapter feed).
pub fn series_list_chapters(document: &Html, site: &SiteConfig, base: &str) -> Vec<ChapterInfo> {
    let Some(css) = site.chapters_selector.as_deref() else {
        return Vec::new();
    };
    let mut out: Vec<ChapterInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for row in document.select(&selector(css)) {
        let title = row
            .select(&selector("span"))
            .next()
            .map(|element| text_of(&element))
            .unwrap_or_default();
        let href = row
            .select(&selector("a[href]"))
            .next()
            .and_then(|link| attr(&link, "href"));
        let Some(href) = href else {
            continue;
        };
        if title.is_empty() {
            continue;
        }
        let path = absolute(base, &href);
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        out.push(ChapterInfo {
            source_id: site.id,
            number: chapter_number(&title),
            title,
            path,
            scanlator: None,
            uploaded: 0,
        });
    }
    out
}

// ============================== Pages ==============================

/// Rewrite a Blogger image URL to its original (`/s0/`) size.
fn full_size(url: &str) -> String {
    let Some(last_slash) = url.rfind('/') else {
        return url.to_string();
    };
    let (head, file) = url.split_at(last_slash + 1);
    let Some(previous_slash) = head.trim_end_matches('/').rfind('/') else {
        return url.to_string();
    };
    let segment = &head[previous_slash + 1..head.trim_end_matches('/').len()];
    let mut characters = segment.chars();
    let first = characters.next().unwrap_or(' ');
    let rest = characters.as_str();
    if matches!(first, 's' | 'w' | 'h')
        && !rest.is_empty()
        && rest
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return format!("{}s0/{}", &head[..=previous_slash], file);
    }
    url.to_string()
}

fn collect_images(document: &Html, css: &str, base: &str, filter_out: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for element in document.select(&selector(css)) {
        if element.value().name() == "img" {
            if let Some(src) = image_src(&element) {
                let url = absolute(base, &src);
                if !out.contains(&url) {
                    out.push(url);
                }
            }
            continue;
        }
        for image in element.select(&selector("img")) {
            if let Some(src) = image_src(&image) {
                if let Some(filter) = filter_out {
                    if src.contains(filter) {
                        continue;
                    }
                }
                let url = absolute(base, &src);
                if !out.contains(&url) {
                    out.push(url);
                }
            }
        }
    }
    out
}

/// Extract page image URLs from a chapter reader.
pub fn chapter_pages(document: &Html, site: &SiteConfig, base: &str) -> Vec<String> {
    let page_selector = site
        .pages_selector
        .as_deref()
        .unwrap_or("div.check-box div.separator");
    let exclude = site.pages_filter_exclude.as_deref();

    match site.pages_mode.as_str() {
        "textarea" | "raw_text" => {
            let source_selector = site
                .pages_source_selector
                .as_deref()
                .unwrap_or("textarea#zeist-raw-data");
            if let Some(source) = document.select(&selector(source_selector)).next() {
                let raw = if source.value().name() == "textarea" {
                    source.text().collect::<Vec<_>>().join("")
                } else {
                    source.inner_html()
                };
                let fragment = Html::parse_fragment(&raw);
                return collect_images(&fragment, page_selector, base, exclude);
            }
            Vec::new()
        }
        "script_image" => {
            let mut out = Vec::new();
            if let Some(script) = script_html_with(document, "chapterImage") {
                let start = script.find("chapterImage").unwrap_or(0);
                let rest = &script[start..];
                let mut characters = rest.char_indices().peekable();
                while let Some((index, character)) = characters.next() {
                    if character != '"' {
                        continue;
                    }
                    let body = &rest[index + 1..];
                    let end = match body.find('"') {
                        Some(end) => end,
                        None => break,
                    };
                    let candidate = &body[..end];
                    if candidate.starts_with("http") {
                        let url = candidate.to_string();
                        if !out.contains(&url) {
                            out.push(url);
                        }
                    }
                    for _ in 0..end {
                        characters.next();
                    }
                }
            }
            out
        }
        "script_json" => {
            let mut out = Vec::new();
            if let Some(script) = script_html_with(document, "=[") {
                if let Some(start) = script.find("=[") {
                    let rest = &script[start + 1..];
                    if let Some(end) = rest.find(']') {
                        let json = &rest[..=end];
                        if let Ok(urls) = serde_json::from_str::<Vec<String>>(json) {
                            for url in urls {
                                if !url.trim().is_empty() {
                                    out.push(url);
                                }
                            }
                        }
                    }
                }
            }
            out
        }
        "html_fullsize" => collect_images(document, page_selector, base, exclude)
            .into_iter()
            .map(|url| full_size(&url))
            .collect(),
        _ => {
            let mut pages = collect_images(document, page_selector, base, exclude);
            if pages.is_empty() {
                // Newer blog layouts moved the reader markup; fall back to the
                // containers every generation shares.
                for fallback in [
                    "#reader .separator",
                    "#reader div.separator",
                    "div.i_img .separator",
                    "#pages_panel img",
                    "div#pages img",
                    "article .separator",
                ] {
                    pages = collect_images(document, fallback, base, exclude);
                    if !pages.is_empty() {
                        break;
                    }
                }
            }
            pages
        }
    }
}

/// Entry labels of a Blogger feed entry.
pub fn entry_labels(entry: &serde_json::Value) -> Vec<String> {
    entry
        .get("category")
        .and_then(|categories| categories.as_array())
        .map(|categories| {
            categories
                .iter()
                .filter_map(|category| {
                    category
                        .get("term")
                        .and_then(|term| term.as_str())
                        .map(|term| term.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Strip the blog-name suffix Blogger appends to page titles.
fn clean_title(title: &str) -> String {
    let title = title.trim();
    for separator in [" | ", " — "] {
        if let Some((head, _)) = title.split_once(separator) {
            let head = head.trim();
            if !head.is_empty() {
                return head.to_string();
            }
        }
    }
    title.to_string()
}

/// Pick the series label for a chapter feed out of a post's labels.
///
/// Chapter posts carry the series label plus the chapter category; the series
/// post itself carries its own title as a label on most blogs.
pub fn post_label(title: &str, labels: &[String], chapter_category: &str) -> Option<String> {
    let title = clean_title(title);
    if let Some(matched) = labels
        .iter()
        .find(|label| label.trim().eq_ignore_ascii_case(title.trim()))
    {
        return Some(matched.clone());
    }
    const GENERIC: [&str; 16] = [
        "Series",
        "Manga",
        "Manhwa",
        "Manhua",
        "Doujinshi",
        "Novel",
        "Novela",
        "Anime",
        "Project",
        "Projeto",
        "Chapter",
        "Capítulo",
        "Capitulo",
        "Bölüm",
        "فصل",
        "الفصل",
    ];
    labels
        .iter()
        .find(|label| {
            !GENERIC
                .iter()
                .any(|generic| label.eq_ignore_ascii_case(generic))
                && !label.eq_ignore_ascii_case(chapter_category)
        })
        .cloned()
        .or_else(|| {
            if title.is_empty() {
                None
            } else {
                Some(title)
            }
        })
}
