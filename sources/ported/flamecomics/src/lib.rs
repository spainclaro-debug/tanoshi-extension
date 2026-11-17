//! Flame Comics (`https://flamecomics.xyz`) — Rust port of the Keiyoushi
//! Tachiyomi extension.
//!
//! The site is a Next.js app, so everything is read from the JSON routes under
//! `/_next/data/<buildId>/…`. The build id is embedded in the homepage's
//! `__NEXT_DATA__` script and changes on every deploy; it is cached and
//! refreshed once when a data route starts failing.
//!
//! | operation | route |
//! |---|---|
//! | build id | `GET /` → `__NEXT_DATA__.buildId` |
//! | popular | `GET /_next/data/{id}/browse.json` → `pageProps.series`, sorted by `popularityRank` |
//! | latest | `GET /_next/data/{id}/latest.json` → `pageProps.allSeries` |
//! | search | client-side filter of the browse list; the site has no search route (`/search` 404s) |
//! | details + chapters | `GET /_next/data/{id}/series/{seriesId}.json?id={seriesId}` |
//! | pages | `GET /_next/data/{id}/series/{seriesId}/{token}.json?id={seriesId}&token={token}` |
//!
//! Series paths are `/series/<seriesId>`, chapter paths are
//! `/series/<seriesId>/<token>`. Page images and covers live on
//! `cdn.flamecomics.xyz` and need no special headers.

use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use serde::de::DeserializeOwned;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod dto;
use dto::*;

extension_utils::export_extension!(register, FlameComics, NAME);

const ID: i64 = 8531542650987673943;
const NAME: &str = "Flame Comics";
const URL: &str = "https://flamecomics.xyz";
const ICON_URL: &str = "https://flamecomics.xyz/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// All series images (covers and pages) are served from this CDN prefix.
const CDN_IMAGES: &str = "https://cdn.flamecomics.xyz/uploads/images/series";
/// The original throttles to 2 requests per 2 seconds.
const REQUESTS_PER_SECOND: f64 = 1.0;

/// The list routes return the whole catalogue in one response, so only page 1
/// has data (the original reports `hasNextPage = false`).
const FIRST_PAGE: i64 = 1;

pub struct FlameComics {
    client: RateLimitedAgent,
    /// Cache for the Next.js build id; `None` until the homepage is scraped.
    build_id: Mutex<Option<String>>,
}

impl Default for FlameComics {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                Some(REQUESTS_PER_SECOND),
            ),
            build_id: Mutex::new(None),
        }
    }
}

fn data_url(build_id: &str, path: &str) -> String {
    format!("{URL}/_next/data/{build_id}/{path}")
}

/// Scrape the build id from the homepage's `__NEXT_DATA__` script.
fn extract_build_id(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("script#__NEXT_DATA__").expect("static selector");
    let script = document
        .select(&selector)
        .next()
        .ok_or_else(|| anyhow!("missing __NEXT_DATA__ script"))?;
    let json: String = script.text().collect();
    let root: NextRoot = serde_json::from_str(&json).context("parsing __NEXT_DATA__")?;
    let build_id = root.build_id.trim().to_string();
    if build_id.is_empty() {
        bail!("empty buildId in __NEXT_DATA__");
    }
    Ok(build_id)
}

/// Normalize a title or query the way the original does: lowercase and drop
/// every character that is not an ASCII letter, digit or space.
fn normalize_title(value: &str) -> String {
    let normalized: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .flat_map(char::to_lowercase)
        .collect();
    normalized.trim().to_string()
}

