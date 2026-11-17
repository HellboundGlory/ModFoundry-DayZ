//! Cursor paging for the MCP list methods (2025-06-18).
//!
//! `cursor` and `nextCursor` are opaque strings to a client, and `nextCursor`
//! appears only when more results exist — a list that fits one page carries
//! none. The cursor this server hands out is the offset of the next page; a
//! cursor it did not hand out is a bad request rather than a silent restart.

/// How many entries one page holds.
pub const PAGE_SIZE: usize = 100;

/// One page of a listing, and the cursor that continues it.
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Decode a client cursor: no cursor is the first page, anything else is the
/// offset a previous `nextCursor` named.
pub fn offset(cursor: Option<&str>) -> Result<usize, String> {
    match cursor {
        None => Ok(0),
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| format!("invalid cursor: {text:?}")),
    }
}

/// Page `items` at `offset`, emitting a cursor only when entries follow.
pub fn page<T>(items: Vec<T>, offset: usize) -> Page<T> {
    let next_cursor =
        (items.len() > offset.saturating_add(PAGE_SIZE)).then(|| (offset + PAGE_SIZE).to_string());
    Page {
        items: items.into_iter().skip(offset).take(PAGE_SIZE).collect(),
        next_cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_is_an_offset_and_nonsense_is_a_bad_request() {
        assert_eq!(offset(None).unwrap(), 0);
        assert_eq!(offset(Some("12")).unwrap(), 12);
        assert!(offset(Some("next-page")).is_err());
        assert!(offset(Some("-1")).is_err());
    }

    #[test]
    fn a_short_list_is_one_page_without_a_cursor() {
        let page = page(vec![1, 2, 3], 0);
        assert_eq!(page.items, [1, 2, 3]);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn an_offset_past_the_end_is_an_empty_page() {
        assert!(page(vec![1, 2, 3], 3).items.is_empty());
        assert!(page(vec![1, 2, 3], 99).items.is_empty());
        assert!(page::<u8>(Vec::new(), 0).items.is_empty());
    }

    #[test]
    fn a_long_list_pages_and_the_cursor_walks_it() {
        let all: Vec<usize> = (0..PAGE_SIZE + 5).collect();

        let first = page(all.clone(), 0);
        assert_eq!(first.items.len(), PAGE_SIZE);
        assert_eq!(first.items[0], 0);
        assert_eq!(first.next_cursor.as_deref(), Some("100"));

        let next = offset(first.next_cursor.as_deref()).unwrap();
        let second = page(all, next);
        assert_eq!(second.items.len(), 5);
        assert_eq!(second.items[0], PAGE_SIZE);
        assert_eq!(second.next_cursor, None);
    }
}
