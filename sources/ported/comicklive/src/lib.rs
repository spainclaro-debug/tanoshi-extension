//! Comick (`https://comick.live`) — Rust port of the Keiyoushi Tachiyomi
//! extension "Comick (Unoriginal)".
//!
//! The site is a Laravel backend with a few JSON endpoints and server-rendered
//! pages that boot Alpine.js from embedded JSON:
//!
//! | operation | request |
//! |---|---|
//! | popular | `/api/comics/top?day={7,30,90}&type={follow,most_follow_new}` |
//! | latest | `/api/chapters/latest?order=new&page=N&cursor=...` |
//! | search | `/api/search?q=...&order_by=...&order_direction=...&type=comic&showAll=false&exclude_mylist=false&cursor=...` |
//! | detail | `/comic/{slug}` + `#comic-data` JSON |
//! | chapters | `/api/comics/{slug}/chapter-list?lang=en&page=N` |
//! | pages | `/comic/{slug}/{hid}-chapter-{chap}-{lang}` + `#sv-data` JSON |
//!
//! Pagination notes (verified live): the listing endpoints paginate with an
//! opaque `next_cursor`, not an offset — `page=2` without the cursor replays
//! page 1. Since the [`Extension`] API is stateless per call, page N is
//! reached by walking the cursor chain from page 1.
//!
//! Quirk: every `/search` request (both the HTML page and `/api/search`) is
//! answered with a Cloudflare managed challenge for non-browser clients,
//! while the rest of the site is served normally. The Kotlin source declares
//! `https://comick.art` as an official mirror, and that host serves the same
//! JSON without the challenge, so search uses the mirror first and falls back
//! to the primary host. Every other request goes to `comick.live` first.

mod dto;
mod parse;

use anyhow::{Context, Result, anyhow, bail};
use networking::{FetchedDocument, RateLimitedAgent, build_rate_limited_ureq_agent};
use serde::de::DeserializeOwned;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

use dto::*;

extension_utils::export_extension!(register, Comick, NAME);

/// Keiyoushi source id for "Comick (Unoriginal)".
const ID: i64 = 4972933717624256217;
const NAME: &str = "Comick";
const URL: &str = "https://comick.live";
/// Official mirror listed in the Kotlin source's `mirrors(...)` block.
const MIRROR_URL: &str = "https://comick.art";
const ICON_URL: &str = "https://comick.live/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The Keiyoushi source rate-limits itself to 2 requests per second.
const REQUESTS_PER_SECOND: f64 = 2.0;
/// Fallback chapter language: the Kotlin source's default whitelist is "en".
const CHAPTER_LANG: &str = "en";
/// Safety valve for the cursor walks and the chapter pagination loop.
const MAX_LIST_PAGES: i64 = 60;
const MAX_CHAPTER_PAGES: i64 = 60;
/// The site's CDN rejects requests without a same-site referer.
const REFERER: &str = "https://comick.live/";
const SORT_BY: &str = "created_at";
const SORT_DIRECTION: &str = "desc";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

pub struct Comick {
    client: RateLimitedAgent,
}

impl Default for Comick {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), Some(REQUESTS_PER_SECOND)),
        }
    }
}

/// Normalise a manga path to the bare site slug.
///
/// Tanoshi stores whatever `MangaInfo.path` contained; accept a bare slug,
/// `/comic/{slug}`, or a full reader URL so older rows keep working.
fn manga_slug(path: &str) -> String {
    let path = path.trim();
    let path = request_path(path);
    let slug = path.strip_prefix("/comic/").unwrap_or(&path);
    slug.trim_matches('/')
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Reduce any path/URL to a site-relative request path.
fn request_path(path: &str) -> String {
    let path = path.trim();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = path.strip_prefix(scheme) {
            return match rest.find('/') {
                Some(index) => rest[index..].to_string(),
                None => "/".to_string(),
            };
        }
    }
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Turn a possibly-relative URL into an absolute one for the Tanoshi server.
fn absolute_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return url.to_string();
    }
    if let Some(rest) = url.strip_prefix("//") {
        return format!("https://{rest}");
    }
    format!("{URL}/{}", url.trim_start_matches('/'))
}

