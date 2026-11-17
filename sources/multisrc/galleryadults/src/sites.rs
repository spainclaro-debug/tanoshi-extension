//! URL planning and page-list extraction for the `galleryadults` theme.
//!
//! Every rule here mirrors the Kotlin theme (`GalleryAdults.kt` and the
//! per-site overrides in `src/all/<slug>/`), including the quirky pagination
//! used by HentaiFox and Hentai.name.

use anyhow::{Context, Result, anyhow};
use scraper::Html;
use serde_json::Value;

use crate::{Site, parse};

/// Kotlin `LANGUAGE_*` value for this source (empty string == multi/all).
pub fn mangalang(site: &Site) -> &'static str {
    match site.slug.as_str() {
        "asmhentai" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            "zh" => "chinese",
            _ => "",
        },
        "hentaienvy" | "hentaiera" | "hentaizap" | "imhentai" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            "es" => "spanish",
            "fr" => "french",
            "ko" => "korean",
            "de" => "german",
            "ru" => "russian",
            _ => "",
        },
        "hentaifox" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            "zh" => "chinese",
            "ko" => "korean",
            _ => "",
        },
        "hentainame" => "",
        "hentairox" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            _ => "",
        },
        "nhentaito" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            "zh" => "chinese",
            "ko" => "korean",
            _ => "",
        },
        "nhentaiwebsite" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            "zh" => "chinese",
            _ => "",
        },
        "nhentaixxx" => match site.lang.as_str() {
            "en" => "english",
            "ja" => "japanese",
            "zh" => "chinese",
            _ => "",
        },
        _ => "",
    }
}

fn supports_latest(site: &Site) -> bool {
    match site.slug.as_str() {
        "hentainame" | "nhentaiwebsite" => false,
        "asmhentai" | "hentaienvy" | "hentaifox" => !mangalang(site).is_empty(),
        _ => true,
    }
}

pub fn base(site: &Site) -> String {
    site.url.trim_end_matches('/').to_string()
}

