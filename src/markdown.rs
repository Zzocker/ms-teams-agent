//! Markdown in message text (people paste it, often as a Teams "Markdown" code block):
//! laid out as plain text with its styles kept as byte ranges.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Bold,
    Italic,
    Code,
    /// A fenced or indented code block.
    Block,
    Heading,
}

pub struct Md {
    pub text: String,
    pub spans: Vec<(Range<usize>, Style)>,
    /// Where each byte of the source went in `text`, so ranges into the source (links) still fit.
    map: Vec<usize>,
}

impl Md {
    pub fn at(&self, i: usize) -> usize {
        self.map[i.min(self.map.len() - 1)]
    }

    fn push(&mut self, s: &str, src: &str, from: Range<usize>) {
        // Text kept as written maps byte for byte; escapes and entities only map where they start.
        let at = src[from.clone()].find(s).map(|i| from.start + i);
        match at {
            Some(at) => (0..s.len()).for_each(|i| self.map[at + i] = self.text.len() + i),
            None => self.map[from.start] = self.text.len(),
        }
        self.text.push_str(s);
    }

    fn line(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    /// A blank line before a block, as the source has between paragraphs.
    fn gap(&mut self) {
        self.line();
        if !self.text.is_empty() && !self.text.ends_with("\n\n") {
            self.text.push('\n');
        }
    }
}

// shortcut: tables, task lists and strikethrough stay as typed; turn on their pulldown-cmark Options if people send them.
pub fn parse(src: &str) -> Md {
    const UNSET: usize = usize::MAX;
    let mut md = Md { text: String::new(), spans: vec![], map: vec![UNSET; src.len() + 1] };
    let mut open: Vec<(usize, Option<Style>)> = vec![];
    // Each open list's next number (None: bullets).
    let mut lists: Vec<Option<u64>> = vec![];
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let style = match &tag {
                    Tag::Strong => Some(Style::Bold),
                    Tag::Emphasis => Some(Style::Italic),
                    Tag::Heading { .. } => Some(Style::Heading),
                    Tag::CodeBlock(_) => Some(Style::Block),
                    _ => None,
                };
                match tag {
                    Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(CodeBlockKind::Fenced(_) | CodeBlockKind::Indented)
                    | Tag::BlockQuote(_) if !after_marker(&md) => md.gap(),
                    Tag::List(first) => {
                        if lists.is_empty() { md.gap() } else { md.line() }
                        lists.push(first);
                    }
                    Tag::Item => {
                        md.line();
                        md.text.push_str(&"  ".repeat(lists.len().saturating_sub(1)));
                        match lists.last_mut() {
                            Some(Some(n)) => { md.text.push_str(&format!("{n}. ")); *n += 1; }
                            _ => md.text.push_str("• "),
                        }
                    }
                    _ => {}
                }
                open.push((md.text.len(), style));
            }
            Event::End(end) => {
                if let Some((start, Some(style))) = open.pop() {
                    let end_at = md.text.trim_end_matches('\n').len().max(start);
                    if end_at > start { md.spans.push((start..end_at, style)); }
                }
                if let TagEnd::List(_) = end { lists.pop(); }
            }
            Event::Text(t) => md.push(&t, src, range),
            Event::Code(t) => {
                let start = md.text.len();
                md.push(&t, src, range);
                md.spans.push((start..md.text.len(), Style::Code));
            }
            Event::Html(t) | Event::InlineHtml(t) => md.push(&t, src, range),
            // A chat message's line breaks are meant, unlike a document's.
            Event::SoftBreak | Event::HardBreak => md.text.push('\n'),
            Event::Rule => { md.gap(); md.text.push_str("———") }
            _ => {}
        }
    }
    md.text.truncate(md.text.trim_end().len());
    // Markers and dropped source bytes map to where the next kept byte went.
    let mut next = md.text.len();
    for v in md.map.iter_mut().rev() {
        if *v == UNSET { *v = next } else { *v = (*v).min(md.text.len()); next = *v }
    }
    md
}

/// Right after a list marker, the item's first block shouldn't open a blank line.
fn after_marker(md: &Md) -> bool {
    md.text.ends_with("• ") || md.text.ends_with(". ")
}

#[cfg(test)]
mod tests {
    use super::{Style::*, parse};

    #[test]
    fn markers_go_and_their_styles_stay() {
        let src = "# Title\n- a **b** `c`\n  * nested *it*\n\n```\nx * y\n```\nend 2 * 3 * 4 and **open";
        let md = parse(src);
        assert_eq!(md.text, "Title\n\n• a b c\n  • nested it\n\nx * y\n\nend 2 * 3 * 4 and **open");
        let styled: Vec<_> = md.spans.iter().map(|(r, s)| (&md.text[r.clone()], *s)).collect();
        assert_eq!(styled, [("Title", Heading), ("b", Bold), ("c", Code), ("it", Italic), ("x * y", Block)]);
        // A link over "b" in the source still covers "b".
        let b = src.find("**b**").unwrap() + 2;
        assert_eq!(&md.text[md.at(b)..md.at(b + 1)], "b");
        assert_eq!(md.at(src.len()), md.text.len());
    }
}