fn encode_params(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn manga_from_browse(comic: BrowseComic) -> MangaInfo {
    MangaInfo {
        source_id: ID,
        title: comic.title,
        author: Vec::new(),
        genre: Vec::new(),
        status: None,
        description: None,
        path: comic.slug,
        cover_url: comic.default_thumbnail.unwrap_or_default(),
    }
}

fn manga_from_detail(comic: ComicData) -> MangaInfo {
    let mut author = comic
        .authors
        .iter()
        .map(|author| author.name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if author.is_empty() {
        author = comic
            .artists
            .iter()
            .map(|artist| artist.name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
    }

    let description = description_from_detail(&comic);
    let mut genre = Vec::new();
    match comic.country.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("jp") => genre.push("Manga".to_string()),
        Some("cn") => genre.push("Manhua".to_string()),
        Some("ko") => genre.push("Manhwa".to_string()),
        _ => {}
    }
    match comic
        .content_rating
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("suggestive") => genre.push("Content Rating: Suggestive".to_string()),
        Some("erotica") => genre.push("Content Rating: Erotica".to_string()),
        _ => {}
    }
    genre.extend(
        comic
            .genres
            .iter()
            .map(|wrapper| wrapper.md_genres.name.trim().to_string())
            .filter(|name| !name.is_empty()),
    );

    MangaInfo {
        source_id: ID,
        title: comic.title,
        author,
        genre,
        status: status_name(comic.status, comic.translation_completed),
        description,
        path: comic.slug,
        cover_url: comic.default_thumbnail.unwrap_or_default(),
    }
}

fn status_name(status: Option<i64>, translation_completed: bool) -> Option<String> {
    let status = status?;
    Some(
        match status {
            1 => "Ongoing",
            2 if translation_completed => "Completed",
            2 => "Publishing finished",
            3 => "Cancelled",
            4 => "Hiatus",
            _ => "Unknown",
        }
        .to_string(),
    )
}

/// Convert the HTML `desc` field to plain text and append alternative titles
/// the same way the Kotlin extension does.
fn description_from_detail(comic: &ComicData) -> Option<String> {
    let html = comic.desc.as_deref().unwrap_or_default();
    let fragment = scraper::Html::parse_fragment(html);
    let mut description = extension_utils::description_text(fragment.root_element()).unwrap_or_default();

    if let Some(titles) = &comic.titles {
        let titles: Vec<String> = match titles {
            Titles::List(list) => list.iter().map(|title| title.title.clone()).collect(),
            Titles::Map(map) => map.values().map(|title| title.title.clone()).collect(),
        };
        let mut titles: Vec<String> = titles
            .into_iter()
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .collect();
        if !titles.is_empty() {
            titles.sort();
            titles.dedup();
            description.push_str("\n\nAlternative Titles:\n");
            for title in titles {
                description.push_str("- ");
                description.push_str(&title);
                description.push('\n');
            }
        }
    }

    let description = description.trim().to_string();
    (!description.is_empty()).then_some(description)
}

fn timestamp_millis(value: Option<&str>) -> i64 {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.timestamp_millis())
        .unwrap_or(0)
}

fn chapter_from_dto(slug: &str, chapter: ChapterDto) -> ChapterInfo {
    let number_text = chapter.chap.unwrap_or_default();
    let language = chapter.lang;
    let mut title = String::new();
    if let Some(volume) = chapter
        .vol
        .as_deref()
        .map(str::trim)
        .filter(|volume| !volume.is_empty())
    {
        title.push_str("Vol. ");
        title.push_str(volume);
        title.push(' ');
    }
    title.push_str("Ch. ");
    title.push_str(&number_text);
    if let Some(chapter_title) = chapter
        .title
        .as_deref()
        .map(str::trim)
        .filter(|chapter_title| !chapter_title.is_empty())
    {
        title.push_str(": ");
        title.push_str(chapter_title);
    }
    // The source is multi-language; flag non-English chapters so duplicates
    // with the same number stay distinguishable in clients.
    if !language.is_empty() && language != CHAPTER_LANG {
        title.push_str(" [");
        title.push_str(&language);
        title.push(']');
    }

    let scanlator = {
        let groups = chapter
            .group_name
            .unwrap_or_default()
            .iter()
            .map(|group| group.trim().to_string())
            .filter(|group| !group.is_empty())
            .collect::<Vec<_>>();
        (!groups.is_empty()).then(|| groups.join(", "))
    };

    ChapterInfo {
        source_id: ID,
        title,
        path: format!(
            "/comic/{slug}/{}-chapter-{}-{}",
            chapter.hid, number_text, language
        ),
        number: number_text.parse::<f64>().unwrap_or_default(),
        scanlator,
        uploaded: timestamp_millis(chapter.created_at.as_deref()),
    }
}

