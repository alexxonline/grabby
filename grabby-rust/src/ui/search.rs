//! Full-text transcript search (`pages/SearchPage.tsx`).

use rune::tok::{radius, space, type_};
use rune::{El, Span, TextEdit, col, row, text};
use rune_kit::{ButtonVariant, TextInput};

use super::{P, Ui, button, error, h1, muted, open_transcription};
use crate::core::{self, SearchResult};
use crate::speakers::parse_snippet;

#[derive(Default)]
pub struct SearchPage {
    pub query: TextEdit,
    pub results: Vec<SearchResult>,
    pub searched: bool,
    /// The query the results answer ("No results found for ...").
    pub searched_query: String,
    pub error: Option<String>,
}

fn do_search(s: &mut Ui) {
    let q = s.search.query.text.trim().to_owned();
    if q.is_empty() {
        return;
    }
    s.search.error = None;
    match core::search_transcriptions(&q) {
        Ok(results) => {
            s.search.results = results;
            s.search.searched = true;
            s.search.searched_query = s.search.query.text.clone();
        }
        Err(e) => s.search.error = Some(if e.detail.is_empty() { "Search failed".into() } else { e.detail }),
    }
}

fn display_name(r: &SearchResult) -> String {
    r.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| match r.filename.rfind('.') {
        Some(i) if i > 0 => r.filename[..i].to_owned(),
        _ => r.filename.clone(),
    })
}

fn result_item(p: &P, r: &SearchResult) -> El {
    let (snippet, marks) = parse_snippet(&r.snippet);
    let spans: Vec<Span> = marks
        .into_iter()
        .map(|(a, b)| Span { bg: Some(p.mark), bg_pad: 1.0, ..Span::new(a, b).bold() })
        .collect();
    let filename = r.filename.clone();
    col()
        .key(format!("result-{}", r.filename))
        .w_full()
        .padding(space::S3)
        .gap(space::S1)
        .radius(radius::MD)
        .fill(p.t.card)
        .border(1.0, p.t.card_border)
        .hover(p.t.hover)
        .press(p.t.press)
        .role("link")
        .focusable(true)
        .label(display_name(r))
        .on_press(move |s: &mut Ui| open_transcription(s, &filename, false))
        .child(text(display_name(r)).key("name").font(type_::TITLE).color(p.accent))
        .child(text(snippet).key("snippet").font(type_::BODY).color(p.t.text_secondary).wrap().spans(spans))
}

pub fn view(s: &Ui, p: &P) -> El {
    let q = &s.search;
    let empty_query = q.query.text.trim().is_empty();
    let mut page = col()
        .key("search-page")
        .w_full()
        .gap(space::S3)
        .child(h1(p, "title", "Search Transcripts"))
        .child(
            row()
                .key("search-bar")
                .w_full()
                .gap(space::S2)
                .center_y()
                .child(
                    TextInput::new(&q.query)
                        .placeholder("Search transcripts…")
                        .on_edit(|s: &mut Ui| &mut s.search.query)
                        .on_submit(|s: &mut Ui, _| do_search(s))
                        .build(&p.k)
                        .key("search-input")
                        .grow(1.0),
                )
                .child(button(p, "search-button", "Search", ButtonVariant::Submit, empty_query, do_search)),
        );
    if let Some(e) = &q.error {
        page = page.child(error(p, "error", e.clone()));
    }
    if q.searched && q.results.is_empty() {
        page = page.child(muted(p, "empty", format!("No results found for \"{}\"", q.searched_query)));
    }
    if !q.results.is_empty() {
        let mut list = col().key("search-results").w_full().gap(space::S2).role("list");
        for r in &q.results {
            list = list.child(result_item(p, r));
        }
        page = page.child(list);
    }
    page
}
