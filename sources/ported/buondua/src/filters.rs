//! Search filters, ported from `BuonDua.getFilterList`.
//!
//! The original exposes a single free-text "Tag ID" field (a site tag slug such
//! as `yeha-11167`); the keyword takes precedence when both are given.

use tanoshi_lib::prelude::Input;

pub(crate) fn filter_list() -> Vec<Input> {
    vec![Input::Text {
        name: "Tag ID".to_string(),
        state: None,
    }]
}

/// Tag slug selected by the client, if any.
pub(crate) fn tag_id(filters: &[Input]) -> Option<String> {
    filters.iter().find_map(|filter| match filter {
        Input::Text {
            name,
            state: Some(state),
        } if name == "Tag ID" => {
            let tag = state.trim();
            (!tag.is_empty()).then(|| tag.to_string())
        }
        _ => None,
    })
}