/// Strip inline HTML from a synopsis.
///
/// The detail payload wraps each paragraph in `<p>`; join those with blank
/// lines so the reading order is preserved, and fall back to plain text
/// extraction when there are no paragraphs.
fn html_to_text(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let selector = Selector::parse("p").expect("static selector");
    let paragraphs: Vec<String> = document
        .select(&selector)
        .filter_map(extension_utils::description_text)
        .collect();
    if !paragraphs.is_empty() {
        return paragraphs.join("\n\n");
    }
    document
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn status_name(status: Option<&str>) -> Option<String> {
    let status = status?.trim();
    if status.is_empty() {
        return None;
    }
    Some(
        match status.to_ascii_lowercase().as_str() {
            "ongoing" => "Ongoing",
            "completed" => "Completed",
            "dropped" => "Cancelled",
            "cancelled" | "canceled" => "Cancelled",
            "hiatus" => "Hiatus",
            _ => "Unknown",
        }
        .to_string(),
    )
}

/// Parse the chapter number from the API's `"311.00"` strings.
fn parse_chapter_number(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

/// Render a chapter title the way the original does: `Chapter 23 - Subtitle`,
/// or just `Chapter 23` when the site has no subtitle.
fn chapter_title(number: f64, title: Option<&str>) -> String {
    let number_text = if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        number.to_string()
    };
    let base = if number >= 0.0 {
        format!("Chapter {number_text}")
    } else {
        "Chapter".to_string()
    };
    match title.map(str::trim).filter(|title| !title.is_empty()) {
        Some(title) => format!("{base} - {title}"),
        None => base,
    }
}

/// Extract the series id from the stored `/series/<id>` path.
fn series_id_from_path(path: &str) -> Result<i64> {
    path.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .find_map(|segment| segment.trim().parse::<i64>().ok())
        .ok_or_else(|| anyhow!("{NAME}: expected a /series/<id> path, got {path:?}"))
}

/// Split the stored `/series/<seriesId>/<token>` chapter path.
fn chapter_parts(path: &str) -> Result<(i64, String)> {
    let path = path.trim().trim_end_matches('/');
    let mut segments = path.rsplit('/');
    let token = segments
        .next()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow!("{NAME}: expected a /series/<id>/<token> path, got {path:?}"))?;
    let series_id = segments
        .next()
        .and_then(|segment| segment.parse::<i64>().ok())
        .ok_or_else(|| anyhow!("{NAME}: expected a /series/<id>/<token> path, got {path:?}"))?;
    Ok((series_id, token))
}

impl FlameComics {
    /// Cached Next.js build id, scraped from the homepage on first use.
    fn build_id(&self) -> Result<String> {
        {
            let cached = self
                .build_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(build_id) = cached.as_ref() {
                return Ok(build_id.clone());
            }
        }
        self.refresh_build_id()
    }

    /// Scrape the current build id from the homepage and cache it.
    fn refresh_build_id(&self) -> Result<String> {
        log::debug!("{NAME}: GET {URL}/ (build id)");
        let html = self
            .client
            .fetch_text(URL)
            .with_context(|| format!("requesting {URL}/"))?;
        let build_id =
            extract_build_id(&html).with_context(|| format!("{NAME}: reading the build id"))?;
        let mut cached = self
            .build_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cached = Some(build_id.clone());
        Ok(build_id)
    }

    /// Fetch and unwrap a Next.js data route.
    ///
    /// A stale build id makes the route 404 (or return an error page), so on
    /// failure the build id is re-scraped and the route is retried once.
    fn fetch_page<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let build_id = self.build_id()?;
        let url = data_url(&build_id, path);
        log::debug!("{NAME}: GET {url}");
        let first_error = match self.client.fetch_json::<NextData<T>>(&url) {
            Ok(data) => return Ok(data.page_props),
            Err(error) => error.context(format!("requesting {url}")),
        };

        let refreshed = match self.refresh_build_id() {
            Ok(build_id) => build_id,
            Err(_) => return Err(first_error),
        };
        if refreshed == build_id {
            return Err(first_error);
        }

        let url = data_url(&refreshed, path);
        log::debug!("{NAME}: retrying GET {url}");
        self.client
            .fetch_json::<NextData<T>>(&url)
            .map(|data| data.page_props)
            .with_context(|| format!("requesting {url}"))
    }

    fn browse_series(&self) -> Result<Vec<Series>> {
        Ok(self.fetch_page::<BrowseProps>("browse.json")?.series)
    }

    /// `browse.json` ordered by `popularityRank`, the site's popular order.
    fn series_by_popularity(&self) -> Result<Vec<Series>> {
        let mut series = self.browse_series()?;
        series.sort_by_key(|series| series.popularity_rank.unwrap_or(i64::MAX));
        Ok(series)
    }

    fn series_page(&self, series_id: i64) -> Result<SeriesProps> {
        self.fetch_page(&format!("series/{series_id}.json?id={series_id}"))
    }
}

