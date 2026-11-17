//! HTML parsing for the shared MangaThemesia WordPress theme.
//!
//! The theme (formerly WPMangaStream / WPMangaReader) has a handful of page
//! shapes: a listing grid of `div.bsx` cards, a series page carrying the
//! metadata in `.infotable`/`.tsinfo`/`.fmed` rows and the chapter list in
//! `#chapterlist`-style containers, and a reader page whose images live either
//! in `div#readerarea` or inside a `ts_reader.run({...})` script.
//!
//! The Jsoup-based Kotlin original relies on several Jsoup-only CSS features
//! (`:contains()`, `:has()`); those are implemented here as explicit tree
//! walks instead so plain `scraper` selectors keep working.

use anyhow::Result;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::Site;

/// Fallback listing cards used by the theme. Sites can override the selector.
pub const DEFAULT_SEARCH_SELECTOR: &str =
    ".utao .uta .imgu, .listupd .bs .bsx, .listo .bs .bsx";
/// Fallback series-wrapper selectors.
pub const DEFAULT_DETAIL_SELECTOR: &str =
    "div.bigcontent, div.animefull, div.main-info, div.postbody";
/// Fallback title selectors.
pub const DEFAULT_TITLE_SELECTOR: &str = ".entry-title, .ts-breadcrumb li:last-child span";
/// Fallback description selectors.
pub const DEFAULT_DESC_SELECTOR: &str = ".desc, .entry-content[itemprop=description]";
/// Fallback genre selectors.
pub const DEFAULT_GENRE_SELECTOR: &str = "div.gnr a, .mgen a, .seriestugenre a";
/// Fallback series cover selectors.
pub const DEFAULT_THUMB_SELECTOR: &str = ".infomanga > div[itemprop=image] img, .thumb img";
/// Fallback reader image selector.
pub const DEFAULT_PAGE_SELECTOR: &str = "div#readerarea img";

/// Alternative name labels used by the theme's different localisations.
const ALT_KEYWORDS: &[&str] = &[
    "alternative",
    "alternatif",
    "الأسماء الثانوية",
    "alt name",
    "other name",
    "alternate name",
    "nombre alternativo",
    "título alternativo",
    "titre alternatif",
    "nome alternativo",
    "別名",
    "别名",
];

const AUTHOR_KEYWORDS: &[&str] = &[
    "author",
    "auteur",
    "autor",
    "autora",
    "المؤلف",
    "mangaka",
    "seniman",
    "pengarang",
    "penulis",
    "yazar",
    "ผู้แต่ง",
    "ผู้เขียน",
    "นักเขียน",
    "scénariste",
    "story",
    "writer",
];

const ARTIST_KEYWORDS: &[&str] = &[
    "artist",
    "artiste",
    "artista",
    "الرسام",
    "الناشر",
    "illüstratör",
    "illüstrator",
    "illustrator",
    "çizer",
    "sanatçı",
    "นักวาด",
    "ศิลปิน",
    "desainer",
    "drawing",
    "menggambar",
];

const TYPE_KEYWORDS: &[&str] = &[
    "type", "tipe", "türü", "النوع", "ประเภท", "พิมพ์", "tipo", "tür",
];

const STATUS_KEYWORDS: &[&str] = &[
    "status",
    "statut",
    "durum",
    "狀態",
    "状态",
    "連載状況",
    "estado",
    "الحالة",
    "حالة العمل",
    "حالة",
    "สถานะ",
    "stato",
    "statüsü",
];

const GENRE_KEYWORDS: &[&str] = &[
    "genre",
    "genres",
    "التصنيف",
    "generos",
    "gêneros",
    "türler",
    "thể loại",
    "category",
    "categorias",
];

const ONGOING: &[&str] = &[
    "مستمرة",
    "en curso",
    "ongoing",
    "on going",
    "new season",
    "mass released",
    "ativo",
    "en cours",
    "en cours de publication",
    "đang tiến hành",
    "em lançamento",
    "онгоінг",
    "publishing",
    "devam ediyor",
    "güncel",
    "em andamento",
    "in corso",
    "berjalan",
    "продолжается",
    "updating",
    "lançando",
    "in arrivo",
    "emision",
    "en emision",
    "مستمر",
    "curso",
    "en marcha",
    "publicandose",
    "publicando",
    "连载中",
    "devam etmekte",
    "連載中",
];

