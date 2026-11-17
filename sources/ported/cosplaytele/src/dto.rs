//! JSON shapes for the WordPress popular-posts endpoint used by CosplayTele.

use serde::Deserialize;
use tanoshi_lib::prelude::MangaInfo;

use crate::ID;
use crate::parse::{absolute, site_relative, unescape_html};

#[derive(Debug, Deserialize)]
pub struct PopularPostDto {
    pub title: RenderedField,
    pub link: String,
    #[serde(rename = "_embedded")]
    pub embedded: Option<EmbeddedField>,
}

#[derive(Debug, Deserialize)]
pub struct RenderedField {
    pub rendered: String,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddedField {
    #[serde(rename = "wp:featuredmedia")]
    pub featured_media: Option<Vec<FeaturedMediaDto>>,
}

#[derive(Debug, Deserialize)]
pub struct FeaturedMediaDto {
    #[serde(default)]
    pub source_url: Option<String>,
}

impl PopularPostDto {
    pub fn into_manga(self) -> MangaInfo {
        let title = unescape_html(&self.title.rendered);
        let cover_url = self
            .embedded
            .and_then(|embedded| embedded.featured_media)
            .and_then(|media| media.into_iter().next())
            .and_then(|media| media.source_url)
            .map(|url| absolute(&url))
            .unwrap_or_default();

        MangaInfo {
            source_id: ID,
            title,
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path: site_relative(&self.link),
            cover_url,
        }
    }
}