impl Extension for FlameComics {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        if page > FIRST_PAGE {
            return Ok(Vec::new());
        }
        Ok(self
            .series_by_popularity()?
            .into_iter()
            .filter_map(Series::into_list_manga)
            .collect())
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        if page > FIRST_PAGE {
            return Ok(Vec::new());
        }
        let props: LatestProps = self.fetch_page("latest.json")?;
        // Novel entries have no `series_id`; the original skips them too.
        Ok(props
            .all_series
            .into_iter()
            .filter_map(Series::into_list_manga)
            .collect())
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        if page > FIRST_PAGE {
            return Ok(Vec::new());
        }
        let query = query
            .as_deref()
            .map(normalize_title)
            .unwrap_or_default();
        // A blank query is the catalogue itself, in the same order as popular.
        if query.trim().is_empty() {
            return Ok(self
                .series_by_popularity()?
                .into_iter()
                .filter_map(Series::into_list_manga)
                .collect());
        }

        // The original filters titles (and we include alternative titles).
        // The site surfaces some keywords only in synopses, so fall back to
        // those when no title matches rather than returning nothing.
        let (title_matches, rest): (Vec<Series>, Vec<Series>) = self
            .browse_series()?
            .into_iter()
            .partition(|series| series.matches_title(&query));
        let matched = if title_matches.is_empty() {
            rest.into_iter()
                .filter(|series| series.matches_description(&query))
                .collect()
        } else {
            title_matches
        };
        Ok(matched
            .into_iter()
            .filter_map(Series::into_list_manga)
            .collect())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let series_id = series_id_from_path(&path)?;
        self.series_page(series_id)?.series.into_detail_manga()
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let series_id = series_id_from_path(&path)?;
        Ok(self
            .series_page(series_id)?
            .chapters
            .into_iter()
            .filter_map(Chapter::into_chapter)
            .collect())
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let (series_id, token) = chapter_parts(&path)?;
        let encoded_token = urlencoding::encode(&token);
        let props: ChapterProps = self.fetch_page(&format!(
            "series/{series_id}/{encoded_token}.json?id={series_id}&token={encoded_token}"
        ))?;
        if props.chapter.series_id != series_id {
            return Err(anyhow!(
                "{NAME}: chapter page is for series {} but {series_id} was requested",
                props.chapter.series_id
            ));
        }
        props.chapter.into_pages(&token)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn normalizes_titles_like_the_original() {
        assert_eq!(normalize_title("Omniscient Reader's Viewpoint"), "omniscient readers viewpoint");
        assert_eq!(normalize_title("Solo Leveling: Ragnarok"), "solo leveling ragnarok");
    }

    #[test]
    fn splits_stored_paths() {
        assert_eq!(series_id_from_path("/series/2").unwrap(), 2);
        assert_eq!(series_id_from_path("series/2/").unwrap(), 2);
        assert!(series_id_from_path("/series/not-a-number").is_err());
        assert_eq!(
            chapter_parts("/series/2/364db6fd6bef182e").unwrap(),
            (2, "364db6fd6bef182e".to_string())
        );
        assert!(chapter_parts("/series/2").is_err());
    }

    #[test]
    fn renders_chapter_titles() {
        assert_eq!(chapter_title(311.0, Some("Dokja's Fable")), "Chapter 311 - Dokja's Fable");
        assert_eq!(chapter_title(311.0, Some("  ")), "Chapter 311");
        assert_eq!(chapter_title(1.5, None), "Chapter 1.5");
        assert_eq!(chapter_title(-1.0, None), "Chapter");
    }

    #[test]
    fn maps_site_statuses() {
        assert_eq!(status_name(Some("Ongoing")).as_deref(), Some("Ongoing"));
        assert_eq!(status_name(Some("Dropped")).as_deref(), Some("Cancelled"));
        assert_eq!(status_name(Some("Hiatus")).as_deref(), Some("Hiatus"));
        assert_eq!(status_name(Some("Coming Soon")).as_deref(), Some("Unknown"));
        assert_eq!(status_name(None), None);
    }

    #[test]
    fn parses_chapter_numbers() {
        assert_eq!(parse_chapter_number("311.00"), Some(311.0));
        assert_eq!(parse_chapter_number("1.50"), Some(1.5));
        assert_eq!(parse_chapter_number(""), None);
    }
}