impl Comick {
    /// Fetch a site-relative path, trying the primary host first (or the
    /// mirror first when `mirror_first` is set) and falling back to the other
    /// host on any HTTP error such as the search route's Cloudflare challenge.
    fn get_document(&self, path: &str, mirror_first: bool) -> Result<FetchedDocument> {
        let path = request_path(path);
        let hosts: [&str; 2] = if mirror_first {
            [MIRROR_URL, URL]
        } else {
            [URL, MIRROR_URL]
        };
        let mut failures: Vec<String> = Vec::new();
        for host in hosts {
            let url = format!("{host}{path}");
            log::debug!("{NAME}: GET {url}");
            match self.client.fetch_document(&url) {
                Ok(document) => return Ok(document),
                Err(error) => {
                    log::debug!("{NAME}: {url} failed ({error:#}); trying next host");
                    failures.push(format!("{url}: {error:#}"));
                }
            }
        }
        bail!(
            "{NAME}: all hosts failed for {path} ({}); the site may be down or blocking this client",
            failures.join(" | ")
        )
    }

    fn get_text(&self, path: &str, mirror_first: bool) -> Result<String> {
        Ok(self.get_document(path, mirror_first)?.body)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str, mirror_first: bool) -> Result<T> {
        let body = self.get_text(path, mirror_first)?;
        serde_json::from_str(&body).with_context(|| format!("decoding JSON from {path}"))
    }

    fn latest_page(&self, page: i64, cursor: Option<&str>) -> Result<CursorResponse> {
        let mut params = vec![
            ("order", "new".to_string()),
            ("page", page.max(1).to_string()),
        ];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor.to_string()));
        }
        self.get_json(
            &format!("/api/chapters/latest?{}", encode_params(&params)),
            false,
        )
    }

    fn search_page(&self, query: Option<&str>, cursor: Option<&str>) -> Result<CursorResponse> {
        let mut params = vec![
            ("order_by", SORT_BY.to_string()),
            ("order_direction", SORT_DIRECTION.to_string()),
            ("type", "comic".to_string()),
            ("showAll", "false".to_string()),
            ("exclude_mylist", "false".to_string()),
        ];
        if let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) {
            params.push(("q", query.to_string()));
        }
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor.to_string()));
        }
        // Search is Cloudflare-challenged on comick.live (see module docs), so
        // the official mirror is tried first.
        self.get_json(&format!("/api/search?{}", encode_params(&params)), true)
    }

    fn fetch_comic_data(&self, path: &str) -> Result<ComicData> {
        let slug = manga_slug(path);
        if slug.is_empty() {
            bail!("{NAME}: empty manga path");
        }
        let document = self.get_document(&format!("/comic/{slug}"), false)?;
        let json = parse::json_element(&document.body, "comic-data").ok_or_else(|| {
            anyhow!(
                "{NAME}: no #comic-data JSON on the page for {slug} (final URL {})",
                document.final_url
            )
        })?;
        serde_json::from_str(&json).with_context(|| format!("decoding comic data for {slug}"))
    }

    /// Fetch every chapter-list page for one slug.
    fn fetch_all_chapters(&self, slug: &str, language: Option<&str>) -> Result<Vec<ChapterDto>> {
        let mut chapters = Vec::new();
        let mut page = 1;
        let mut last_page = 1;

        while page <= last_page && page <= MAX_CHAPTER_PAGES {
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(language) = language {
                params.push(("lang", language.to_string()));
            }
            if page > 1 {
                params.push(("page", page.to_string()));
            }
            let mut path = format!("/api/comics/{slug}/chapter-list");
            if !params.is_empty() {
                path.push('?');
                path.push_str(&encode_params(&params));
            }

            let data: ChapterListResponse = self.get_json(&path, false)?;
            if page == 1 {
                last_page = data.pagination.last_page.max(1);
            }
            if data.data.is_empty() {
                break;
            }
            chapters.extend(data.data);
            page += 1;
        }

        Ok(chapters)
    }

    /// Chapter list preferring English, like the Kotlin source's default
    /// language whitelist; fall back to every language when a comic has no
    /// English scanlations at all.
    fn collect_chapters(&self, slug: &str) -> Result<Vec<ChapterInfo>> {
        let mut chapters = self.fetch_all_chapters(slug, Some(CHAPTER_LANG))?;
        if chapters.is_empty() {
            log::debug!(
                "{NAME}: no {CHAPTER_LANG} chapters for {slug}; falling back to all languages"
            );
            chapters = self.fetch_all_chapters(slug, None)?;
        }
        Ok(chapters
            .into_iter()
            .map(|chapter| chapter_from_dto(slug, chapter))
            .collect())
    }
}

