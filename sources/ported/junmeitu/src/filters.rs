//! Search filters, ported from `Junmeitu.getFilterList`.
//!
//! The original exposes free-text tag/model/group fields plus a category and a
//! sort select, and picks exactly one of them when building the listing URL.
//! The "ignored if using text search" precedence is preserved.

use tanoshi_lib::prelude::{Input, InputType};

use crate::URL;

/// `(label, path slug, category id)` — the original's `getCategoryFilter`.
const CATEGORIES: [(&str, &str, &str); 4] = [
    ("beauty", "beauty", "6"),
    ("handsome", "handsome", "5"),
    ("news", "news", "30"),
    ("street", "street", "32"),
];

/// `(label, path slug)` — the original's `getSortFilter`.
const SORTS: [(&str, &str); 2] = [("default", "index"), ("hot", "hot")];

pub(crate) fn filter_list() -> Vec<Input> {
    vec![
        Input::Text {
            name: "Tag".to_string(),
            state: None,
        },
        Input::Text {
            name: "Model".to_string(),
            state: None,
        },
        Input::Text {
            name: "Group".to_string(),
            state: None,
        },
        Input::Select {
            name: "Category".to_string(),
            values: CATEGORIES
                .iter()
                .map(|(label, _, _)| InputType::String((*label).to_string()))
                .collect(),
            state: Some(0),
        },
        Input::Select {
            name: "Sort".to_string(),
            values: SORTS
                .iter()
                .map(|(label, _)| InputType::String((*label).to_string()))
                .collect(),
            state: Some(0),
        },
    ]
}

/// Filter values selected by the client.
#[derive(Default)]
pub(crate) struct SearchFilters {
    pub(crate) tag: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) category: usize,
    pub(crate) sort: usize,
}

impl SearchFilters {
    pub(crate) fn parse(filters: &[Input]) -> Self {
        let mut out = SearchFilters::default();
        for filter in filters {
            match filter {
                Input::Text {
                    name,
                    state: Some(state),
                } => {
                    let value = state.trim();
                    if value.is_empty() {
                        continue;
                    }
                    match name.as_str() {
                        "Tag" => out.tag = Some(value.to_string()),
                        "Model" => out.model = Some(value.to_string()),
                        "Group" => out.group = Some(value.to_string()),
                        _ => {}
                    }
                }
                Input::Select {
                    name,
                    state: Some(state),
                    ..
                } => {
                    let index = usize::try_from(*state).unwrap_or(0);
                    match name.as_str() {
                        "Category" => out.category = index,
                        "Sort" => out.sort = index,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Reproduce the original's `when` chain for the search URL.
    pub(crate) fn search_url(&self, page: i64, query: &str) -> String {
        let (_, category_slug, _) = CATEGORIES
            .get(self.category)
            .copied()
            .unwrap_or(CATEGORIES[0]);
        let (_, sort_slug) = SORTS.get(self.sort).copied().unwrap_or(SORTS[0]);

        if !query.is_empty() {
            return format!("{URL}/search/{}-{page}.html", urlencoding::encode(query));
        }
        if let Some(tag) = &self.tag {
            // The site's tag pages are `/tags/{slug}.html` and
            // `/tags/{slug}-{page}.html` (verified live; the original's
            // `{tag}-{catid}-{page}` form now renders the page-1 content for
            // every page and is ignored by the site).
            let tag = urlencoding::encode(tag);
            return if page <= 1 {
                format!("{URL}/tags/{tag}.html")
            } else {
                format!("{URL}/tags/{tag}-{page}.html")
            };
        }
        if let Some(model) = &self.model {
            return format!("{URL}/model/{}-{page}.html", urlencoding::encode(model));
        }
        if let Some(group) = &self.group {
            return format!("{URL}/xzjg/{}-{page}.html", urlencoding::encode(group));
        }
        if self.category > 0 || self.sort > 0 {
            return format!("{URL}/{category_slug}/{sort_slug}-{page}.html");
        }
        format!("{URL}/beauty/index-{page}.html")
    }
}
