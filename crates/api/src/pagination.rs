use crate::HttpApiError;
use agentkube_core::{Metadata, ResourceVersion};
use agentkube_protocol::{ContinueToken, ListMetadata, ListOptions};

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 200;

pub(crate) fn paginate<T>(
    items: Vec<T>,
    options: &ListOptions,
    metadata: impl Fn(&T) -> &Metadata,
) -> Result<(Vec<T>, ListMetadata), HttpApiError> {
    let requested = options
        .limit()
        .map_or(DEFAULT_PAGE_SIZE, |limit| limit.get() as usize);
    if requested > MAX_PAGE_SIZE {
        return Err(HttpApiError::bad_request(format!(
            "limit must not exceed {MAX_PAGE_SIZE}"
        )));
    }
    let start = match options.continue_token() {
        Some(token) => items
            .iter()
            .position(|item| resource_token(metadata(item)) == token.as_str())
            .map(|index| index + 1)
            .ok_or_else(|| HttpApiError::bad_request("continue token is invalid or expired"))?,
        None => 0,
    };
    let end = start.saturating_add(requested).min(items.len());
    let remaining = items.len().saturating_sub(end);
    let collection_version = items
        .iter()
        .map(|item| metadata(item).resource_version())
        .max()
        .unwrap_or(ResourceVersion::INITIAL);
    let mut list_metadata = ListMetadata::new()
        .with_resource_version(collection_version)
        .with_remaining_item_count(remaining as u64);
    if remaining > 0
        && let Some(last) = items.get(end.saturating_sub(1))
    {
        let token = ContinueToken::new(resource_token(metadata(last)))
            .expect("validated names and namespaces create a bounded token");
        list_metadata = list_metadata.with_continue_token(token);
    }
    Ok((
        items.into_iter().skip(start).take(requested).collect(),
        list_metadata,
    ))
}

fn resource_token(metadata: &Metadata) -> String {
    format!("{}/{}", metadata.namespace(), metadata.name())
}
