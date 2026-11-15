//! Markdown in message text, read by pulldown-cmark: laid out as plain text with its styles kept
//! as byte ranges. A ```markdown block (as Teams sends a pasted one) is read as Markdown too.

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser, Tag, TagEnd};
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

    /// Markers and dropped source bytes map to where the next kept byte went.
    fn fill_map(&mut self) {
        let mut next = self.text.len();
        for v in self.map.iter_mut().rev() {
            if *v == UNSET { *v = next } else { *v = (*v).min(self.text.len()); next = *v }
        }
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
const UNSET: usize = usize::MAX;

pub fn parse(src: &str) -> Md {
    let mut md = Md { text: String::new(), spans: vec![], map: vec![UNSET; src.len() + 1] };
    let mut open: Vec<(usize, Option<Style>)> = vec![];
    // Each open list's next number (None: bullets).
    let mut lists: Vec<Option<u64>> = vec![];
    // The open ```markdown block's text, read as Markdown when it ends.
    let mut nested: Option<String> = None;
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Text(t) if nested.is_some() => nested.iter_mut().for_each(|n| n.push_str(&t)),
            Event::End(TagEnd::CodeBlock) if nested.is_some() => {
                open.pop();
                let inner = parse(&nested.take().unwrap_or_default());
                let start = md.text.len();
                md.spans.extend(inner.spans.into_iter().map(|(r, s)| (r.start + start..r.end + start, s)));
                md.text.push_str(&inner.text);
            }
            Event::Start(tag) => {
                if let Tag::CodeBlock(CodeBlockKind::Fenced(lang)) = &tag && is_markdown(lang) {
                    nested = Some(String::new());
                }
                let style = match &tag {
                    Tag::Strong => Some(Style::Bold),
                    Tag::Emphasis => Some(Style::Italic),
                    Tag::Heading { .. } => Some(Style::Heading),
                    Tag::CodeBlock(_) if nested.is_none() => Some(Style::Block),
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
    md.fill_map();
    md
}

fn is_markdown(lang: &CowStr) -> bool {
    matches!(lang.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str(), "markdown" | "md")
}

/// The fenced code blocks pulldown-cmark finds in `src`: each one's source lines (fences, their
/// indent and the closing line's newline included), language and code.
pub fn code_blocks(src: &str) -> Vec<(Range<usize>, String, String)> {
    let mut blocks: Vec<(Range<usize>, String, String)> = vec![];
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                let start = range.start - (src[..range.start].len() - src[..range.start].trim_end_matches([' ', '\t']).len());
                let end = range.end + usize::from(src[range.end..].starts_with('\n'));
                blocks.push((start..end, lang.split_whitespace().next().unwrap_or("").into(), String::new()));
            }
            Event::Text(t) => {
                if let Some((_, _, code)) = blocks.last_mut().filter(|(r, ..)| r.start <= range.start && range.end <= r.end) {
                    code.push_str(&t);
                }
            }
            _ => {}
        }
    }
    blocks
}

/// Right after a list marker, the item's first block shouldn't open a blank line.
fn after_marker(md: &Md) -> bool {
    md.text.ends_with("• ") || md.text.ends_with(". ")
}

#[cfg(test)]
mod tests {
    use super::{Style::*, code_blocks, parse};

    #[test]
    fn markdown_blocks_read_as_markdown_and_other_code_stays_code() {
        let src = "plain **bold**\n  ```markdown\n    # Heading\n  ```\n```rust\nlet x = 1;\n```\n````md\n```\nnested\n```\n````";
        let md = parse(src);
        assert_eq!(md.text, "plain bold\n\nHeading\n\nlet x = 1;\n\nnested");
        let styled: Vec<_> = md.spans.iter().map(|(r, s)| (&md.text[r.clone()], *s)).collect();
        assert_eq!(styled, [("bold", Bold), ("Heading", Heading), ("let x = 1;", Block), ("nested", Block)]);
        let blocks = code_blocks(src);
        assert_eq!(blocks.iter().map(|(r, l, c)| (&src[r.clone()], l.as_str(), c.as_str())).collect::<Vec<_>>(), [
            ("  ```markdown\n    # Heading\n  ```\n", "markdown", "  # Heading\n"),
            ("```rust\nlet x = 1;\n```\n", "rust", "let x = 1;\n"),
            ("````md\n```\nnested\n```\n````", "md", "```\nnested\n```\n"),
        ]);
    }

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


