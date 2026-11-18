use modfoundry_core::Span;

use crate::parse::Pos;

/// A source text with its line starts, to turn a position into a byte span.
pub struct Source<'a> {
    body: &'a str,
    base: usize,
    line_starts: Vec<usize>,
}

impl<'a> Source<'a> {
    /// Indexes the line starts of `text`; spans count bytes from the start of `text`, byte order mark included.
    pub fn new(text: &'a str) -> Self {
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        let breaks = body.match_indices('\n').map(|(at, _)| at + 1);
        Self {
            body,
            base: text.len() - body.len(),
            line_starts: std::iter::once(0).chain(breaks).collect(),
        }
    }

    /// The byte span of the line from `pos` to its end, clamped to the text.
    pub fn span_from(&self, pos: &Pos) -> Span {
        let line_start = self
            .line_starts
            .get(pos.line as usize - 1)
            .copied()
            .unwrap_or(self.body.len());
        let line_end = self.line_end(line_start);
        let skip = (pos.column as usize).saturating_sub(1);
        let start = self.body[line_start..line_end]
            .char_indices()
            .nth(skip)
            .map_or(line_end, |(at, _)| line_start + at);
        Span::new(self.base + start, self.base + line_end)
    }

    fn line_end(&self, line_start: usize) -> usize {
        let end = self.body[line_start..]
            .find('\n')
            .map_or(self.body.len(), |at| line_start + at);
        if self.body[line_start..end].ends_with('\r') {
            end - 1
        } else {
            end
        }
    }
}
