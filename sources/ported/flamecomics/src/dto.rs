//! Wire types for the Flame Comics Next.js `_next/data` routes.
//!
//! The payloads are not stable API models: browse entries carry `categories`
//! while the detail route carries `tags`, latest entries are partial (no
//! author/description/genre) and chapter numbers are serialized as strings
//! (`"311.00"`). Optional fields therefore dominate and list/detail use
//! separate conversions.

use std::collections::BTreeMap;

use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, MangaInfo};

use crate::{
    CDN_IMAGES, ID, NAME, chapter_title, html_to_text, normalize_title, parse_chapter_number,
    status_name,
};

/// The `__NEXT_DATA__` payload embedded in every page.
#[derive(Debug, Deserialize)]
pub struct NextRoot {
    #[serde(rename = "buildId")]
    pub build_id: String,
}

/// Every data route wraps its payload in `pageProps`.
#[derive(Debug, Deserialize)]
pub struct NextData<T> {
    #[serde(rename = "pageProps")]
    pub page_props: T,
}

/// `browse.json` — the full catalogue, the source of popular and search.
#[derive(Debug, Deserialize)]
pub struct BrowseProps {
    pub series: Vec<Series>,
}

/// `latest.json` — the catalogue ordered by latest chapter release.
#[derive(Debug, Deserialize)]
pub struct LatestProps {
    #[serde(rename = "allSeries")]
    pub all_series: Vec<Series>,
}

/// `series/{id}.json` — details plus the full (unpaginated) chapter list.
#[derive(Debug, Deserialize)]
pub struct SeriesProps {
    pub series: Series,
    pub chapters: Vec<Chapter>,
}

/// `series/{id}/{token}.json` — one chapter's image list.
#[derive(Debug, Deserialize)]
pub struct ChapterProps {
    pub chapter: ChapterImages,
}

/// One series, from any of the three routes that embed it.
#[derive(Debug, Deserialize)]
pub struct Series {
    #[serde(rename = "series_id", default)]
    pub series_id: Option<i64>,
    pub title: String,
    /// Plain text (browse) or inline HTML (detail / latest).
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "altTitles", default)]
    pub alt_titles: Option<Vec<String>>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// Browse calls it `categories`, the detail route calls it `tags`.
    #[serde(default, alias = "categories")]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub author: Option<Vec<String>>,
    #[serde(default)]
    pub artist: Option<Vec<String>>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub cover: Option<String>,
    /// Cover cache-busting timestamp, appended to the CDN URL by the site.
    #[serde(rename = "last_edit", default)]
    pub last_edit: Option<i64>,
    #[serde(rename = "popularityRank", default)]
    pub popularity_rank: Option<i64>,
}

impl Series {
    /// Stored identity: the series id is enough to rebuild every route.
    pub fn path(&self) -> Option<String> {
        self.series_id.map(|id| format!("/series/{id}"))
    }

    /// CDN cover, cache-busted with `last_edit` like the site does.
    pub fn cover_url(&self) -> String {
        cover_url(self.series_id, self.cover.as_deref(), self.last_edit)
    }

    /// Whether the title (or an alternative title) contains the normalized
    /// query, matching the original's title filter.
    pub fn matches_title(&self, normalized_query: &str) -> bool {
        normalize_title(&self.title).contains(normalized_query)
            || self
                .alt_titles
                .iter()
                .flatten()
                .any(|alt| normalize_title(alt).contains(normalized_query))
    }

    /// Whether the synopsis mentions the normalized query.
    pub fn matches_description(&self, normalized_query: &str) -> bool {
        self.description
            .as_deref()
            .map(normalize_title)
            .is_some_and(|description| description.contains(normalized_query))
    }

    /// Listing conversion: browsing only needs title, cover and path, which
    /// is exactly what the original stores for list entries.
    pub fn into_list_manga(self) -> Option<MangaInfo> {
        let path = self.path()?;
        let cover_url = self.cover_url();
        Some(MangaInfo {
            source_id: ID,
            title: self.title,
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path,
            cover_url,
        })
    }

