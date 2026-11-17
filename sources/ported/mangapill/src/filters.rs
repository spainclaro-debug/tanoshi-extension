//! Search filters, ported from the extension's `Filters.kt`.
//!
//! Keiyoushi declares a `Status` and a `Type` select plus a tri-state genre
//! group. Tanoshi exposes filters as generic [`Input`] values, so [`filter_list`]
//! builds that shape and [`search_filters`] reads the values the client sends
//! back into the query parameters that `/search` understands.
//!
//! Like the original, a value is only added to the query when it is actually
//! selected: tri-state genres must be `Included` (excluded genres are ignored,
//! exactly as `state == 1` is checked upstream).

use tanoshi_lib::prelude::{Input, InputType, TriState};

/// `(label, query value)` pairs, in the original filter order.
const STATUSES: [(&str, &str); 6] = [
    ("All", ""),
    ("Publishing", "publishing"),
    ("Finished", "finished"),
    ("On Hiatus", "on hiatus"),
    ("Discontinued", "discontinued"),
    ("Not yet Published", "not yet published"),
];

const TYPES: [(&str, &str); 8] = [
    ("All", ""),
    ("Manga", "manga"),
    ("Novel", "novel"),
    ("One-Shot", "one-shot"),
    ("Doujinshi", "doujinshi"),
    ("Manhwa", "manhwa"),
    ("Manhua", "manhua"),
    ("Oel", "oel"),
];

/// Site genre values, in the order the site lists them in its search form.
const GENRES: &[&str] = &[
    "Action",
    "Adventure",
    "Cars",
    "Comedy",
    "Dementia",
    "Demons",
    "Drama",
    "Ecchi",
    "Fantasy",
    "Game",
    "Harem",
    "Hentai",
    "Historical",
    "Horror",
    "Josei",
    "Kids",
    "Magic",
    "Martial Arts",
    "Mecha",
    "Military",
    "Music",
    "Mystery",
    "Parody",
    "Police",
    "Psychological",
    "Romance",
    "Samurai",
    "School",
    "Sci-Fi",
    "Seinen",
    "Shoujo",
    "Shoujo Ai",
    "Shounen",
    "Shounen Ai",
    "Slice of Life",
    "Space",
    "Sports",
    "Super Power",
    "Supernatural",
    "Thriller",
    "Vampire",
    "Yaoi",
    "Yuri",
];

fn select_filter(name: &str, values: &[(&str, &str)]) -> Input {
    Input::Select {
        name: name.to_string(),
        values: values
            .iter()
            .map(|(label, _)| InputType::String((*label).to_string()))
            .collect(),
        state: Some(0),
    }
}

pub(crate) fn filter_list() -> Vec<Input> {
    vec![
        select_filter("Status", &STATUSES),
        select_filter("Type", &TYPES),
        Input::Group {
            name: "Genres".to_string(),
            state: GENRES
                .iter()
                .map(|genre| Input::State {
                    name: (*genre).to_string(),
                    selected: Some(TriState::Ignored),
                })
                .collect(),
        },
    ]
}

/// Filter values selected by the client, ready for the search URL.
#[derive(Default)]
pub(crate) struct SearchFilters {
    /// Query value for `status`; `None` means "All".
    pub(crate) status: Option<String>,
    /// Query value for `type`; `None` means "All".
    pub(crate) kind: Option<String>,
    /// Genre values to repeat as `genre=` parameters.
    pub(crate) genres: Vec<String>,
}

/// Read the client's [`Input`] values back into query parameters.
pub(crate) fn search_filters(filters: &[Input]) -> SearchFilters {
    let mut out = SearchFilters::default();

    for filter in filters {
        match filter {
            Input::Select {
                name,
                state: Some(state),
                ..
            } if name == "Status" => {
                out.status = uri_value(&STATUSES, *state);
            }
            Input::Select {
                name,
                state: Some(state),
                ..
            } if name == "Type" => {
                out.kind = uri_value(&TYPES, *state);
            }
            Input::Group { name, state } if name == "Genres" => {
                for genre in state {
                    if let Input::State {
                        name,
                        selected: Some(TriState::Included),
                    } = genre
                    {
                        let value = name.trim();
                        if !value.is_empty() && !out.genres.iter().any(|genre| genre == value) {
                            out.genres.push(value.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    out
}

/// Map a select index onto its query value, treating out-of-range and the
/// "All" entry as no filter.
fn uri_value(values: &[(&str, &str)], state: i64) -> Option<String> {
    let index = usize::try_from(state).ok()?;
    let value = values.get(index)?.1;
    (!value.is_empty()).then(|| value.to_string())
}