impl Extension for Comick {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            // The source carries every scanlation language.
            languages: Lang::All,
            nsfw: false,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        // Pages 1..6 walk follow ranking over 7/30/90 days, then the
        // most-followed-new ranking over the same windows (see Kotlin).
        let (day, kind) = match page.max(1) {
            1 => (7, "follow"),
            2 => (30, "follow"),
            3 => (90, "follow"),
            4 => (7, "most_follow_new"),
            5 => (30, "most_follow_new"),
            6 => (90, "most_follow_new"),
            _ => return Ok(Vec::new()),
        };
        let data: DataResponse<Vec<BrowseComic>> =
            self.get_json(&format!("/api/comics/top?day={day}&type={kind}"), false)?;
        Ok(data.data.into_iter().map(manga_from_browse).collect())
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        // Cursor pagination: page N requires the cursor chain from page 1.
        let target = page.clamp(1, MAX_LIST_PAGES);
        let mut cursor: Option<String> = None;
        for current in 1..=target {
            let data = self.latest_page(current, cursor.as_deref())?;
            if current == target {
                return Ok(data.data.into_iter().map(manga_from_browse).collect());
            }
            cursor = data.next_cursor;
            if cursor.is_none() {
                return Ok(Vec::new());
            }
        }
        Ok(Vec::new())
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        // A blank query is a valid browse request (the endpoint then returns
        // the default popular ordering). Short queries are rejected by the
        // site (it redirects them to the home page), matching the Kotlin
        // source's explicit "at least 3 characters" error.
        if let Some(query) = query.as_deref().map(str::trim).filter(|query| !query.is_empty())
            && query.chars().count() < 3
        {
            bail!("{NAME}: search query must be at least 3 characters");
        }
        let target = page.clamp(1, MAX_LIST_PAGES);
        let mut cursor: Option<String> = None;
        for current in 1..=target {
            let data = self.search_page(query.as_deref(), cursor.as_deref())?;
            if current == target {
                return Ok(data.data.into_iter().map(manga_from_browse).collect());
            }
            cursor = data.next_cursor;
            if cursor.is_none() {
                return Ok(Vec::new());
            }
        }
        Ok(Vec::new())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let comic = self.fetch_comic_data(&path)?;
        Ok(manga_from_detail(comic))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let slug = manga_slug(&path);
        if slug.is_empty() {
            bail!("{NAME}: empty manga path");
        }

        match self.collect_chapters(&slug) {
            Ok(chapters) if !chapters.is_empty() => Ok(chapters),
            Ok(_) => self.chapters_via_canonical_slug(&slug, Vec::new()),
            Err(error) => {
                log::debug!(
                    "{NAME}: chapter list for {slug} failed ({error:#}); retrying with the canonical slug"
                );
                self.chapters_via_canonical_slug(&slug, Vec::new())
            }
        }
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let request = request_path(&path);
        let document = self.get_document(&request, false)?;