    /// Detail conversion, mirroring `SeriesDto.toSMangaDetails` in the
    /// original: kind + tags as genres, author + artist as authors, synopsis
    /// plus alternative names as the description.
    pub fn into_detail_manga(self) -> anyhow::Result<MangaInfo> {
        let series_id = self
            .series_id
            .ok_or_else(|| anyhow::anyhow!("{NAME}: series response has no series id"))?;

        let mut genre: Vec<String> = Vec::new();
        if let Some(kind) = non_empty(self.kind.as_deref()) {
            genre.push(kind.to_string());
        }
        for tag in self.tags.unwrap_or_default() {
            push_unique(&mut genre, &tag);
        }

        let mut author: Vec<String> = Vec::new();
        for name in self
            .author
            .unwrap_or_default()
            .into_iter()
            .chain(self.artist.unwrap_or_default())
        {
            push_unique(&mut author, &name);
        }

        let mut description = self
            .description
            .as_deref()
            .map(html_to_text)
            .filter(|text| !text.is_empty())
            .unwrap_or_default();
        let alt_titles: Vec<String> = self
            .alt_titles
            .unwrap_or_default()
            .into_iter()
            .filter_map(|title| non_empty(Some(title.as_str())).map(str::to_string))
            .collect();
        if !alt_titles.is_empty() {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Alternative Names:");
            for name in &alt_titles {
                description.push_str("\n- ");
                description.push_str(name);
            }
        }

        Ok(MangaInfo {
            source_id: ID,
            title: self.title,
            author,
            genre,
            status: status_name(self.status.as_deref()),
            description: (!description.is_empty()).then_some(description),
            path: format!("/series/{series_id}"),
            cover_url: cover_url(Some(series_id), self.cover.as_deref(), self.last_edit),
        })
    }
}

/// One chapter in a series' chapter list. `chapter` is a string like
/// `"311.00"` in the live API, but tolerate a JSON number as well.
#[derive(Debug, Deserialize)]
pub struct Chapter {
    #[serde(rename = "series_id", default)]
    pub series_id: Option<i64>,
    #[serde(deserialize_with = "deserialize_number_text")]
    pub chapter: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "release_date", default)]
    pub release_date: i64,
    pub token: String,
}

impl Chapter {
    pub fn into_chapter(self) -> Option<ChapterInfo> {
        let series_id = self.series_id?;
        let number = parse_chapter_number(&self.chapter).unwrap_or(-1.0);
        Some(ChapterInfo {
            source_id: ID,
            title: chapter_title(number, self.title.as_deref()),
            path: format!("/series/{series_id}/{}", self.token),
            number,
            scanlator: None,
            uploaded: self.release_date.saturating_mul(1000),
        })
    }
}

/// `chapter` field of a chapter page: `images` is keyed by the page index,
/// which must be sorted numerically (a JSON map does not preserve `"10"` after
/// `"9"`).
#[derive(Debug, Deserialize)]
pub struct ChapterImages {
    #[serde(rename = "series_id")]
    pub series_id: i64,
    #[serde(rename = "release_date", default)]
    pub release_date: Option<i64>,
    /// The site cache-busts page images with `edit_time`; fall back to
    /// `release_date` (which is what the original used).
    #[serde(rename = "edit_time", default)]
    pub edit_time: Option<i64>,
    #[serde(default)]
    pub images: BTreeMap<String, ImageEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ImageEntry {
    pub name: String,
}

impl ChapterImages {
    /// Build the CDN page URLs, in page order.
    pub fn into_pages(self, token: &str) -> anyhow::Result<Vec<String>> {
        let timestamp = self.edit_time.or(self.release_date).unwrap_or(0);
        let token = urlencoding::encode(token);
        let mut pages: Vec<(u64, String)> = self
            .images
            .into_iter()
            .filter_map(|(index, image)| {
                let name = non_empty(Some(image.name.as_str()))?;
                // Non-numeric keys (unexpected) sort last rather than break.
                let index = index.parse::<u64>().unwrap_or(u64::MAX);
                Some((index, name.to_string()))
            })
            .collect();
        pages.sort_by_key(|(index, _)| *index);
        if pages.is_empty() {
            anyhow::bail!("{NAME}: chapter returned an empty page list");
        }
        Ok(pages
            .into_iter()
            .map(|(_, name)| {
                format!(
                    "{CDN_IMAGES}/{}/{token}/{}?{timestamp}",
                    self.series_id,
                    urlencoding::encode(&name)
                )
            })
            .collect())
    }
}

fn cover_url(series_id: Option<i64>, cover: Option<&str>, last_edit: Option<i64>) -> String {
    let (Some(series_id), Some(cover)) = (series_id, non_empty(cover)) else {
        return String::new();
    };
    let mut url = format!(
        "{CDN_IMAGES}/{series_id}/{}",
        urlencoding::encode(cover)
    );
    if let Some(timestamp) = last_edit {
        url.push('?');
        url.push_str(&timestamp.to_string());
    }
    url
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if let Some(value) = non_empty(Some(value))
        && !values.iter().any(|existing| existing == value)
    {
        values.push(value.to_string());
    }
}

/// Accept either a JSON string (`"311.00"`) or a number for the chapter field.
fn deserialize_number_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberText {
        Text(String),
        Number(f64),
    }

    Ok(match NumberText::deserialize(deserializer)? {
        NumberText::Text(text) => text,
        NumberText::Number(number) => number.to_string(),
    })
}