fn host(site: &Site) -> String {
    base(site)
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Absolute URL for a site-relative `path` (or pass absolute URLs through).
pub fn absolute(site: &Site, path: &str) -> String {
    let path = path.trim();
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if path.starts_with('/') {
        return format!("{}{}", base(site), path);
    }
    format!("{}/{}", base(site), path)
}

/// The theme's default `addPageUri`.
fn add_page_uri(url: &str, page: i64) -> String {
    let trimmed = url.trim_matches('/');
    if trimmed.contains('?') {
        format!("{trimmed}&page={page}")
    } else {
        format!("{trimmed}/?page={page}")
    }
}

fn fox_add_page_uri(site: &Site, url: &str, page: i64) -> String {
    if url == format!("{}/", base(site)) && page == 2 {
        format!("{url}page/2/")
    } else if url.contains('?') {
        format!("{url}&page={page}")
    } else {
        format!("{url}/pag/{page}/")
    }
}

fn name_add_page_uri(url: &str, page: i64) -> String {
    if url.contains('?') {
        format!("{url}&p={page}")
    } else {
        format!("{url}?p={page}")
    }
}

fn language_url(site: &Site) -> String {
    let lang = mangalang(site);
    if lang.is_empty() {
        format!("{}/", base(site))
    } else {
        format!("{}/language/{lang}/", base(site))
    }
}

pub fn popular_url(site: &Site, page: i64) -> String {
    let page = page.max(1);
    match site.slug.as_str() {
        "hentainame" => name_add_page_uri(&format!("{}/new/", base(site)), page),
        "hentaiera" | "nhentaixxx" if mangalang(site).is_empty() => {
            add_page_uri(&basic_search_url(site, "", true), page)
        }
        "hentairox" if mangalang(site).is_empty() => {
            add_page_uri(&format!("{}/search/?sort=popular", base(site)), page)
        }
        "nhentaito" => add_page_uri(&nhentaito_search_url(site, "", 2), page),
        _ => {
            let mut url = language_url(site);
            if supports_latest(site) {
                url.push_str("popular");
            }
            if site.slug == "hentaifox" {
                fox_add_page_uri(site, &url, page)
            } else {
                add_page_uri(&url, page)
            }
        }
    }
}

pub fn latest_url(site: &Site, page: i64) -> String {
    let page = page.max(1);
    match site.slug.as_str() {
        "hentainame" => name_add_page_uri(&format!("{}/new/", base(site)), page),
        "hentairox" if mangalang(site).is_empty() => {
            add_page_uri(&format!("{}/search/", base(site)), page)
        }
        "nhentaito" => add_page_uri(&nhentaito_search_url(site, "", 1), page),
        _ => {
            let url = language_url(site).trim_end_matches('/').to_string();
            if site.slug == "hentaifox" {
                fox_add_page_uri(site, &url, page)
            } else {
                add_page_uri(&url, page)
            }
        }
    }
}

/// The theme's `buildQueryString`: spaces become `+`, joined by commas.
fn build_query_string(site: &Site, query: &str, include_lang: bool) -> String {
    if query.trim().is_empty() {
        return String::new();
    }
    match site.slug.as_str() {
        "hentaifox" => {
            // joiner is "+", and every run of special characters becomes "+"
            let mut parts: Vec<String> = vec![query.trim().to_string()];
            let lang = mangalang(site);
            if include_lang && !lang.is_empty() {
                parts.push(lang.to_string());
            }
            parts
                .iter()
                .map(|part| part.trim().replace(|c: char| !c.is_alphanumeric() && c != '"', "+"))
                .collect::<Vec<_>>()
                .join("+")
        }
        _ => query
            .trim()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("+"),
    }
}

fn basic_search_url(site: &Site, query: &str, include_lang: bool) -> String {
    let key = match site.slug.as_str() {
        "hentaienvy" | "hentairox" | "nhentaixxx" => "key",
        _ => "q",
    };
    let encoded = urlencoding::encode(&build_query_string(site, query, include_lang)).into_owned();
    format!("{}/search/?{key}={encoded}&sort=popular", base(site))
}

/// Intermediate search used by HentaiEra, HentaiZap and IMHentai.
fn intermediate_search_url(site: &Site, query: &str, multi: bool) -> String {
    let lang = if multi { "" } else { mangalang(site) };
    let mut params: Vec<String> = Vec::new();
    // getSortOrderURIs() with state 0 (Popular).
    params.push("pp=1".to_string());
    params.push("lt=0".to_string());
    params.push("dl=0".to_string());
    params.push("tr=0".to_string());
    // getCategoryURIs(): all checked by default.
    let categories: &[&str] = if site.slug == "hentaiera" {
        &["mg", "dj", "ws", "is", "ac", "gc"]
    } else {
        &["m", "d", "w", "i", "a", "g"]
    };
    for category in categories {
        params.push(format!("{category}=1"));
    }
    // getLanguageURIs(): selected language (or all for LANGUAGE_MULTI).
    for (name, uri) in [
        ("english", "en"),
        ("japanese", "jp"),
        ("spanish", "es"),
        ("french", "fr"),
        ("korean", "kr"),
        ("german", "de"),
        ("russian", "ru"),
    ] {
        let selected = lang.is_empty() || lang == name;
        params.push(format!("{uri}={}", if selected { 1 } else { 0 }));
    }
    let encoded = urlencoding::encode(&build_query_string(site, query, !multi)).into_owned();
    params.push(format!("key={encoded}"));
    format!("{}/search/?{}", base(site), params.join("&"))
}

fn nhentaito_search_url(site: &Site, query: &str, sort_state: usize) -> String {
    nhentaito_search_url_multi(site, query, sort_state, false)
}

fn nhentaito_search_url_multi(site: &Site, query: &str, sort_state: usize, multi: bool) -> String {
    let q = if query.trim().is_empty() {
        "*".to_string()
    } else {
        urlencoding::encode(query.trim()).into_owned()
    };
    let sort = match sort_state {
        1 => "newest",
        2 => "most-favorited",
        _ => "relevance",
    };
    let lang = if multi { "" } else { mangalang(site) };
    format!(
        "{}/search?q={q}&sort={sort}&lang={}",
        base(site),
        urlencoding::encode(lang)
    )
}

pub fn search_url(site: &Site, page: i64, query: &str) -> String {
    search_url_internal(site, page, query, false)
}

/// Search without any language restriction, used as a fallback when a
/// language-filtered search legitimately has no match.
pub fn search_url_multi(site: &Site, page: i64, query: &str) -> String {
    search_url_internal(site, page, query, true)
}

fn search_url_internal(site: &Site, page: i64, query: &str, multi: bool) -> String {
    let page = page.max(1);
    let query = query.trim();
    match site.slug.as_str() {
        "hentainame" => {
            let slug = query.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-");
            name_add_page_uri(&format!("{}/search/{slug}/", base(site)), page)
        }
        "hentaiera" | "hentaizap" | "imhentai" => {
            add_page_uri(&intermediate_search_url(site, query, multi), page)
        }
        "nhentaito" => add_page_uri(&nhentaito_search_url_multi(site, query, 0, multi), page),
        "nhentaiwebsite" => {
            let encoded = urlencoding::encode(query).into_owned();
            add_page_uri(&format!("{}/search?q={encoded}", base(site)), page)
        }
        _ => add_page_uri(&basic_search_url(site, query, !multi), page),
    }
}

/* ------------------------------ page lists ------------------------------ */

fn gallery_id(path: &str) -> String {
    path.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn cover_selector(slug: &str) -> &'static str {
    match slug {
        "asmhentai" | "hentaifox" | "nhentaixxx" => ".cover img",
        "hentaienvy" => ".hnv-gallery-cover img",
        "hentaiera" | "imhentai" => ".left_cover img",
        "hentainame" | "nhentaito" | "nhentaiwebsite" => "#cover img",
        "hentairox" => ".hr-gallery-cover img",
        "hentaizap" => ".hz-gallery-cover img",
        _ => ".cover img",
    }
}

fn cover_host(html: &Html, site: &Site) -> Option<String> {
    let image = parse::first(html, cover_selector(&site.slug))?;
    let url = parse::img_attr(&image, &site.url)?;
    let host = url
        .split("://")
        .nth(1)?
        .split('/')
        .next()?
        .to_string();
    Some(host)
}

/// JSON-object page list used by HentaiEra, HentaiFox, IMHentai and
/// NHentai.xxx: `load_dir`/`load_id`/`load_server` inputs + `$.parseJSON`.
fn json_object_pages(site: &Site, html: &Html, prefix: &str) -> Result<Vec<String>> {
    let raw = parse::parse_json_script(html)
        .ok_or_else(|| anyhow!("{}: page metadata JSON not found", site.slug))?;
    let json = parse::parse_json_object(&raw)
        .ok_or_else(|| anyhow!("{}: page metadata JSON invalid", site.slug))?;
    let load_dir = parse::input_value(html, "load_dir").unwrap_or_default();
    let load_id = parse::input_value(html, "load_id").unwrap_or_default();
    if load_dir.is_empty() || load_id.is_empty() {
        return Err(anyhow!(
            "{}: load_dir/load_id inputs missing",
            site.slug
        ));
    }
    let server = parse::input_value(html, "load_server")
        .filter(|value| !value.trim().is_empty())
        .map(|number| format!("{prefix}{}.{}", number.trim(), host(site)))
        .or_else(|| cover_host(html, site))
        .ok_or_else(|| anyhow!("{}: image server not found", site.slug))?;
    let images_uri = format!("https://{server}/{load_dir}/{load_id}");
    Ok(parse::images_from_json_object(&json, &images_uri))
}

/// NHentai.xxx embeds `{"fl":{"1":"j,..."},"th":...}` and the theme extracts
/// only the `fl` object.
fn nhentaixxx_json(html: &Html) -> Option<Value> {
    for script in parse::select(html, "script") {
        let data = script.text().collect::<String>();
        let Some(rest) = data.split("$.parseJSON('{\"fl\":").nth(1) else {
            continue;
        };
        let Some(inner) = rest.split(",\"th\":").next() else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(inner.trim()) {
            if value.as_object().is_some() {
                return Some(value);
            }
        }
    }
    None
}

fn pages_from_json_object(
    site: &Site,
    html: &Html,
    json: &Value,
    prefix: &str,
) -> Result<Vec<String>> {
    let load_dir = parse::input_value(html, "load_dir").unwrap_or_default();
    let load_id = parse::input_value(html, "load_id").unwrap_or_default();
    if load_dir.is_empty() || load_id.is_empty() {
        return Err(anyhow!("{}: load_dir/load_id inputs missing", site.slug));
    }
    let server = parse::input_value(html, "load_server")
        .filter(|value| !value.trim().is_empty())
        .map(|number| format!("{prefix}{}.{}", number.trim(), host(site)))
        .or_else(|| cover_host(html, site))
        .ok_or_else(|| anyhow!("{}: image server not found", site.slug))?;
    let images_uri = format!("https://{server}/{load_dir}/{load_id}");
    Ok(parse::images_from_json_object(json, &images_uri))
}

/// Reader page list shared by HentaiEnvy, HentaiZap and HentaiRox: the first
/// reader page embeds every page's extension in a JSON script, and the reader
/// `<img>` gives the image directory. Fetching one page is far cheaper than the
/// Kotlin theme's page-by-page resolution.
fn reader_pages(
    site: &Site,
    path: &str,
    get: &dyn Fn(&str) -> Result<Html>,
) -> Result<Vec<String>> {
    let id = gallery_id(path);
    let reader_url = format!("{}/g/{id}/1/", base(site));
    let html = get(&reader_url).with_context(|| format!("{}: reader page", site.slug))?;
    let json = parse::reader_pages_json(&html)
        .or_else(|| parse::parse_json_script(&html).and_then(|raw| serde_json::from_str(&raw).ok()))
        .ok_or_else(|| anyhow!("{}: reader page JSON not found", site.slug))?;
    let image = parse::reader_image_url(&html, &site.url)
        .ok_or_else(|| anyhow!("{}: reader image not found", site.slug))?;
    let images_uri = image
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .ok_or_else(|| anyhow!("{}: reader image directory not found", site.slug))?;
    Ok(parse::images_from_reader_json(&json, &images_uri))
}

/// Total pages from `#info > div` (Hentai.name / NHentai.website).
fn info_total_pages(html: &Html, info_selector: &str) -> i64 {
    for container in parse::select(html, info_selector) {
        for child in parse::select_in(&container, "div") {
            let text = parse::own_text(child);
            let cleaned = text.trim();
            let digits: String = cleaned.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            let rest = cleaned[digits.len()..].trim();
            if rest == "pages" || rest == "page" || rest == "pages:" {
                return digits.parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Cover-directory page lists: every page is `<cover dir>/<n>.webp`.
fn cover_dir_pages(html: &Html, site: &Site, cover_sel: &str, total: i64) -> Result<Vec<String>> {
    if total <= 0 {
        return Err(anyhow!("{}: page count not found", site.slug));
    }
    let image = parse::first(html, cover_sel)
        .ok_or_else(|| anyhow!("{}: cover image not found", site.slug))?;
    let cover = parse::img_attr(&image, &site.url)
        .ok_or_else(|| anyhow!("{}: cover url not found", site.slug))?;
    let images_uri = cover
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .ok_or_else(|| anyhow!("{}: cover directory not found", site.slug))?;
    Ok((1..=total)
        .map(|page| format!("{images_uri}/{page}.webp"))
        .collect())
}

/// Resolve the page list for a chapter path.
pub fn pages(
    site: &Site,
    path: &str,
    get: &dyn Fn(&str) -> Result<Html>,
    post: &dyn Fn(&str, &[(&str, &str)]) -> Result<String>,
) -> Result<Vec<String>> {
    match site.slug.as_str() {
        "asmhentai" => {
            let html = get(&absolute(site, path))?;
            let total = parse::input_value(&html, "t_pages")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let gallery_id = parse::input_value(&html, "load_id").unwrap_or_default();
            let load_dir = parse::input_value(&html, "load_dir").unwrap_or_default();
            let mut urls: Vec<String> = parse::select(&html, ".preview_thumb a img")
                .iter()
                .filter_map(|image| parse::img_attr(image, &site.url))
                .collect();
            if total > 0 && urls.len() < total {
                let token = parse::csrf_token(&html).unwrap_or_default();
                let visible = urls.len().to_string();
                let total_string = total.to_string();
                let mut form: Vec<(&str, &str)> = vec![
                    ("id", gallery_id.as_str()),
                    ("dir", load_dir.as_str()),
                    ("visible_pages", visible.as_str()),
                    ("t_pages", total_string.as_str()),
                    ("type", "2"),
                ];
                if !token.is_empty() {
                    form.push(("_token", token.as_str()));
                }
                let url = format!("{}/inc/thumbs_loader.php", base(site));
                let fragment = post(&url, &form)?;
                let fragment = Html::parse_fragment(&fragment);
                for image in parse::select(&fragment, "a img") {
                    if let Some(url) = parse::img_attr(&image, &site.url) {
                        urls.push(url);
                    }
                }
            }
            Ok(urls.iter().map(|url| parse::thumbnail_to_full(url)).collect())
        }
        "hentaienvy" | "hentaizap" | "hentairox" => reader_pages(site, path, get),
        "hentaiera" | "hentaifox" | "imhentai" => {
            let html = get(&absolute(site, path))?;
            let raw = parse::parse_json_script(&html)
                .ok_or_else(|| anyhow!("{}: page metadata JSON not found", site.slug))?;
            let json = parse::parse_json_object(&raw)
                .ok_or_else(|| anyhow!("{}: page metadata JSON invalid", site.slug))?;
            pages_from_json_object(site, &html, &json, "m")
        }
        "nhentaixxx" => {
            let html = get(&absolute(site, path))?;
            let json = nhentaixxx_json(&html)
                .ok_or_else(|| anyhow!("{}: page metadata JSON not found", site.slug))?;
            pages_from_json_object(site, &html, &json, "i")
        }
        "nhentaito" => {
            let html = get(&absolute(site, path))?;
            let total = parse::info_value(&parse::metadata(site, &html), "Pages")
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let gallery_id = parse::input_value(&html, "gallery_id").unwrap_or_default();
            let load_id = parse::input_value(&html, "load_id").unwrap_or_default();
            let load_dir = parse::input_value(&html, "load_dir").unwrap_or_default();
            let server = parse::input_value(&html, "load_server").unwrap_or_default();
            let token = parse::csrf_token(&html).unwrap_or_default();
            let mut urls: Vec<String> = parse::select(&html, "#thumbnail-container .thumb-container img")
                .iter()
                .filter_map(|image| parse::img_attr(image, &site.url))
                .collect();
            if total > 0 && urls.len() < total && !load_id.is_empty() {
                let visible = urls.len().to_string();
                let total_string = total.to_string();
                let mut form: Vec<(&str, &str)> = vec![
                    ("u_id", gallery_id.as_str()),
                    ("g_id", load_id.as_str()),
                    ("img_dir", load_dir.as_str()),
                    ("visible_pages", visible.as_str()),
                    ("total_pages", total_string.as_str()),
                    ("type", "2"),
                ];
                if !token.is_empty() {
                    form.push(("_token", token.as_str()));
                }
                if !server.is_empty() {
                    form.push(("server", server.as_str()));
                }
                let url = format!("{}/inc/thumbs_loader.php", base(site));
                let fragment = post(&url, &form)?;
                let fragment = Html::parse_fragment(&fragment);
                for image in parse::select(&fragment, "a img") {
                    if let Some(url) = parse::img_attr(&image, &site.url) {
                        urls.push(url);
                    }
                }
            }
            Ok(urls.iter().map(|url| parse::thumbnail_to_full(url)).collect())
        }
        "nhentaiwebsite" => {
            let html = get(&absolute(site, path))?;
            let total = info_total_pages(&html, "#info");
            cover_dir_pages(&html, site, "#cover img", total)
        }
        "hentainame" => {
            let html = get(&absolute(site, path))?;
            let total = info_total_pages(&html, "#info");
            cover_dir_pages(&html, site, "#cover img", total)
        }
        _ => Err(anyhow!("{}: page list not implemented", site.slug)),
    }
}