        let json = match parse::json_element(&document.body, "sv-data") {
            Some(json) => json,
            None => {
                // A legacy manga slug redirects chapter URLs to the comic
                // page. Its #comic-data blob carries the canonical slug, so
                // rebuild the chapter URL and retry once.
                let canonical = parse::json_element(&document.body, "comic-data")
                    .and_then(|json| serde_json::from_str::<ComicData>(&json).ok())
                    .map(|comic| comic.slug)
                    .filter(|slug| !slug.is_empty())
                    .ok_or_else(|| {
                        anyhow!(
                            "{NAME}: no #sv-data JSON at {request} (final URL {})",
                            document.final_url
                        )
                    })?;
                let chapter_segment = request.rsplit('/').next().unwrap_or_default();
                let retry = format!("/comic/{canonical}/{chapter_segment}");
                log::debug!("{NAME}: retrying chapter page as {retry}");
                let document = self.get_document(&retry, false)?;
                parse::json_element(&document.body, "sv-data").ok_or_else(|| {
                    anyhow!(
                        "{NAME}: no #sv-data JSON at {retry} (final URL {})",
                        document.final_url
                    )
                })?
            }
        };

        let data: PageListData =
            serde_json::from_str(&json).with_context(|| format!("decoding page list for {path}"))?;
        let pages: Vec<String> = data
            .chapter
            .images
            .into_iter()
            .map(|image| absolute_url(&image.url))
            .filter(|url| !url.is_empty())
            .collect();
        if pages.is_empty() {
            bail!("{NAME}: chapter at {path} has no images");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, REFERER, NAME)
    }
}

impl Comick {
    /// Canonicalise through the detail page when the stored slug is stale.
    fn chapters_via_canonical_slug(
        &self,
        slug: &str,
        fallback: Vec<ChapterInfo>,
    ) -> Result<Vec<ChapterInfo>> {
        let comic = self.fetch_comic_data(slug)?;
        if comic.slug == slug {
            return Ok(fallback);
        }
        log::debug!("{NAME}: resolving legacy slug {slug} -> {}", comic.slug);
        self.collect_chapters(&comic.slug)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn normalises_manga_paths() {
        for path in ["witch", "/witch", "/comic/witch", "https://comick.live/comic/witch/"] {
            assert_eq!(manga_slug(path), "witch", "{path}");
        }
        assert_eq!(manga_slug("/comic/00-witch"), "00-witch");
        assert_eq!(request_path("https://comick.live/a/b"), "/a/b");
    }

    #[test]
    fn builds_absolute_image_urls() {
        assert_eq!(
            absolute_url("https://cdn1.example/covers/1.webp"),
            "https://cdn1.example/covers/1.webp"
        );
        assert_eq!(absolute_url("//cdn.example/1.webp"), "https://cdn.example/1.webp");
        assert_eq!(absolute_url("/img/1.webp"), format!("{URL}/img/1.webp"));
        assert_eq!(absolute_url("  "), "");
    }

    #[test]
    fn chapter_titles_match_the_kotlin_format() {
        let chapter = ChapterDto {
            hid: "IhXNoRw".to_string(),
            chap: Some("12.5".to_string()),
            vol: Some("2".to_string()),
            lang: "en".to_string(),
            title: Some("The Gate".to_string()),
            created_at: Some("2025-08-12T02:20:40.000000Z".to_string()),
            group_name: Some(vec!["flamecomics".to_string(), "".to_string()]),
        };
        let info = chapter_from_dto("00-solo-leveling", chapter);
        assert_eq!(info.title, "Vol. 2 Ch. 12.5: The Gate");
        assert_eq!(
            info.path,
            "/comic/00-solo-leveling/IhXNoRw-chapter-12.5-en"
        );
        assert_eq!(info.number, 12.5);
        assert_eq!(info.scanlator.as_deref(), Some("flamecomics"));
        assert_eq!(info.uploaded, 1754965240000);
    }
}
