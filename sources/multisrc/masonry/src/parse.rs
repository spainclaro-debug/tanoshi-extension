//! HTML scraping for the Keiyoushi `masonry` theme (Elite Babes + the six
//! Hunter sites).
//!
//! Markup verified against the live sites:
//! - listing pages render cards as `.list-gallery:not(.static) figure`, with the
//!   detail link (`<a href title>`) and a lazyloaded `<img>` (`srcset`/`data-src`/
//!   `src`); the theme's "Free Sites"/related rails use `.list-gallery.static`
//!   and are excluded;
//! - the paginated feeds (`/gridapi/?content=updates&sort=...&mpage=N`) return a
//!   bare `<li><figure>` fragment with the same card markup;
//! - gallery detail pages carry the title in `<h1>`, studio/model/tag links in
//!   `p.link-btn`, an optional `#content > p` description and the photo set as
//!   `.list-gallery.static a[href^=https://cdn.]` full-size links.

use scraper::{ElementRef, Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::Site;

/// Path prefixes on listing cards that are not galleries.
const NON_GALLERY_PREFIXES: [&str; 3] = ["/tag/", "/model/", "/video/"];

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

/// Lazyloaded image URL: `srcset` first candidate, then data attributes, then src.
fn image_src(img: &ElementRef<'_>) -> Option<String> {
    if let Some(srcset) = attr(img, "srcset").or_else(|| attr(img, "data-srcset")) {
        if let Some(first) = srcset.split(',').next() {
            if let Some(url) = first.split_whitespace().next() {
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
    }
    ["data-src", "data-lazy-src", "data-cfsrc", "src"]
        .iter()
        .filter_map(|name| attr(img, name))
        .find(|value| !value.contains("dflazy") && !value.starts_with("data:"))
}

fn absolute(site: &Site, url: &str) -> String {
    let url = url.trim();
    if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{}/{}", site.base(), url.trim_start_matches('/'))
    }
}

/// Site-relative gallery path from an absolute URL on this site.
///
/// Anything pointing at another host (the network's "Free Sites" rails link to
/// the sibling Hunter sites) is rejected: treating those as gallery paths would
/// turn the static rails into bogus results.
fn gallery_path(site: &Site, href: &str) -> Option<String> {
    let href = href.trim();
    let without_scheme = href
        .split(['?', '#'])
        .next()
        .unwrap_or(href)
        .trim_end_matches('/');
    let base = site.base();
    let relative = without_scheme.strip_prefix(base.as_str())?;
    if relative.is_empty() {
        return None;
    }
    let path = if relative.starts_with('/') {
        relative.to_string()
    } else {
        format!("/{relative}")
    };
    if NON_GALLERY_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return None;
    }
    // Video entries expose a player instead of a photo set, so they would open
    // with zero pages; the theme only filters `<a href*=/video/>`, which misses
    // these slugs.
    let slug = path.rsplit('/').find(|part| !part.is_empty())?;
    if slug.ends_with("-video") || slug.contains("-video-") {
        return None;
    }
    Some(path)
}

/// Parse one page/fragment of a gallery listing.
pub fn listing(site: &Site, document: &Html) -> Vec<MangaInfo> {
    let mut figures: Vec<ElementRef<'_>> = document
        .select(&selector(".list-gallery:not(.static) figure"))
        .collect();
    if figures.is_empty() {
        // The paginated `gridapi` feed returns bare `<li><figure>` fragments.
        figures = document.select(&selector("figure")).collect();
    }

    let mut out: Vec<MangaInfo> = Vec::new();
    for figure in figures {
        // Video posts carry a play badge in the card's label; they open a player
        // instead of a photo set and would have zero pages, so skip them.
        if figure.select(&selector(".icon-play")).next().is_some() {
            continue;
        }
        let Some(link) = figure.select(&selector("a[href]")).next() else {
            continue;
        };
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let Some(path) = gallery_path(site, &href) else {
            continue;
        };
        if out.iter().any(|manga| manga.path == path) {
            continue;
        }

        let cover_url = figure
            .select(&selector("img"))
            .find_map(|img| image_src(&img))
            .map(|src| absolute(site, &src))
            .unwrap_or_default();

        let mut title = attr(&link, "title").unwrap_or_default();
        if title.is_empty() {
            title = figure
                .select(&selector("img"))
                .next()
                .and_then(|img| attr(&img, "alt"))
                .unwrap_or_default();
        }
        if title.is_empty() {
            title = path
                .trim_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("Gallery")
                .replace('-', " ");
        }

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

/// Parse a gallery detail page.
pub fn detail(site: &Site, document: &Html, path: &str) -> MangaInfo {
    let title = document
        .select(&selector("h1"))
        .next()
        .map(|element| text_of(&element))
        .filter(|title| !title.is_empty())
        .or_else(|| {
            document
                .select(&selector("title"))
                .next()
                .map(|element| text_of(&element))
                .map(|title| {
                    // "Kira Rami Nude in Electric Blue at Femjoy Hunter" → drop
                    // the trailing " at <site>" suffix.
                    match title.rsplit_once(" at ") {
                        Some((head, _)) => head.to_string(),
                        None => title,
                    }
                })
        })
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            path.trim_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("Gallery")
                .replace('-', " ")
        });

    let mut author: Vec<String> = Vec::new();
    let mut artist: Vec<String> = Vec::new();
    let mut genre: Vec<String> = Vec::new();
    if let Some(meta) = document.select(&selector("p.link-btn")).next() {
        for link in meta.select(&selector("a[href]")) {
            let name = text_of(&link);
            let href = attr(&link, "href").unwrap_or_default();
            // Skip the "+ All Tags" / "+ Less Tags" expander buttons.
            if name.is_empty() || name.starts_with('+') || href == "#" || href.starts_with("javascript:")
            {
                continue;
            }
            if href.contains("/tag/") {
                if !genre.contains(&name) {
                    genre.push(name);
                }
            } else if href.contains("/model/") {
                if !artist.contains(&name) {
                    artist.push(name);
                }
            } else if author.is_empty() {
                author = vec![name];
            }
        }
    }
    if author.is_empty() && !artist.is_empty() {
        author = artist.clone();
    }

    let description = document
        .select(&selector("#content > p"))
        .find(|element| {
            let classes = element.value().attr("class").unwrap_or_default();
            let text = text_of(element);
            !classes.split_whitespace().any(|class| class == "link-btn")
                && !text.is_empty()
                && !text.to_ascii_lowercase().starts_with("much more at")
        })
        .map(|element| text_of(&element))
        .filter(|text| !text.is_empty());

    let cover_url = document
        .select(&selector(".list-gallery.static img"))
        .next()
        .or_else(|| document.select(&selector("img")).next())
        .and_then(|img| image_src(&img))
        .map(|src| absolute(site, &src))
        .unwrap_or_default();

    MangaInfo {
        source_id: site.id,
        title,
        author,
        genre,
        status: Some("Completed".to_string()),
        description,
        path: path.to_string(),
        cover_url,
    }
}

/// Galleries are single-chapter works, exactly like the theme's
/// `parseChapterList`.
pub fn chapters(site: &Site, path: &str) -> Vec<ChapterInfo> {
    vec![ChapterInfo {
        source_id: site.id,
        title: "Gallery".to_string(),
        path: path.to_string(),
        number: 1.0,
        scanlator: None,
        uploaded: 0,
    }]
}

/// Full-size photo links from a gallery page.
pub fn pages(site: &Site, document: &Html) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for link in document.select(&selector(
        r#".list-gallery.static a[href^="https://cdn."], .list-gallery a[href^="https://cdn."]"#,
    )) {
        let Some(href) = attr(&link, "href") else {
            continue;
        };
        let url = absolute(site, &href);
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}