const COMPLETED: &[&str] = &[
    "completed",
    "completo",
    "complété",
    "fini",
    "achevé",
    "terminé",
    "tamamlandı",
    "đã hoàn thành",
    "hoàn thành",
    "مكتملة",
    "завершено",
    "finished",
    "finalizad",
    "completata",
    "one-shot",
    "bitti",
    "tamat",
    "completado",
    "concluído",
    "完結",
    "concluido",
    "已完结",
    "bitmiş",
];

const CANCELLED: &[&str] = &[
    "canceled",
    "cancelled",
    "cancelado",
    "cancellato",
    "cancelados",
    "dropped",
    "discontinued",
    "abandonné",
    "bırakıldı",
];

const HIATUS: &[&str] = &[
    "hiatus",
    "on hold",
    "season end",
    "pausado",
    "en espera",
    "en pause",
    "en attente",
    "hiato",
    "sezon finali",
];

fn selector(value: &str) -> Option<Selector> {
    Selector::parse(value).ok()
}

fn select<'a>(root: &ElementRef<'a>, value: &str) -> Vec<ElementRef<'a>> {
    selector(value)
        .map(|sel| root.select(&sel).collect())
        .unwrap_or_default()
}

fn text(element: &ElementRef<'_>) -> String {
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

/// Lazyload-aware image source: prefer the original/lazy attributes over the
/// placeholder `src`.
pub fn image_src(site: &Site, img: &ElementRef<'_>) -> Option<String> {
    let mut names: Vec<&str> = site.img_attrs.iter().map(String::as_str).collect();
    names.extend_from_slice(&[
        "data-lazy-src",
        "data-src",
        "data-cfsrc",
        "data-lm-orig-src",
        "src",
    ]);

    for name in names {
        if let Some(value) = attr(img, name) {
            if value.starts_with("data:") {
                continue;
            }
            return Some(site.absolute(&value));
        }
    }

    // `...-original-src` style attributes used by a few sites (ManhwaDesu).
    for (name, value) in img.value().attrs() {
        if name.ends_with("original-src") && !value.trim().is_empty() {
            return Some(site.absolute(value.trim()));
        }
    }

    None
}

/// Parse one listing page into manga cards.
pub fn manga_cards(site: &Site, doc: &Html, selector: &str) -> Vec<MangaInfo> {
    manga_cards_in(site, &doc.root_element(), selector)
}

/// Parse the manga cards under an arbitrary container.
pub fn manga_cards_in(site: &Site, root: &ElementRef<'_>, selector: &str) -> Vec<MangaInfo> {
    let cards = select(root, selector);
    let link_selector = Selector::parse("a").expect("static selector");
    let img_selector = Selector::parse("img").expect("static selector");
    let title_selector =
        Selector::parse(".tt, .bigor .tt, h3 a, .lch a").expect("static selector");
    let mut out: Vec<MangaInfo> = Vec::new();

    for card in cards {
        let Some(link) = card.select(&link_selector).next() else {
            continue;
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let Some(path) = site.stored_identity(&href) else {
            continue;
        };

        let mut title = attr(&link, "title")
            .or_else(|| card.select(&title_selector).next().map(|el| text(&el)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| collapse(&link.text().collect::<Vec<_>>().join(" ")));
        if title.is_empty() {
            // Some cards only carry the name in the cover's `alt` attribute.
            let alt = card
                .select(&img_selector)
                .find_map(|img| attr(&img, "alt"))
                .unwrap_or_default();
            title = alt;
        }
        let title = if site.strip_paren_prefix {
            strip_paren_prefix(&title)
        } else {
            collapse(&title)
        };

        let cover_url = card
            .select(&img_selector)
            .find_map(|img| image_src(site, &img))
            .map(|value| ascii_url(&value))
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

fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove a leading `(Language)` marker such as `( Português )` from a title.
fn strip_paren_prefix(value: &str) -> String {
    let value = collapse(value);
    if let Some(close) = value.find(')') {
        if close < 34 && value.starts_with('(') {
            let rest = value[close + 1..].trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    value
}

/// Percent-encode non-ASCII characters in an asset URL so the stored value is
/// a plain ASCII URL (some hosts return UTF-8 paths, which downstream clients
/// handle inconsistently).
pub fn ascii_url(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let mut buffer = [0u8; 4];
            for byte in ch.encode_utf8(&mut buffer).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// A metadata row: free-form text plus any links it holds (genres, type, ...).
#[derive(Clone, Default)]
struct Info {
    text: String,
    links: Vec<String>,
}

fn strip_label_prefix(value: &str) -> String {
    value
        .trim()
        .trim_start_matches([':', '-', '–', '：'])
        .trim()
        .to_string()
}

/// Collect the label → value pairs from every row shape the theme uses.
fn scan_info_rows(root: &ElementRef<'_>) -> Vec<(String, Info)> {
    let mut rows: Vec<(String, Info)> = Vec::new();

    // `<table class="infotable"><tr><td>Status</td><td>Ongoing</td></tr>`.
    for tr in select(root, "tr") {
        let cells = select(&tr, "td, th");
        if cells.len() < 2 {
            continue;
        }
        let label = collapse(&text(&cells[0])).to_ascii_lowercase();
        if label.is_empty() {
            continue;
        }
        let value_cell = &cells[cells.len() - 1];
        let links = select(value_cell, "a")
            .iter()
            .map(|element| collapse(&text(element)))
            .filter(|value| !value.is_empty())
            .collect();
        rows.push((
            label,
            Info {
                text: collapse(&text(value_cell)),
                links,
            },
        ));
    }

    // `<div class="imptdt"><i class="icon"></i>Status <i>Ongoing</i></div>`.
    for element in select(root, ".imptdt") {
        let icons = select(&element, "i");
        let full = collapse(&text(&element));
        let links: Vec<String> = select(&element, "a")
            .iter()
            .map(|link| collapse(&text(link)))
            .filter(|value| !value.is_empty())
            .collect();
        if icons.len() >= 2 {
            let first = collapse(&text(&icons[0]));
            let last = collapse(&text(&icons[icons.len() - 1]));
            if last.is_empty() {
                continue;
            }
            let label = if first.is_empty() {
                full.replacen(&last, "", 1).to_ascii_lowercase()
            } else {
                first.to_ascii_lowercase()
            };
            rows.push((collapse(&label), Info { text: last, links }));
        } else if icons.len() == 1 {
            let value = collapse(&text(&icons[0]));
            if value.is_empty() {
                continue;
            }
            let label = full.replacen(&value, "", 1).to_ascii_lowercase();
            rows.push((collapse(&label), Info { text: value, links }));
        } else if !full.is_empty() {
            let value = links.first().cloned().unwrap_or_else(|| full.clone());
            let label = full.replacen(&value, "", 1).to_ascii_lowercase();
            rows.push((collapse(&label), Info { text: value, links }));
        }
    }

    // `<div class="fmed"><b>Author</b> <span>...</span></div>` and the
    // `.wd-full` variant used by LectorHentai/MiauScan.
    for element in select(root, ".fmed, .wd-full, .info-label") {
        let bold = select(&element, "b").first().map(|b| collapse(&text(b)));
        let Some(label) = bold.filter(|value| !value.is_empty()) else {
            continue;
        };
        let full = collapse(&text(&element));
        let value = full.replacen(&label, "", 1);
        let links: Vec<String> = select(&element, "a")
            .iter()
            .map(|link| collapse(&text(link)))
            .filter(|value| !value.is_empty())
            .collect();
        let text_value = if links.is_empty() {
            strip_label_prefix(&value)
        } else {
            links.join(", ")
        };
        rows.push((
            label.to_ascii_lowercase(),
            Info {
                text: text_value,
                links,
            },
        ));
    }

    rows
}

fn find_row(rows: &[(String, Info)], keywords: &[&str]) -> Option<Info> {
    rows.iter()
        .find(|(label, _)| keywords.iter().any(|keyword| label.contains(keyword)))
        .map(|(_, info)| info.clone())
}

fn first_text(root: &ElementRef<'_>, selector_list: &str) -> Option<String> {
    select(root, selector_list)
        .first()
        .map(|element| collapse(&text(element)))
        .filter(|value| !value.is_empty())
}

fn first_attr(root: &ElementRef<'_>, selector_list: &str, attr_name: &str) -> Option<String> {
    select(root, selector_list)
        .iter()
        .find_map(|element| attr(element, attr_name))
}

/// Parse a series page into a [`MangaInfo`].
pub fn manga_detail(site: &Site, doc: &Html, path: &str) -> MangaInfo {
    let document = doc.root_element();
    let details = select(
        &document,
        site
            .detail_sel
            .as_deref()
            .unwrap_or(DEFAULT_DETAIL_SELECTOR),
    )
    .into_iter()
    .next()
    .unwrap_or(document);

    let rows = scan_info_rows(&details);

    let title = site
        .title_sel
        .as_deref()
        .and_then(|sel| first_text(&details, sel))
        .or_else(|| first_text(&details, DEFAULT_TITLE_SELECTOR))
        .or_else(|| first_text(&document, "h1.entry-title, h1, .entry-title"))
        .or_else(|| {
            // Last resort: document <title> without the site suffix.
            let raw = first_text(&document, "title")?;
            let cleaned = raw
                .split(['–', '|'])
                .next()
                .unwrap_or(&raw)
                .trim()
                .to_string();
            (!cleaned.is_empty()).then_some(cleaned)
        })
        .unwrap_or_else(|| {
            path.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .replace('-', " ")
        });
    let title = if site.strip_paren_prefix {
        strip_paren_prefix(&title)
    } else {
        title
    };

    let author = site
        .author_sel
        .as_deref()
        .and_then(|sel| text_or_links(&details, sel))
        .or_else(|| find_row(&rows, AUTHOR_KEYWORDS).map(|info| info.text))
        .map(|value| normalize_person(&value))
        .unwrap_or_default();

    let artist = site
        .artist_sel
        .as_deref()
        .and_then(|sel| text_or_links(&details, sel))
        .or_else(|| find_row(&rows, ARTIST_KEYWORDS).map(|info| info.text))
        .map(|value| normalize_person(&value))
        .unwrap_or_default();

    let mut genre: Vec<String> = site
        .genre_sel
        .as_deref()
        .map(|sel| texts(&details, sel))
        .unwrap_or_else(|| texts(&details, DEFAULT_GENRE_SELECTOR));

    if genre.is_empty() {
        if let Some(info) = find_row(&rows, GENRE_KEYWORDS) {
            genre.extend(info.links);
            if genre.is_empty() && !info.text.is_empty() {
                genre.extend(info.text.split(',').map(str::to_string));
            }
        }
    }
    if genre.is_empty() {
        // Anchor fallback used by sites that link genre archives.
        genre = select(&details, "a[href*=\"/genre\"], a[href*=\"genre=\"]")
            .iter()
            .map(|element| collapse(&text(element)))
            .filter(|value| !value.is_empty())
            .collect();
    }

    let series_type = site
        .type_sel
        .as_deref()
        .and_then(|sel| text_or_links(&details, sel))
        .or_else(|| find_row(&rows, TYPE_KEYWORDS).map(|info| info.text));
    if let Some(series_type) = series_type.filter(|value| !value.is_empty()) {
        genre.push(series_type);
    }

    let mut genre: Vec<String> = genre
        .into_iter()
        .map(|value| collapse(&value))
        .filter(|value| !value.is_empty())
        .map(|value| {
            let lower = value.to_lowercase();
            let mut chars = lower.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => value,
            }
        })
        .collect();
    let mut seen: Vec<String> = Vec::new();
    genre.retain(|value| {
        if seen.contains(value) {
            false
        } else {
            seen.push(value.clone());
            true
        }
    });

    let status = site
        .status_sel
        .as_deref()
        .and_then(|sel| first_text(&details, sel))
        .or_else(|| find_row(&rows, STATUS_KEYWORDS).map(|info| info.text))
        .and_then(|value| parse_status(&value));

    let mut description = site
        .desc_sel
        .as_deref()
        .and_then(|sel| description_of(&details, sel))
        .or_else(|| description_of(&details, DEFAULT_DESC_SELECTOR))
        .or_else(|| {
            description_of(
                &details,
                ".entry-content, div[itemprop=description], .summary__content",
            )
        })
        .filter(|value| !value.is_empty());

    let alt_name = site
        .alt_sel
        .as_deref()
        .and_then(|sel| text_or_links(&details, sel))
        .or_else(|| find_row(&rows, ALT_KEYWORDS).map(|info| info.text))
        .map(|value| collapse(&value))
        .filter(|value| !value.is_empty() && value != &title);

    if let Some(alt) = alt_name {
        let heading = site
            .alt_prefix
            .clone()
            .unwrap_or_else(|| "Alternative Names:".to_string());
        let names = alt
            .split(['|', '/', '•', ',', ';'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("- {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        if !names.is_empty() {
            description = Some(match description {
                Some(existing) => format!("{existing}\n\n{heading}\n{names}"),
                None => format!("{heading}\n{names}"),
            });
        }
    }

    let cover_url = site
        .thumb_sel
        .as_deref()
        .and_then(|sel| first_image(site, &details, sel))
        .or_else(|| first_image(site, &details, DEFAULT_THUMB_SELECTOR))
        .or_else(|| {
            first_image(
                site,
                &details,
                "img[itemprop=image], img.wp-post-image, .seriestucontl img",
            )
        })
        .map(|value| ascii_url(&value))
        .unwrap_or_default();

    let mut people: Vec<String> = Vec::new();
    for person in [author, artist] {
        if !person.is_empty() && !people.contains(&person) {
            people.push(person);
        }
    }

    MangaInfo {
        source_id: site.id,
        title,
        author: people,
        genre,
        status,
        description,
        path: path.to_string(),
        cover_url,
    }
}

fn text_or_links(root: &ElementRef<'_>, selector_list: &str) -> Option<String> {
    let elements = select(root, selector_list);
    if elements.is_empty() {
        return None;
    }
    let joined = elements
        .iter()
        .map(|element| collapse(&text(element)))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    (!joined.is_empty()).then_some(joined)
}

fn texts(root: &ElementRef<'_>, selector_list: &str) -> Vec<String> {
    select(root, selector_list)
        .iter()
        .map(|element| collapse(&text(element)))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_person(value: &str) -> String {
    let value = strip_label_prefix(value);
    match value.to_ascii_lowercase().as_str() {
        "" | "-" | "n/a" | "unknown" | "updating" => String::new(),
        _ => value,
    }
}

fn description_of(root: &ElementRef<'_>, selector_list: &str) -> Option<String> {
    select(root, selector_list)
        .first()
        .and_then(|element| extension_utils::description_text(*element))
}

fn first_image(site: &Site, root: &ElementRef<'_>, selector_list: &str) -> Option<String> {
    select(root, selector_list)
        .iter()
        .find_map(|img| image_src(site, img))
}

fn parse_status(value: &str) -> Option<String> {
    let lower = value.to_lowercase();
    if ONGOING.iter().any(|keyword| lower.contains(keyword)) {
        Some("Ongoing".to_string())
    } else if COMPLETED.iter().any(|keyword| lower.contains(keyword)) {
        Some("Completed".to_string())
    } else if CANCELLED.iter().any(|keyword| lower.contains(keyword)) {
        Some("Cancelled".to_string())
    } else if HIATUS.iter().any(|keyword| lower.contains(keyword)) {
        Some("Hiatus".to_string())
    } else {
        None
    }
}

/// The chapter `li` elements of a series page.
fn chapter_elements<'a>(site: &Site, doc: &'a Html) -> Vec<ElementRef<'a>> {
    let root: ElementRef<'a> = doc.root_element();

    if let Some(selector_list) = site.chapter_sel.as_deref() {
        let explicit = select(&root, selector_list);
        if !explicit.is_empty() {
            return explicit;
        }
    }

    for container_selector in [
        "#chapterlist",
        "div.bxcl",
        "div.cl",
        "div.chapter-list",
        "#lrs-native-chapterlist",
        "div.eplister",
        "ul.clstyle",
    ] {
        let containers = select(&root, container_selector);
        if containers.is_empty() {
            continue;
        }
        let mut chapters = Vec::new();
        for container in containers {
            let items = select(&container, "li");
            if items.is_empty() {
                chapters.push(container);
            } else {
                chapters.extend(items);
            }
        }
        if !chapters.is_empty() {
            return chapters;
        }
    }

    // Last resort: any `li` that carries the theme's chapter marker classes.
    let mut chapters = Vec::new();
    for item in select(&root, "li") {
        if !select(&item, ".chbox, .eph-num, .chapternum, .lch").is_empty() {
            chapters.push(item);
        }
    }
    chapters
}

/// Parse a chapter date in any of the formats the theme's sites use.
pub fn parse_uploaded(site: &Site, raw: &str) -> i64 {
    let raw = raw.trim();
    if raw.is_empty() {
        return 0;
    }

    let mut formats: Vec<String> = Vec::new();
    if let Some(pattern) = &site.date_format {
        formats.push(pattern.clone());
    }
    formats.extend(
        [
            "%B %d, %Y",
            "%b %d, %Y",
            "%d %B %Y",
            "%d %b %Y",
            "%B %d %Y",
            "%b %d %Y",
            "%d/%m/%Y",
            "%m/%d/%Y",
            "%d %B, %Y",
            "%d %b, %Y",
            "%B %d, %Y %H:%M",
            "%b %d, %Y %H:%M",
            "%d/%m/%Y %H:%M",
            "%Y-%m-%d",
            "%Y/%m/%d",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M:%S",
            "%d.%m.%Y",
            "%Y.%d.%m",
            "%d. %B %Y",
        ]
        .iter()
        .map(|value| value.to_string()),
    );

    for format in formats {
        if let Ok(parsed) = NaiveDate::parse_from_str(raw, &format) {
            let datetime = parsed.and_hms_opt(0, 0, 0).unwrap_or_default();
            return Utc.from_utc_datetime(&datetime).timestamp();
        }
        if let Ok(parsed) = NaiveDateTime::parse_from_str(raw, &format) {
            return Utc.from_utc_datetime(&parsed).timestamp();
        }
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return parsed.timestamp();
    }
    if let Ok(parsed) = DateTime::parse_from_rfc2822(raw) {
        return parsed.timestamp();
    }
    0
}

/// Parsed chapter list for a series page.
pub fn chapter_list(site: &Site, doc: &Html) -> Vec<ChapterInfo> {
    let root = doc.root_element();
    let mut out: Vec<ChapterInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for element in chapter_elements(site, doc) {
        let Some(link) = select(&element, "a").into_iter().find(|link| {
            attr(link, "href")
                .map(|href| {
                    !href.starts_with('#')
                        && !href.starts_with("javascript:")
                        && !href.contains("{{")
                })
                .unwrap_or(false)
        }) else {
            // Locked/paid chapters can carry no href at all; the original
            // theme marks them with a lock icon, but nothing can be opened.
            continue;
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let Some(path) = site.stored_identity(&href) else {
            continue;
        };
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());

        let mut name = site
            .chapter_name_sel
            .as_deref()
            .and_then(|sel| first_text(&element, sel))
            .or_else(|| {
                first_text(
                    &element,
                    ".lch a, .chapternum, .ch-num, .chapter-number, .lm4-chapter-name, .chapter-name, .chap-title",
                )
            })
            .or_else(|| attr(&link, "title"))
            .unwrap_or_else(|| collapse(&text(&link)))
            .replace('\u{1f512}', "")
            .trim()
            .to_string();
        if name.is_empty() {
            name = format!(
                "Chapter {}",
                chapter_number(&path).map(|n| format!("{n}")).unwrap_or_default()
            );
        }

        let date_raw = site
            .chapter_date_sel
            .as_deref()
            .and_then(|sel| {
                first_text(&element, sel).or_else(|| first_attr(&element, sel, "datetime"))
            })
            .or_else(|| {
                first_text(
                    &element,
                    ".chapterdate, .ch-date, time.chapterdate, .chapter-date",
                )
            })
            .or_else(|| first_attr(&element, "time", "datetime"))
            .unwrap_or_default();
        let mut uploaded = parse_uploaded(site, &date_raw);

        if uploaded == 0 && out.is_empty() {
            // The theme seeds the newest chapter timestamp from "Updated On".
            let updated = first_attr(
                &root,
                ".listinfo time[itemprop=dateModified], .fmed time, span time[itemprop=dateModified]",
                "datetime",
            )
            .or_else(|| first_text(&root, ".fmed time, .listinfo time"));
            if let Some(updated) = updated {
                uploaded = parse_uploaded(site, &updated);
            }
        }

        out.push(ChapterInfo {
            source_id: site.id,
            number: chapter_number(&name)
                .or_else(|| chapter_number(&path))
                .unwrap_or(-1.0),
            title: name,
            path,
            scanlator: None,
            uploaded,
        });
    }

    if site.chapters == "reverse" {
        out.reverse();
    }

    out
}

/// Pull a chapter number out of a label such as `Chapter 12.5` or `Ch. 7`.
pub fn chapter_number(label: &str) -> Option<f64> {
    let lower = label.to_lowercase();
    let trigger = [
        "chapter", "ch.", "ch ", "episode", "ep.", "bolum", "bölüm", "capitulo", "capítulo",
    ]
    .iter()
    .filter_map(|needle| lower.find(needle).map(|index| (index, needle.len())))
    .min_by_key(|(index, _)| *index);

    let search_from = match trigger {
        Some((index, length)) => index + length,
        None => 0,
    };

    let after = &label[search_from.min(label.len())..];
    if let Some(value) = last_number(after) {
        return Some(value);
    }
    last_number(label)
}

fn last_number(value: &str) -> Option<f64> {
    let mut best: Option<f64> = None;
    let mut current = String::new();
    for ch in value.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !current.is_empty() && !current.contains('.')) {
            current.push(ch);
        } else {
            if let Ok(parsed) = current.parse::<f64>() {
                best = Some(parsed);
            }
            current.clear();
        }
    }
    if let Ok(parsed) = current.parse::<f64>() {
        best = Some(parsed);
    }
    best
}

/// Image URLs from a reader page, following the theme's two rendering modes.
pub fn page_list(site: &Site, doc: &Html) -> Result<Vec<String>> {
    let root = doc.root_element();
    let mut out: Vec<String> = Vec::new();

    let selector_list = site
        .page_sel
        .as_deref()
        .unwrap_or(DEFAULT_PAGE_SELECTOR);
    for img in select(&root, selector_list) {
        if let Some(src) = image_src(site, &img) {
            if !src.starts_with("data:") && !out.contains(&src) {
                out.push(src);
            }
        }
    }

    if out.is_empty() {
        out = page_list_from_scripts(site, doc);
    }

    Ok(out)
}

/// Extract the `images` array embedded in a `ts_reader.run({...})` script (or
/// the base64 `data:text/javascript` variant the theme uses on some sites).
pub fn page_list_from_scripts(site: &Site, doc: &Html) -> Vec<String> {
    let root = doc.root_element();
    let mut bodies: Vec<String> = Vec::new();

    for script in select(&root, "script") {
        if let Some(src) = attr(&script, "src") {
            if let Some(encoded) = src.split("base64,").nth(1) {
                if let Some(decoded) = decode_base64(encoded) {
                    bodies.push(decoded);
                }
            }
        }
        let body: String = script.text().collect();
        if !body.trim().is_empty() {
            bodies.push(body);
        }
    }

    for body in &bodies {
        if let Some(images) = scan_images_json(body) {
            if !images.is_empty() {
                return images
                    .into_iter()
                    .map(|url| site.absolute(&unescape_json_slash(&url)))
                    .filter(|url| url.starts_with("http"))
                    .collect();
            }
        }
    }

    Vec::new()
}

fn decode_base64(value: &str) -> Option<String> {
    use base64::Engine;
    let cleaned: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    // URL-safe alphabets appear on some sites; try both engines.
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(cleaned.as_bytes()))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(cleaned.as_bytes()))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

fn unescape_json_slash(value: &str) -> String {
    value.replace("\\/", "/").replace("\\\"", "\"")
}

/// Locate a JSON array of image URLs after an `images`/`imageUrls` key.
fn scan_images_json(body: &str) -> Option<Vec<String>> {
    for key in [
        "\"images\"",
        "imageUrls",
        "image_urls",
        "chapterImages",
        "chapter_images",
        "'images'",
        "images",
    ] {
        let mut search_from = 0usize;
        while let Some(relative) = body[search_from..].find(key) {
            let index = search_from + relative + key.len();
            search_from = index;
            // The theme writes the array after `:` in JSON and after `=` in a
            // JS variable assignment (`var imageUrls = [...]`).
            let rest = body[index..].trim_start();
            let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('='));
            let Some(rest) = rest else {
                continue;
            };
            let rest = rest.trim_start();
            if !rest.starts_with('[') {
                continue;
            }
            if let Some(values) = parse_json_string_array(rest) {
                if !values.is_empty() {
                    return Some(values);
                }
            }
        }
    }
    None
}

/// Minimal parser for a JSON array of strings.
fn parse_json_string_array(value: &str) -> Option<Vec<String>> {
    let mut chars = value.char_indices();
    let (_, first) = chars.next()?;
    if first != '[' {
        return None;
    }

    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut expect_string = true;

    for (_, ch) in chars {
        if in_string {
            if escaped {
                current.push('\\');
                current.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                out.push(current.clone());
                current.clear();
                expect_string = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                if !expect_string {
                    // Nested array or unexpected value; abort this candidate.
                    return None;
                }
            }
            ',' => expect_string = true,
            ']' => return Some(out),
            ' ' | '\n' | '\r' | '\t' => {}
            _ => {
                if expect_string {
                    return None;
                }
            }
        }
    }
    None
}
