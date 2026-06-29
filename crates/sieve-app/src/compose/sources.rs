use crate::render_refs::{resolve_ref_summary_input, RenderRef};
use crate::response_style::{
    dedupe_preserve_order, extract_plain_urls_from_text, filter_non_asset_urls,
};
use sieve_llm::ResponseTurnInput;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SOURCE_URLS: usize = 8;

pub(super) async fn collect_source_urls_from_refs(
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut seen = BTreeSet::new();
    for outcome in &response_input.tool_outcomes {
        for metadata in &outcome.refs {
            if metadata.byte_count == 0 {
                continue;
            }
            let Some((content, _, _)) =
                resolve_ref_summary_input(&metadata.ref_id, render_refs).await
            else {
                continue;
            };
            for url in extract_plain_urls_from_text(&content) {
                if push_unique_source_url(&mut urls, &mut seen, url) {
                    return urls;
                }
            }
        }
    }
    urls
}

pub(super) fn collect_source_urls_from_extracted_evidence(
    evidence_records: &[sieve_llm::ResponseEvidenceRecord],
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut seen = BTreeSet::new();
    for record in evidence_records {
        for url in &record.source_urls {
            if push_unique_source_url(&mut urls, &mut seen, url.as_str()) {
                return urls;
            }
        }
        if let Some(candidate) = &record.answer_candidate {
            if let Some(url) = &candidate.url {
                if push_unique_source_url(&mut urls, &mut seen, url.as_str()) {
                    return urls;
                }
            }
        }
        for item in &record.items {
            if let Some(url) = &item.url {
                if push_unique_source_url(&mut urls, &mut seen, url.as_str()) {
                    return urls;
                }
            }
        }
    }
    urls
}

pub(super) fn normalize_source_urls(urls: Vec<String>) -> Vec<String> {
    filter_non_asset_urls(dedupe_preserve_order(urls))
}

fn push_unique_source_url(
    urls: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    url: impl Into<String>,
) -> bool {
    let url = url.into();
    if seen.insert(url.clone()) {
        urls.push(url);
    }
    urls.len() >= MAX_SOURCE_URLS
}
