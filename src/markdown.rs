//! Markdown preview.
//!
//! Markdown is parsed into a small block tree once per edit, and every text
//! block is shaped into a `Galley` that is cached afterwards. A frame then only
//! paints cached geometry, so scrolling stays smooth regardless of document
//! length. The caller debounces re-parsing while the user types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{
    Color32, FontFamily, Galley, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    epaint::text::{LayoutJob, TextWrapping},
    text::CCursor,
};
use pulldown_cmark::{Alignment, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::theme::{FAMILY_UI, bold_font, c, fs as tfs, mono_font, sp, ui_font};
use crate::thumbs;

/// One run of inline text sharing a style.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// Raised text (`<sup>` / `^x^`).
    pub sup: bool,
    /// Lowered text (`<sub>` / `~x~`).
    pub sub: bool,
    pub link: Option<String>,
}

impl Span {
    fn plain(text: &str) -> Span {
        Span {
            text: text.to_owned(),
            ..Default::default()
        }
    }
}

/// A list item, optionally a task-list checkbox.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A table with its column alignments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub aligns: Vec<Alignment>,
    pub header: Vec<Vec<Span>>,
    pub rows: Vec<Vec<Vec<Span>>>,
}

/// A block-level element.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Para(Vec<Span>),
    Code {
        lang: String,
        text: String,
    },
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Table(Table),
    Rule,
    /// Raw HTML, shown as dimmed source rather than dropped.
    Html(String),
    /// A `<details>` disclosure: its summary and, when open, its body.
    Details {
        summary: String,
        blocks: Vec<Block>,
        open: bool,
    },
    Image {
        alt: String,
        url: String,
        /// Explicit `width`/`height` from a raw `<img>` tag, in points.
        width: Option<f32>,
        height: Option<f32>,
    },
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
}

/// Parses Markdown into blocks.
#[cfg(test)]
pub fn parse(source: &str) -> Vec<Block> {
    parse_with_lines(source).0
}

/// Parses Markdown into blocks, and says which line of the source each one starts on.
pub fn parse_with_lines(source: &str) -> (Vec<Block>, Vec<usize>) {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_SMART_PUNCTUATION;
    let (blocks, starts) = Builder::run(Parser::new_ext(source, options).into_offset_iter());
    // `<details>` arrives as three or more top-level HTML blocks — the opening tag,
    // the summary, then the body — so they are folded into one disclosure here.
    let (blocks, starts) = fold_details(blocks, starts);
    // Byte offsets to line numbers, counting the newlines between one and the next.
    let bytes = source.as_bytes();
    let (mut at, mut line) = (0usize, 0usize);
    let lines = starts
        .into_iter()
        .map(|off| {
            let off = off.min(bytes.len());
            if off < at {
                at = 0;
                line = 0;
            }
            line += bytes[at..off].iter().filter(|b| **b == b'\n').count();
            at = off;
            line
        })
        .collect();
    (blocks, lines)
}

/// Folds the HTML blocks of a `<details>` element into one [`Block::Details`].
///
/// The parser hands back the opening tag, the summary and the closing tag as
/// separate top-level HTML blocks, with the body between them parsed normally.
/// This walks the list and gathers each run into a single block, keeping the
/// starting offset of the opening tag so the line mapping still lines up.
fn fold_details(blocks: Vec<Block>, starts: Vec<usize>) -> (Vec<Block>, Vec<usize>) {
    let mut slots: Vec<Option<Block>> = blocks.into_iter().map(Some).collect();
    let mut out: Vec<Block> = Vec::with_capacity(slots.len());
    let mut out_starts: Vec<usize> = Vec::with_capacity(starts.len());
    let mut i = 0;
    while i < slots.len() {
        let open = match slots[i].as_ref() {
            Some(Block::Html(h)) => details_open(h),
            _ => None,
        };
        let Some(open) = open else {
            if let Some(b) = slots[i].take() {
                out.push(b);
                out_starts.push(starts[i]);
            }
            i += 1;
            continue;
        };
        let start = starts[i];
        let mut summary = String::new();
        if let Some(Block::Html(h)) = slots[i].as_ref() {
            extract_summary(h, &mut summary);
        }
        let mut inner = Vec::new();
        let mut j = i + 1;
        while j < slots.len() {
            match slots[j].as_ref() {
                Some(Block::Html(h)) if is_details_close(h) => {
                    slots[j] = None;
                    j += 1;
                    break;
                }
                Some(Block::Html(h)) if is_summary_only(h) => {
                    if summary.is_empty() {
                        extract_summary(h, &mut summary);
                    }
                    slots[j] = None;
                    j += 1;
                }
                _ => {
                    if let Some(b) = slots[j].take() {
                        inner.push(b);
                    }
                    j += 1;
                }
            }
        }
        out.push(Block::Details {
            summary,
            blocks: inner,
            open,
        });
        out_starts.push(start);
        i = j;
    }
    (out, out_starts)
}

/// Whether an HTML block opens a `<details>`, and whether it starts open.
fn details_open(html: &str) -> Option<bool> {
    let lower = html.to_ascii_lowercase();
    let rest = lower.split("<details").nth(1)?;
    let tag = rest.split('>').next().unwrap_or("");
    Some(tag.split_ascii_whitespace().any(|a| a == "open"))
}

/// Whether an HTML block closes a `<details>`.
fn is_details_close(html: &str) -> bool {
    html.to_ascii_lowercase().contains("</details>")
}

/// Whether an HTML block is just a `<summary>` line, which belongs to the header.
fn is_summary_only(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("<summary") && lower.contains("</summary>") && !lower.contains("<details")
}

/// Pulls the text out of a `<summary>` tag, tags and entities removed.
fn extract_summary(html: &str, out: &mut String) {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<summary") else {
        return;
    };
    let Some(gt) = html[start..].find('>') else {
        return;
    };
    let from = start + gt + 1;
    let Some(end) = lower[from..].find("</summary>") else {
        return;
    };
    out.push_str(&strip_html(&html[from..from + end]));
}

/// Turns a scrap of HTML into readable text: tags dropped, a few entities decoded,
/// whitespace collapsed.
fn strip_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decodes the handful of HTML entities that turn up in prose.
fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Recognises a lone `<img …>` tag and returns its `(alt, src, width, height)`.
/// Width and height are the tag's own attributes, in points, when it has them.
fn parse_img_tag(html: &str) -> Option<(String, String, Option<f32>, Option<f32>)> {
    let t = html.trim();
    let lower = t.to_ascii_lowercase();
    if !lower.starts_with("<img") || !lower.ends_with('>') {
        return None;
    }
    let src = attr_value(t, "src")?;
    let alt = attr_value(t, "alt").unwrap_or_default();
    let width = attr_value(t, "width").as_deref().and_then(css_len);
    let height = attr_value(t, "height").as_deref().and_then(css_len);
    Some((decode_entities(&alt), src, width, height))
}

/// A CSS length in points, from an attribute value like `320` or `320px`.
fn css_len(v: &str) -> Option<f32> {
    let v = v.trim();
    let v = v
        .strip_suffix("px")
        .or_else(|| v.strip_suffix("PX"))
        .unwrap_or(v);
    v.trim().parse::<f32>().ok().filter(|n| *n > 0.0)
}

/// The value of an HTML attribute in `tag`, double-quoted, single-quoted or bare.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let bytes = tag.as_bytes();
    let mut at = 0;
    while let Some(pos) = lower[at..].find(name) {
        let i = at + pos;
        // Only a real attribute: preceded by whitespace or the tag's `<`.
        let preceded = i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b'<';
        let rest = tag[i + name.len()..].trim_start();
        if preceded && let Some(rest) = rest.strip_prefix('=') {
            let rest = rest.trim_start();
            let bytes = rest.as_bytes();
            return match bytes.first() {
                Some(&q @ (b'"' | b'\'')) => {
                    let end = rest[1..].find(q as char)?;
                    Some(rest[1..1 + end].to_owned())
                }
                Some(_) => {
                    let end = rest
                        .find(|c: char| c.is_ascii_whitespace() || c == '>')
                        .unwrap_or(rest.len());
                    Some(rest[..end].to_owned())
                }
                None => None,
            };
        }
        at = i + name.len();
    }
    None
}

/// One level of container nesting while parsing.
enum Frame {
    Heading(u8),
    Code {
        lang: String,
        text: String,
    },
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
    Table(Table),
    Image {
        alt: String,
        url: String,
    },
}

/// Mutable parse state; tracks nesting so spans inherit the right style.
struct Builder {
    root: Vec<Block>,
    /// The byte offset of the source each block of `root` starts at.
    starts: Vec<usize>,
    /// Where the block now being read began.
    cur: usize,
    stack: Vec<Frame>,
    spans: Vec<Span>,
    link: Option<String>,
    bold: u32,
    italic: u32,
    strike: u32,
    sup: u32,
    sub: u32,
    /// Cells collected so far in the current table row.
    cells: Vec<Vec<Span>>,
    /// Whether the row being built is the header row.
    in_header: bool,
}

impl Builder {
    fn run<'a>(
        parser: impl Iterator<Item = (Event<'a>, std::ops::Range<usize>)>,
    ) -> (Vec<Block>, Vec<usize>) {
        let mut b = Builder {
            root: Vec::new(),
            starts: Vec::new(),
            cur: 0,
            stack: Vec::new(),
            spans: Vec::new(),
            link: None,
            bold: 0,
            italic: 0,
            strike: 0,
            sup: 0,
            sub: 0,
            cells: Vec::new(),
            in_header: false,
        };
        for (ev, range) in parser {
            // A block at the top level starts where its first event does.
            if b.stack.is_empty() {
                let begins = match &ev {
                    Event::Start(tag) => matches!(
                        tag,
                        Tag::Paragraph
                            | Tag::Heading { .. }
                            | Tag::CodeBlock(_)
                            | Tag::BlockQuote(_)
                            | Tag::List(_)
                            | Tag::FootnoteDefinition(_)
                            | Tag::Table(_)
                            | Tag::DefinitionList
                            | Tag::HtmlBlock
                    ),
                    Event::Rule | Event::Html(_) => true,
                    _ => false,
                };
                if begins {
                    b.cur = range.start;
                }
            }
            b.event(ev);
        }
        b.finish()
    }

    /// Adds a block to the top level, remembering where in the source it began.
    fn push_root(&mut self, block: Block) {
        self.root.push(block);
        self.starts.push(self.cur);
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(code) => {
                let mut span = Span::plain(&code);
                span.code = true;
                merge(&mut self.spans, span);
            }
            Event::SoftBreak => merge(&mut self.spans, Span::plain(" ")),
            Event::HardBreak => merge(&mut self.spans, Span::plain("\n")),
            // The checkbox is metadata, not text.
            Event::TaskListMarker(checked) => {
                if let Some(Frame::List { items, .. }) = self.stack.last_mut()
                    && let Some(item) = items.last_mut()
                {
                    item.checked = Some(checked);
                }
            }
            Event::Rule => self.push(Block::Rule),
            Event::Html(html) | Event::InlineHtml(html) => {
                let t = html.trim();
                if !t.is_empty() {
                    // A lone `<img>` tag is a picture, not source to be shown.
                    if let Some((alt, url, width, height)) = parse_img_tag(t) {
                        self.push(Block::Image {
                            alt,
                            url,
                            width,
                            height,
                        });
                    } else {
                        self.push(Block::Html(t.to_owned()));
                    }
                }
            }
            Event::FootnoteReference(label) => {
                merge(&mut self.spans, Span::plain(&format!("[{label}]")));
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.spans.clear();
            }
            Tag::Heading { level, .. } => {
                self.spans.clear();
                self.stack.push(Frame::Heading(level as u8));
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.stack.push(Frame::Code {
                    lang,
                    text: String::new(),
                });
            }
            Tag::BlockQuote(_) => self.stack.push(Frame::Quote(Vec::new())),
            Tag::List(start) => self.stack.push(Frame::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => {
                if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                    items.push(ListItem::default());
                }
            }
            Tag::FootnoteDefinition(label) => self.stack.push(Frame::Footnote {
                label: label.to_string(),
                blocks: Vec::new(),
            }),
            Tag::Table(aligns) => self.stack.push(Frame::Table(Table {
                aligns,
                ..Default::default()
            })),
            Tag::TableHead => {
                self.in_header = true;
            }
            Tag::TableRow => {
                self.cells.clear();
                self.in_header = false;
            }
            Tag::TableCell => self.spans.clear(),
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { dest_url, .. } => self.stack.push(Frame::Image {
                alt: String::new(),
                url: dest_url.to_string(),
            }),
            Tag::MetadataBlock(_) | Tag::HtmlBlock => {}
            Tag::Superscript => self.sup += 1,
            Tag::Subscript => self.sub += 1,
            // Definition lists render as a flat list: enough structure to read.
            Tag::DefinitionList => self.stack.push(Frame::List {
                start: None,
                items: Vec::new(),
            }),
            Tag::DefinitionListTitle | Tag::DefinitionListDefinition => self.spans.clear(),
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let spans = std::mem::take(&mut self.spans);
                // A paragraph that is only whitespace — the space a soft break leaves
                // between two images — is not worth a block of its own.
                if spans.iter().any(|s| !s.text.trim().is_empty()) {
                    self.push(Block::Para(spans));
                }
            }
            TagEnd::Heading(_) => {
                if let Some(Frame::Heading(level)) = self.stack.pop() {
                    let spans = std::mem::take(&mut self.spans);
                    self.push(Block::Heading { level, spans });
                }
            }
            TagEnd::CodeBlock => {
                if let Some(Frame::Code { lang, text }) = self.stack.pop() {
                    self.push(Block::Code { lang, text });
                }
            }
            TagEnd::BlockQuote(_) => {
                if let Some(Frame::Quote(blocks)) = self.stack.pop() {
                    self.push(Block::Quote(blocks));
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.push(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                // A tight list has no `Paragraph` tags: its text arrives as
                // bare `Text` events, so this is where it becomes a block.
                let spans = std::mem::take(&mut self.spans);
                if spans.is_empty() {
                    return;
                }
                if let Some(Frame::List { items, .. }) = self.stack.last_mut()
                    && let Some(item) = items.last_mut()
                {
                    // Some sources spell the marker out as literal text, so
                    // fall back to that when the event did not set one.
                    let mut spans = spans;
                    if item.checked.is_none() {
                        item.checked = take_task_marker(&mut spans);
                    }
                    if !spans.is_empty() {
                        item.blocks.push(Block::Para(spans));
                    }
                } else {
                    self.push(Block::Para(spans));
                }
            }
            TagEnd::FootnoteDefinition => {
                if let Some(Frame::Footnote { label, blocks }) = self.stack.pop() {
                    self.push(Block::Footnote { label, blocks });
                }
            }
            TagEnd::Table => {
                if let Some(Frame::Table(table)) = self.stack.pop() {
                    self.push(Block::Table(table));
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.spans);
                self.cells.push(cell);
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                let row = std::mem::take(&mut self.cells);
                let header = self.in_header;
                if let Some(Frame::Table(table)) = self.stack.last_mut() {
                    if header {
                        table.header = row;
                    } else {
                        table.rows.push(row);
                    }
                }
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Superscript => self.sup = self.sup.saturating_sub(1),
            TagEnd::Subscript => self.sub = self.sub.saturating_sub(1),
            TagEnd::Link => self.link = None,
            TagEnd::Image => {
                if let Some(Frame::Image { alt, url }) = self.stack.pop() {
                    let alt = if alt.is_empty() { url.clone() } else { alt };
                    self.push(Block::Image {
                        alt,
                        url,
                        width: None,
                        height: None,
                    });
                }
            }
            TagEnd::MetadataBlock(_) | TagEnd::HtmlBlock => {}
            TagEnd::DefinitionList => {
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.push(Block::List { start, items });
                }
            }
            TagEnd::DefinitionListTitle | TagEnd::DefinitionListDefinition => {
                let spans = std::mem::take(&mut self.spans);
                if spans.is_empty() {
                    return;
                }
                let para = Block::Para(spans);
                match self.stack.last_mut() {
                    Some(Frame::List { items, .. }) => items.push(ListItem {
                        checked: None,
                        blocks: vec![para],
                    }),
                    _ => self.push(para),
                }
            }
        }
    }

    fn text(&mut self, text: &CowStr<'_>) {
        if let Some(Frame::Code { text: buf, .. }) = self.stack.last_mut() {
            buf.push_str(text.as_ref());
            return;
        }
        if let Some(Frame::Image { alt, .. }) = self.stack.last_mut() {
            alt.push_str(text.as_ref());
            return;
        }
        let mut span = Span::plain(text);
        span.bold = self.bold > 0;
        span.italic = self.italic > 0;
        span.strike = self.strike > 0;
        span.sup = self.sup > 0;
        span.sub = self.sub > 0;
        span.link = self.link.clone();
        merge(&mut self.spans, span);
    }

    /// Appends a finished block to the innermost container, or to the root.
    fn push(&mut self, block: Block) {
        match self.stack.last_mut() {
            None => self.push_root(block),
            Some(Frame::Quote(blocks)) => blocks.push(block),
            Some(Frame::Footnote { blocks, .. }) => blocks.push(block),
            Some(Frame::List { items, .. }) => {
                if let Some(item) = items.last_mut() {
                    item.blocks.push(block);
                } else {
                    items.push(ListItem {
                        checked: None,
                        blocks: vec![block],
                    });
                }
            }
            Some(_) => self.push_root(block),
        }
    }

    fn finish(mut self) -> (Vec<Block>, Vec<usize>) {
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Heading(level) => {
                    let spans = std::mem::take(&mut self.spans);
                    if !spans.is_empty() {
                        self.push_root(Block::Heading { level, spans });
                    }
                }
                Frame::Code { lang, text } => self.push_root(Block::Code { lang, text }),
                Frame::Quote(blocks) | Frame::Footnote { blocks, .. } => {
                    for block in blocks {
                        self.push_root(block);
                    }
                }
                Frame::List { start, items } => self.push_root(Block::List { start, items }),
                Frame::Table(table) => self.push_root(Block::Table(table)),
                Frame::Image { alt, url } => self.push_root(Block::Image {
                    alt,
                    url,
                    width: None,
                    height: None,
                }),
            }
        }
        if !self.spans.is_empty() {
            let para = Block::Para(std::mem::take(&mut self.spans));
            self.push_root(para);
        }
        (self.root, self.starts)
    }
}

/// Merges adjacent spans sharing a style, keeping layout jobs small.
/// Removes a leading task-list marker (`[x] ` / `[ ] `) and reports its state.
fn take_task_marker(spans: &mut Vec<Span>) -> Option<bool> {
    let first = spans.first_mut()?;
    let lead = first.text.len() - first.text.trim_start().len();
    let rest = &first.text[lead..];
    let lower = rest.to_ascii_lowercase();
    let checked = if lower.starts_with("[x]") {
        true
    } else if lower.starts_with("[ ]") {
        false
    } else {
        return None;
    };
    let mut end = lead + 3;
    if first.text[end..].starts_with(' ') {
        end += 1;
    }
    first.text.drain(..end);
    if first.text.is_empty() {
        spans.remove(0);
    }
    Some(checked)
}

fn merge(spans: &mut Vec<Span>, next: Span) {
    if let Some(last) = spans.last_mut()
        && last.bold == next.bold
        && last.italic == next.italic
        && last.code == next.code
        && last.strike == next.strike
        && last.sup == next.sup
        && last.sub == next.sub
        && last.link == next.link
    {
        last.text.push_str(&next.text);
        return;
    }
    spans.push(next);
}

/// Shaped geometry for one block, reused every frame.
struct Shaped {
    galley: Arc<Galley>,
    /// Clickable link targets, relative to the galley origin.
    link_rects: Vec<(Rect, String)>,
}

/// Timing counters, shown in the status bar while typing.
#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub parse_us: u128,
    pub shape_ms: f32,
    pub blocks: usize,
}

/// A document parsed on another thread: the version it was parsed for, its blocks, and how
/// long the parse took in microseconds.
type Parsed = (u64, Vec<Block>, Vec<usize>, usize, u128);

/// A parsed document plus its shaping cache.
pub struct Preview {
    blocks: Vec<Block>,
    /// The line of the source each block starts on, and how many lines there are.
    lines: Vec<usize>,
    source_lines: usize,
    cache: HashMap<u64, Shaped>,
    /// Pixel dimensions of local images, so an image block has a real height on the
    /// first frame rather than changing size once its texture is decoded. `None`
    /// means the file could not be read as an image.
    dims: HashMap<PathBuf, Option<(u32, u32)>>,
    /// Which `<details>` disclosures the reader has opened, by block key.
    details_open: HashMap<u64, bool>,
    /// The exact height of each top-level block. Every block is laid out before the
    /// document is drawn, so the map below is complete and does not move under the
    /// reader as blocks come into view. `None` only until the layout pass runs.
    heights: Vec<Option<f32>>,
    /// `tops[i]` is where block `i` starts, measured from the top of the document, so
    /// a source line and a pixel can each be found with a binary search. Rebuilt
    /// whenever a height changes.
    tops: Vec<f32>,
    /// Whether every height is known, so the map is exact and the sync can trust it.
    laid_out: bool,
    /// Document version the cache belongs to.
    version: Option<u64>,
    /// A parse running on another thread: where its answer will be, once it has one.
    pending: Option<Arc<std::sync::Mutex<Option<Parsed>>>>,
    /// Width the cache was built for.
    width: f32,
    stats: Stats,
}

impl Default for Preview {
    fn default() -> Self {
        Preview::new()
    }
}

impl Preview {
    pub fn new() -> Preview {
        Preview {
            blocks: Vec::new(),
            lines: Vec::new(),
            source_lines: 0,
            cache: HashMap::new(),
            dims: HashMap::new(),
            details_open: HashMap::new(),
            heights: Vec::new(),
            tops: Vec::new(),
            laid_out: false,
            version: None,
            pending: None,
            width: 0.0,
            stats: Stats::default(),
        }
    }

    /// Re-parses when the document version moved on.
    ///
    /// The heights of the blocks that did not change are kept. An edit usually
    /// touches one block, and starting every height over would put every block back
    /// to an estimate, moving the whole document under a reader who is looking at
    /// it. The blocks are compared from both ends, and only the stretch in the middle
    /// that differs has to be measured again.
    pub fn sync(&mut self, text: &str, version: u64) {
        if self.version == Some(version) {
            return;
        }
        let started = std::time::Instant::now();
        let (fresh, lines) = parse_with_lines(text);
        let total = text.matches('\n').count() + 1;
        self.install(fresh, lines, total, version, started.elapsed().as_micros());
    }

    /// [`Preview::sync`] for a document that may be large: one over a size that is
    /// parsed in a frame is parsed on another thread, and the previous version stays
    /// on screen until the new one is ready. The window is woken when it arrives.
    pub fn sync_in_background(&mut self, ctx: &egui::Context, text: &str, version: u64) {
        /// Parsing runs at tens of megabytes a second, so this is a few milliseconds.
        const IN_A_FRAME: usize = 64 * 1024;
        if text.len() <= IN_A_FRAME {
            self.pending = None;
            return self.sync(text, version);
        }
        if let Some(slot) = &self.pending
            && let Some((parsed, blocks, lines, total, us)) =
                slot.lock().ok().and_then(|mut g| g.take())
        {
            self.pending = None;
            self.install(blocks, lines, total, parsed, us);
        }
        if self.version == Some(version) || self.pending.is_some() {
            return;
        }
        let slot = Arc::new(std::sync::Mutex::new(None));
        self.pending = Some(Arc::clone(&slot));
        let source = text.to_owned();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let (blocks, lines) = parse_with_lines(&source);
            let total = source.matches('\n').count() + 1;
            if let Ok(mut g) = slot.lock() {
                *g = Some((version, blocks, lines, total, started.elapsed().as_micros()));
            }
            ctx.request_repaint();
        });
    }

    /// Takes a freshly parsed document in, keeping the heights of what did not change.
    fn install(
        &mut self,
        fresh: Vec<Block>,
        lines: Vec<usize>,
        total_lines: usize,
        version: u64,
        parse_us: u128,
    ) {
        self.stats.parse_us = parse_us;
        self.lines = lines;
        self.source_lines = total_lines;
        let old = std::mem::take(&mut self.blocks);
        let same_head = old.iter().zip(&fresh).take_while(|(a, b)| a == b).count();
        let room = old.len().min(fresh.len()) - same_head;
        let same_tail = old
            .iter()
            .rev()
            .zip(fresh.iter().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        let mut heights: Vec<Option<f32>> = vec![None; fresh.len()];
        for (i, h) in self.heights.iter().enumerate().take(same_head) {
            heights[i] = *h;
        }
        for k in 0..same_tail {
            heights[fresh.len() - 1 - k] = self.heights.get(old.len() - 1 - k).copied().flatten();
        }
        self.heights = heights;
        self.blocks = fresh;
        self.stats.blocks = self.blocks.len();
        self.cache.clear();
        self.version = Some(version);
        // The unchanged blocks keep their heights, so only the stretch that differs
        // has to be laid out again; the map is rebuilt from what is known.
        self.laid_out = self.heights.iter().all(Option::is_some);
        self.recompute_tops();
    }

    /// Forgets the parsed document, so the next `sync` rebuilds it.
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.lines.clear();
        self.source_lines = 0;
        self.heights.clear();
        self.tops.clear();
        self.laid_out = false;
        self.cache.clear();
        self.version = None;
        self.pending = None;
        self.width = 0.0;
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Whether every block has an exact height, so the line<->pixel map is exact.
    pub fn laid_out(&self) -> bool {
        self.laid_out
    }

    /// Rebuilds the block-start offsets from the current heights. The heights of
    /// blocks that have not been laid out yet fall back to an estimate, so the map
    /// still has a length while the layout pass runs.
    fn recompute_tops(&mut self) {
        let mut tops = Vec::with_capacity(self.blocks.len() + 1);
        let mut acc = 0.0f32;
        tops.push(0.0);
        for i in 0..self.blocks.len() {
            acc += self.block_height(i);
            tops.push(acc);
        }
        self.tops = tops;
    }

    /// How tall block `i` is: exactly, if it has been laid out, or an estimate until
    /// then.
    fn block_height(&self, i: usize) -> f32 {
        self.heights
            .get(i)
            .copied()
            .flatten()
            .unwrap_or_else(|| estimate(&self.blocks[i], self.width.max(120.0)))
    }

    /// How far down the preview, in points, a place in the source is: a line number,
    /// fractional for somewhere between two lines. Found through the block the line is
    /// in, by how far through that block's lines it is.
    pub fn y_of_line(&self, line: f32) -> f32 {
        if self.blocks.is_empty() || self.tops.len() <= 1 {
            return 0.0;
        }
        let line = line.max(0.0);
        let k = self
            .lines
            .partition_point(|&l| (l as f32) <= line)
            .saturating_sub(1)
            .min(self.blocks.len() - 1);
        let from = self.lines[k] as f32;
        let to = self
            .lines
            .get(k + 1)
            .copied()
            .unwrap_or(self.source_lines.max(self.lines[k] + 1)) as f32;
        let frac = ((line - from) / (to - from).max(1.0)).clamp(0.0, 1.0);
        self.tops[k] + frac * self.block_height(k)
    }

    /// The line of the source that is `y` points down the preview: the opposite of
    /// [`Preview::y_of_line`].
    pub fn line_at_y(&self, y: f32) -> f32 {
        if self.blocks.is_empty() || self.tops.len() <= 1 {
            return 0.0;
        }
        let y = y.max(0.0);
        let k = self
            .tops
            .partition_point(|&t| t <= y)
            .saturating_sub(1)
            .min(self.blocks.len() - 1);
        let h = self.block_height(k).max(0.001);
        let frac = ((y - self.tops[k]) / h).clamp(0.0, 1.0);
        let from = self.lines[k] as f32;
        let to = self
            .lines
            .get(k + 1)
            .copied()
            .unwrap_or(self.source_lines.max(self.lines[k] + 1)) as f32;
        from + frac * (to - from).max(1.0)
    }

    /// Renders the document with no image caches: the plain entry point used by
    /// tests and by any caller that has none.
    #[cfg(test)]
    pub fn show(&mut self, ui: &mut Ui, indent: f32) -> f32 {
        self.show_ctx(ui, indent, None, None, None)
    }

    /// Renders the document, returning its total height.
    ///
    /// Every block is laid out once, before anything is drawn, so the map from source
    /// lines to pixels is exact and never moves under the reader; only the blocks near
    /// the view are painted, so a frame still costs what is on screen and not what is
    /// in the file. Blocks are painted at absolute positions, so the origin comes from
    /// the `Ui` itself.
    ///
    /// `thumbs` and `remote` are the image caches, and `base` is the directory the
    /// Markdown file lives in, which relative image paths resolve against.
    pub fn show_ctx(
        &mut self,
        ui: &mut Ui,
        indent: f32,
        mut thumbs: Option<&mut thumbs::Thumbs>,
        mut remote: Option<&mut crate::remote::RemoteImages>,
        base: Option<&Path>,
    ) -> f32 {
        let avail = ui.available_width();
        let width = (avail - indent).clamp(120.0, sp::MD_MEASURE);
        if (self.width - width).abs() > 0.5 {
            // A different width wraps everything differently, so every height is stale.
            self.cache.clear();
            self.heights.fill(None);
            self.width = width;
            self.laid_out = false;
        }
        // Lay out every block that has no height yet, so the map below is exact.
        if !self.laid_out {
            self.layout_all(ui, indent, width, &mut thumbs, &mut remote, base);
        }

        let origin = ui.min_rect().min;
        let x0 = origin.x + indent;
        let clip = ui.clip_rect();
        // Painted a screenful either side of the view, so a block is ready before it
        // arrives and a fast scroll does not show a gap.
        let reach = clip.height().max(200.0);
        let (top, bottom) = (clip.top() - reach, clip.bottom() + reach);
        let mut hits: Vec<(Rect, String)> = Vec::new();
        // Only the blocks in the band are painted; their positions come from `tops`.
        let len = self.blocks.len();
        if len > 0 {
            let first = self
                .tops
                .partition_point(|&t| origin.y + t < top)
                .saturating_sub(1)
                .min(len - 1);
            let mut changed = false;
            let blocks = std::mem::take(&mut self.blocks);
            for (i, block) in blocks.iter().enumerate().skip(first) {
                let at = origin.y + self.tops[i];
                if at > bottom {
                    break;
                }
                let h = self.block(
                    ui,
                    key_of(0, i),
                    block,
                    x0,
                    at,
                    width,
                    0,
                    &mut hits,
                    &mut thumbs,
                    &mut remote,
                    base,
                    c::TEXT,
                    true,
                ) - at;
                // A block whose state changed under us — an image finished loading, a
                // `<details>` was opened — has a new height, so the map is rebuilt.
                if (h - self.heights[i].unwrap_or(h)).abs() > 0.5 {
                    self.heights[i] = Some(h);
                    changed = true;
                }
            }
            self.blocks = blocks;
            if changed {
                self.recompute_tops();
                ui.ctx().request_repaint();
            }
        }

        // Open a link when it is clicked with the pointer still on it.
        let (hover, released, press_origin) = ui.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.any_released(),
                i.pointer.press_origin(),
            )
        });
        if let (Some(pos), true, Some(origin)) = (hover, released, press_origin)
            && (pos - origin).length() < 5.0
            && let Some((_, url)) = hits.iter().find(|(r, _)| r.contains(pos))
        {
            let url = url.clone();
            let _ = open::that(&url);
        }
        // A height, not an absolute y, so the caller can size the content.
        self.tops.last().copied().unwrap_or(0.0)
    }

    /// Lays out every block that has no height yet, so the line<->pixel map is exact.
    ///
    /// Nothing is painted: only the geometry is wanted. An image block uses the size it
    /// already knows — a local file's pixels, or a short placeholder for a remote one —
    /// rather than starting a fetch, so a document full of pictures does not decode
    /// them all at once.
    fn layout_all(
        &mut self,
        ui: &mut Ui,
        indent: f32,
        width: f32,
        thumbs: &mut Option<&mut thumbs::Thumbs>,
        remote: &mut Option<&mut crate::remote::RemoteImages>,
        base: Option<&Path>,
    ) {
        let blocks = std::mem::take(&mut self.blocks);
        let mut hits: Vec<(Rect, String)> = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            if self.heights.get(i).copied().flatten().is_some() {
                continue;
            }
            let h = self.block(
                ui,
                key_of(0, i),
                block,
                indent,
                0.0,
                width,
                0,
                &mut hits,
                thumbs,
                remote,
                base,
                c::TEXT,
                false,
            );
            self.heights[i] = Some(h);
        }
        self.blocks = blocks;
        self.laid_out = self.heights.iter().all(Option::is_some);
        self.recompute_tops();
    }

    /// Lays out one block, returning the y cursor after it. When `draw` is false only
    /// the geometry is computed: nothing is painted and no input is taken.
    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        ui: &mut Ui,
        key: u64,
        block: &Block,
        indent: f32,
        y: f32,
        width: f32,
        depth: u32,
        hits: &mut Vec<(Rect, String)>,
        thumbs: &mut Option<&mut thumbs::Thumbs>,
        remote: &mut Option<&mut crate::remote::RemoteImages>,
        base: Option<&Path>,
        text: Color32,
        draw: bool,
    ) -> f32 {
        if depth > 6 {
            return y;
        }
        match block {
            Block::Heading { level, spans } => {
                let size = heading_size(*level);
                // A heading in a blockquote takes the quote's dimmed colour.
                let color = if text == c::TEXT {
                    heading_color(*level)
                } else {
                    text
                };
                let level = *level;
                let make = || {
                    inline_job(spans, |span| {
                        let mut fmt = egui::text::TextFormat::simple(bold_font(size), color);
                        if span.italic {
                            fmt.font_id =
                                egui::FontId::new(size, FontFamily::Name(FAMILY_UI.into()));
                            fmt.italics = true;
                        }
                        if level == 1 {
                            fmt.extra_letter_spacing = 0.15;
                        }
                        raise_or_lower(&mut fmt, span);
                        fmt
                    })
                };
                let h = self.paint(ui, key, make, indent, y, width, hits, draw);
                if level <= 2 {
                    let line_y = (y + h + sp::XS).round();
                    if draw {
                        ui.painter().hline(
                            indent..=(indent + width.min(600.0)),
                            line_y,
                            Stroke::new(1.0, c::DIVIDER),
                        );
                    }
                    line_y + sp::SM
                } else {
                    y + h + sp::XS
                }
            }
            Block::Para(spans) => {
                let h = self.paint(
                    ui,
                    key,
                    || inline_job(spans, |span| body_format(span, text)),
                    indent,
                    y,
                    width,
                    hits,
                    draw,
                );
                y + h + sp::SM
            }
            Block::Code { lang, text: code } => {
                let pad = Vec2::new(sp::MD, sp::SM);
                let galley = self.shape(ui, key, (width - pad.x * 2.0).max(40.0), draw, || {
                    (code_job(code, lang), Vec::new())
                });
                let rect = Rect::from_min_size(
                    Pos2::new(indent, y),
                    Vec2::new(width, galley.size().y + pad.y * 2.0),
                );
                if draw {
                    let painter = ui.painter();
                    painter.rect_filled(rect, 5, c::CODE_BG);
                    painter.rect_stroke(rect, 5, Stroke::new(1.0, c::DIVIDER), StrokeKind::Inside);
                    if !lang.is_empty() {
                        painter.text(
                            rect.right_top() + Vec2::new(-sp::SM, 5.0),
                            egui::Align2::RIGHT_TOP,
                            lang,
                            ui_font(tfs::SMALL),
                            c::TEXT_GHOST,
                        );
                    }
                    crate::widgets::galley_at(painter, rect.min + pad, &galley, c::TEXT);
                }
                rect.max.y + sp::SM
            }
            Block::Quote(blocks) => {
                let inner_x = indent + sp::MD;
                let inner_w = (width - sp::MD).max(40.0);
                let start = y;
                let mut y = y;
                for (i, b) in blocks.iter().enumerate() {
                    y = self.block(
                        ui,
                        child(key, i),
                        b,
                        inner_x,
                        y,
                        inner_w,
                        depth + 1,
                        hits,
                        thumbs,
                        remote,
                        base,
                        c::TEXT_DIM,
                        draw,
                    );
                }
                if draw && y > start + 1.0 {
                    ui.painter().vline(
                        indent,
                        start..=(y - sp::SM).max(start),
                        Stroke::new(3.0, c::BORDER),
                    );
                }
                y
            }
            Block::List { start, items } => {
                let gutter = sp::MD + sp::SM;
                let text_x = indent + gutter;
                let text_w = (width - gutter).max(40.0);
                let mut y = y;
                for (n, item) in items.iter().enumerate() {
                    if draw {
                        // A task item shows a checkbox instead of a bullet.
                        if item.checked.is_none() {
                            let marker = match start {
                                Some(first) => format!("{}.", first + n as u64),
                                None => "\u{2022}".to_owned(),
                            };
                            let marker_font = if start.is_some() {
                                mono_font(tfs::SMALL)
                            } else {
                                bold_font(tfs::BODY)
                            };
                            ui.painter().text(
                                Pos2::new(indent + 2.0, y + 1.0),
                                egui::Align2::LEFT_TOP,
                                marker,
                                marker_font,
                                c::TEXT_FAINT,
                            );
                        }
                        if let Some(checked) = item.checked {
                            let s = 11.0f32;
                            let rect = Rect::from_min_size(
                                Pos2::new(indent + 1.0, y + 3.0),
                                Vec2::splat(s),
                            );
                            let painter = ui.painter();
                            let stroke =
                                Stroke::new(1.0, if checked { c::TEXT_DIM } else { c::BORDER });
                            painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
                            if checked {
                                painter.line_segment(
                                    [
                                        rect.left_top() + Vec2::new(2.0, 5.5),
                                        rect.left_top() + Vec2::new(4.5, 8.5),
                                    ],
                                    Stroke::new(1.3, c::TEXT_DIM),
                                );
                                painter.line_segment(
                                    [
                                        rect.left_top() + Vec2::new(4.5, 8.5),
                                        rect.right_top() + Vec2::new(-1.5, 2.5),
                                    ],
                                    Stroke::new(1.3, c::TEXT_DIM),
                                );
                            }
                        }
                    }
                    for (i, b) in item.blocks.iter().enumerate() {
                        y = self.block(
                            ui,
                            child(child(key, n), i),
                            b,
                            text_x,
                            y,
                            text_w,
                            depth + 1,
                            hits,
                            thumbs,
                            remote,
                            base,
                            text,
                            draw,
                        );
                    }
                    y += sp::XS;
                }
                y + sp::XS
            }
            Block::Table(table) => self.table(ui, key, table, indent, y, width, text, draw),
            Block::Rule => {
                let line_y = (y + sp::SM).round();
                if draw {
                    ui.painter().hline(
                        indent..=(indent + width),
                        line_y,
                        Stroke::new(1.0, c::DIVIDER),
                    );
                }
                line_y + sp::SM
            }
            Block::Html(html) => {
                let make = || {
                    let job = plain_job(
                        html.clone(),
                        egui::text::TextFormat::simple(mono_font(tfs::MONO), c::TEXT_GHOST),
                    );
                    (job, Vec::new())
                };
                let h = self.paint(ui, key, make, indent, y, width, hits, draw);
                y + h + sp::SM
            }
            Block::Image {
                alt,
                url,
                width: aw,
                height: ah,
            } => self.image(
                ui, key, alt, url, *aw, *ah, indent, y, width, hits, thumbs, remote, base, draw,
            ),
            Block::Details {
                summary,
                blocks,
                open,
            } => {
                let expanded = *self.details_open.entry(key).or_insert(*open);
                // Shape the summary first, so the hover background can be drawn
                // beneath it rather than over it.
                let g = self.shape(
                    ui,
                    child(key, usize::MAX),
                    (width - sp::LG).max(40.0),
                    draw,
                    || {
                        (
                            plain_job(
                                summary.clone(),
                                egui::text::TextFormat::simple(bold_font(tfs::BODY), text),
                            ),
                            Vec::new(),
                        )
                    },
                );
                let head = g.size().y;
                let head_rect =
                    Rect::from_min_size(Pos2::new(indent, y), Vec2::new(width, head + sp::XS));
                if draw {
                    let resp = ui.interact(head_rect, Id::new(("md-details", key)), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(head_rect, 4, c::HOVER);
                    }
                    crate::widgets::Icon::Chevron.paint_with(
                        ui.painter(),
                        Rect::from_center_size(
                            Pos2::new(indent + 6.0, y + head * 0.5),
                            Vec2::splat(12.0),
                        ),
                        if resp.hovered() { c::TEXT } else { c::TEXT_DIM },
                        expanded,
                    );
                    crate::widgets::galley_at(
                        ui.painter(),
                        Pos2::new(indent + sp::LG, y),
                        &g,
                        text,
                    );
                    if resp.clicked() {
                        self.details_open.insert(key, !expanded);
                        ui.ctx().request_repaint();
                    }
                }
                let mut y = y + head + sp::XS;
                if expanded {
                    for (i, b) in blocks.iter().enumerate() {
                        y = self.block(
                            ui,
                            child(key, i),
                            b,
                            indent + sp::LG,
                            y,
                            (width - sp::LG).max(40.0),
                            depth + 1,
                            hits,
                            thumbs,
                            remote,
                            base,
                            text,
                            draw,
                        );
                    }
                }
                y + sp::SM
            }
            Block::Footnote { label, blocks } => {
                let make = || {
                    let job = plain_job(
                        format!("[{label}]"),
                        egui::text::TextFormat::simple(ui_font(tfs::SMALL), c::TEXT_FAINT),
                    );
                    (job, Vec::new())
                };
                let mut y = y + self.paint(ui, key, make, indent, y, width, hits, draw);
                for (i, b) in blocks.iter().enumerate() {
                    y = self.block(
                        ui,
                        child(key, i),
                        b,
                        indent + sp::MD,
                        y,
                        (width - sp::MD).max(40.0),
                        depth + 1,
                        hits,
                        thumbs,
                        remote,
                        base,
                        text,
                        draw,
                    );
                }
                y
            }
        }
    }

    /// Table layout, GitHub style: padded cells, a tinted header row and a border
    /// around every cell. Column widths follow the content, then share the room.
    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        ui: &mut Ui,
        key: u64,
        table: &Table,
        indent: f32,
        y: f32,
        width: f32,
        text: Color32,
        draw: bool,
    ) -> f32 {
        // The column count is the widest row: a header cell holds spans, not
        // columns, so counting `header.first()` would count styled runs.
        let cols = table
            .header
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return y;
        }
        let pad = Vec2::new(sp::SM, 5.0);
        let col_w = table_widths(table, cols, width, pad.x);
        // Column left edges, kept so the vertical rules can be drawn afterwards.
        let mut edges = Vec::with_capacity(cols + 1);
        let mut x = indent;
        for w in &col_w {
            edges.push(x);
            x += w;
        }
        edges.push(x);
        let (left, right) = (edges[0], *edges.last().unwrap());

        let mut y = y;
        let mut rules: Vec<f32> = Vec::with_capacity(table.rows.len() + 2);
        rules.push(y);
        // Header: measured first, so its background can be painted beneath the text.
        if !table.header.is_empty() {
            let mut cells = Vec::with_capacity(cols);
            let mut h = 0.0f32;
            for (ci, cell) in table.header.iter().enumerate().take(cols) {
                let g = self.cell_shape(
                    ui,
                    table_key(key, 0, ci),
                    cell,
                    col_w[ci] - pad.x * 2.0,
                    true,
                    text,
                    draw,
                );
                h = h.max(g.size().y);
                cells.push(g);
            }
            let row = Rect::from_min_max(Pos2::new(left, y), Pos2::new(right, y + h + pad.y * 2.0));
            if draw {
                ui.painter().rect_filled(row, 0, c::RAISED);
                for (ci, g) in cells.iter().enumerate() {
                    let align = table.aligns.get(ci).copied().unwrap_or(Alignment::None);
                    cell_paint(
                        ui.painter(),
                        g,
                        edges[ci] + pad.x,
                        y + pad.y,
                        col_w[ci] - pad.x * 2.0,
                        align,
                        if text == c::TEXT { c::TEXT_DIM } else { text },
                    );
                }
            }
            y = row.max.y;
            rules.push(y);
        }
        for (ri, row) in table.rows.iter().enumerate() {
            let mut cells = Vec::with_capacity(cols);
            let mut h = 0.0f32;
            for (ci, cell) in row.iter().enumerate().take(cols) {
                let g = self.cell_shape(
                    ui,
                    table_key(key, ri + 1, ci),
                    cell,
                    col_w[ci] - pad.x * 2.0,
                    false,
                    text,
                    draw,
                );
                h = h.max(g.size().y);
                cells.push(g);
            }
            if draw {
                for (ci, g) in cells.iter().enumerate() {
                    let align = table.aligns.get(ci).copied().unwrap_or(Alignment::None);
                    cell_paint(
                        ui.painter(),
                        g,
                        edges[ci] + pad.x,
                        y + pad.y,
                        col_w[ci] - pad.x * 2.0,
                        align,
                        text,
                    );
                }
            }
            y += h + pad.y * 2.0;
            rules.push(y);
        }
        // The grid: a rule under every row, a rule between every column, and a box.
        if draw {
            let stroke = Stroke::new(1.0, c::BORDER);
            let painter = ui.painter();
            for line in &rules {
                painter.hline(left..=right, *line, stroke);
            }
            let bottom = *rules.last().unwrap();
            for edge in &edges {
                painter.vline(*edge, rules[0]..=bottom, stroke);
            }
        }
        y + sp::SM
    }

    /// Shapes one table cell and returns its galley.
    #[allow(clippy::too_many_arguments)]
    fn cell_shape(
        &mut self,
        ui: &mut Ui,
        key: u64,
        spans: &[Span],
        w: f32,
        header: bool,
        text: Color32,
        cache: bool,
    ) -> Arc<Galley> {
        let color = if header && text == c::TEXT {
            c::TEXT_DIM
        } else {
            text
        };
        let font = if header {
            bold_font(tfs::SMALL)
        } else {
            ui_font(tfs::BODY)
        };
        let bold = if header {
            bold_font(tfs::SMALL)
        } else {
            font.clone()
        };
        self.shape(ui, key, w.max(20.0), cache, || {
            inline_job(spans, move |span| {
                if span.code {
                    let mut fmt = egui::text::TextFormat::simple(mono_font(tfs::MONO), text);
                    fmt.background = c::CODE_BG;
                    fmt.expand_bg = 2.0;
                    fmt
                } else if span.bold {
                    egui::text::TextFormat::simple(bold.clone(), color)
                } else {
                    egui::text::TextFormat::simple(font.clone(), color)
                }
            })
        })
    }

    /// Draws an image — local or remote — or a tidy reference when it cannot be
    /// decoded. In layout-only mode (`draw` false) the height is computed without
    /// painting and without starting a fetch, using the size already known.
    ///
    /// `attr_w`/`attr_h` are a raw `<img>` tag's own width and height, which win over
    /// the picture's natural size the way a browser honours them.
    #[allow(clippy::too_many_arguments)]
    fn image(
        &mut self,
        ui: &mut Ui,
        key: u64,
        alt: &str,
        url: &str,
        attr_w: Option<f32>,
        attr_h: Option<f32>,
        indent: f32,
        y: f32,
        width: f32,
        hits: &mut Vec<(Rect, String)>,
        thumbs: &mut Option<&mut thumbs::Thumbs>,
        remote: &mut Option<&mut crate::remote::RemoteImages>,
        base: Option<&Path>,
        draw: bool,
    ) -> f32 {
        // A web URL is fetched on a worker; a local path is decoded from disk.
        if let Some(url) = remote_url(url)
            && let Some(remote) = remote.as_deref_mut()
            && !remote.is_failed(&url)
        {
            // The size the texture will have, from the one already cached or from the
            // tag's own attributes; a short placeholder while it is still on its way.
            let natural = remote.peek(&url).map(|tex| tex.size_vec2());
            let size = if attr_w.is_some() || attr_h.is_some() {
                Some(fit_size(
                    natural.unwrap_or(Vec2::new(width, 20.0)),
                    attr_w,
                    attr_h,
                    width,
                ))
            } else {
                natural.map(|n| fit_size(n, None, None, width))
            };
            let h = size.map_or(20.0, |s| s.y);
            if draw {
                let ppp = ui.ctx().pixels_per_point();
                let px = thumbs::bucket(width, ppp);
                match remote.get(&url, px) {
                    Some(tex) => {
                        let size = fit_size(tex.size_vec2(), attr_w, attr_h, width);
                        let rect = Rect::from_min_size(Pos2::new(indent, y), size);
                        egui::Image::new(&tex)
                            .fit_to_exact_size(size)
                            .paint_at(ui, rect);
                        return y + size.y + sp::SM;
                    }
                    None => {
                        // Not fetched yet: a short placeholder, since most remote
                        // images in a README are badges. Asking for a repaint is
                        // what makes it appear without further input.
                        let rect = Rect::from_min_size(
                            Pos2::new(indent, y),
                            Vec2::new(width.min(160.0), h),
                        );
                        ui.painter().rect_filled(rect, 3, c::CODE_BG);
                        ui.ctx().request_repaint();
                        return y + h + sp::SM;
                    }
                }
            }
            return y + h + sp::SM;
        }
        let path = local_image_path(url, base);
        let dims = path.as_deref().and_then(|p| self.image_dims(p));
        if let (Some(path), Some((w, h))) = (path, dims)
            && w > 0
            && h > 0
        {
            let size = fit_size(Vec2::new(w as f32, h as f32), attr_w, attr_h, width);
            if !draw {
                return y + size.y + sp::SM;
            }
            if let Some(thumbs) = thumbs.as_deref_mut() {
                let ppp = ui.ctx().pixels_per_point();
                let rect = Rect::from_min_size(Pos2::new(indent, y), size);
                match thumbs.get(&path, thumbs::bucket(size.x, ppp)) {
                    Some(tex) => {
                        egui::Image::new(&tex)
                            .fit_to_exact_size(size)
                            .paint_at(ui, rect);
                    }
                    // Known bad: fall through to the reference line below.
                    None if thumbs.is_failed(&path) => {}
                    None => {
                        // The decode is still running: hold the space at the right
                        // size with a quiet placeholder. Ask for another frame, since
                        // the decode finishing does not wake the window on its own.
                        ui.painter().rect_filled(rect, 4, c::CODE_BG);
                        if !alt.is_empty() {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                alt,
                                ui_font(tfs::SMALL),
                                c::TEXT_FAINT,
                            );
                        }
                        ui.ctx().request_repaint();
                    }
                }
                return y + size.y + sp::SM;
            }
        }
        // Not an image we can draw: keep the source honest with a reference line.
        let make = || {
            let label = if alt.is_empty() {
                url.to_owned()
            } else {
                format!("{alt}  \u{2014}  {url}")
            };
            let job = plain_job(
                label,
                egui::text::TextFormat::simple(ui_font(tfs::SMALL), c::TEXT_FAINT),
            );
            (job, Vec::new())
        };
        let h = self.paint(ui, key, make, indent, y, width, hits, draw);
        y + h + sp::SM
    }

    /// The pixel size of a local image, read once and remembered.
    fn image_dims(&mut self, path: &Path) -> Option<(u32, u32)> {
        if let Some(hit) = self.dims.get(path) {
            return *hit;
        }
        let dims = image::image_dimensions(path).ok();
        self.dims.insert(path.to_path_buf(), dims);
        dims
    }

    /// Shapes a job at `width`, caching the result when `cache` is set.
    ///
    /// The job is built by `make`, and only when it is needed: a block that is
    /// already shaped costs a lookup and nothing else, where building its job first
    /// — a string and a section for every run of text — cost more than the lookup it
    /// was for, on every block, every frame. The layout pass passes `cache` false, so
    /// measuring a whole document does not hold every galley in memory.
    fn shape(
        &mut self,
        ui: &mut Ui,
        key: u64,
        width: f32,
        cache: bool,
        make: impl FnOnce() -> (LayoutJob, Vec<(usize, String)>),
    ) -> Arc<Galley> {
        if cache && let Some(s) = self.cache.get(&key) {
            return s.galley.clone();
        }
        let (job, links) = make();
        let started = std::time::Instant::now();
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(wrap(job, width)));
        self.stats.shape_ms = started.elapsed().as_secs_f32() * 1000.0;
        if !cache {
            return galley;
        }

        // Clickable link geometry comes from the shaped text. Inline code
        // backgrounds are handled by `TextFormat::background`, so nothing else
        // has to be painted here.
        let text = &galley.job.text;
        let mut link_rects = Vec::new();
        for (i, section) in galley.job.sections.iter().enumerate() {
            let Some((_, url)) = links.iter().find(|(idx, _)| *idx == i) else {
                continue;
            };
            let start: usize = section.byte_range.start.into();
            let end: usize = section.byte_range.end.into();
            if end > text.len() || start > end {
                continue;
            }
            let start_char = text[..start].chars().count();
            let end_char = start_char + text[start..end].chars().count();
            let rect = range_rect(&galley, start_char, end_char);
            if rect.width() > 0.0 {
                link_rects.push((rect, url.clone()));
            }
        }

        self.cache.insert(
            key,
            Shaped {
                galley: galley.clone(),
                link_rects,
            },
        );
        galley
    }

    /// Shapes, paints and returns the height of a text block. With `draw` false only
    /// the height is returned.
    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        ui: &mut Ui,
        key: u64,
        make: impl FnOnce() -> (LayoutJob, Vec<(usize, String)>),
        indent: f32,
        y: f32,
        width: f32,
        hits: &mut Vec<(Rect, String)>,
        draw: bool,
    ) -> f32 {
        let galley = self.shape(ui, key, width, draw, make);
        let height = galley.size().y;
        if draw {
            let pos = Pos2::new(indent, y);
            if let Some(shaped) = self.cache.get(&key) {
                for (r, url) in &shaped.link_rects {
                    hits.push((r.translate(pos.to_vec2()), url.clone()));
                }
            }
            crate::widgets::galley_at(ui.painter(), pos, &galley, c::TEXT);
        }
        height
    }
}

/// Draws a shaped cell at `x`,`y`, aligned within `w`.
fn cell_paint(
    painter: &egui::Painter,
    galley: &Arc<Galley>,
    x: f32,
    y: f32,
    w: f32,
    align: Alignment,
    color: Color32,
) {
    let size = galley.size();
    let dx = match align {
        Alignment::Right => (w - size.x).max(0.0),
        Alignment::Center => ((w - size.x) / 2.0).max(0.0),
        _ => 0.0,
    };
    crate::widgets::galley_at(
        painter,
        Pos2::new((x + dx).round(), y.round()),
        galley,
        color,
    );
}

/// The size to draw an image at: the tag's own width/height when it has them, else
/// the picture's natural size. Either way it is never wider than the measure; a
/// small image is left at its own size, only an oversized one is shrunk.
fn fit_size(nat: Vec2, attr_w: Option<f32>, attr_h: Option<f32>, measure: f32) -> Vec2 {
    let cap = |w: f32, h: f32| {
        let s = (measure / w.max(1.0)).min(1.0);
        Vec2::new((w * s).max(1.0), (h * s).max(1.0))
    };
    match (attr_w, attr_h) {
        (Some(w), Some(h)) => cap(w, h),
        (Some(w), None) => cap(w, w * nat.y / nat.x.max(1.0)),
        (None, Some(h)) => cap(h * nat.x / nat.y.max(1.0), h),
        (None, None) => {
            let s = (measure / nat.x.max(1.0)).min(1.0);
            Vec2::new((nat.x * s).max(1.0), (nat.y * s).max(1.0))
        }
    }
}

/// Column widths for a table: the room shared in proportion to how much text each
/// column holds, with a floor so a narrow column stays readable.
fn table_widths(table: &Table, cols: usize, width: f32, pad_x: f32) -> Vec<f32> {
    let mut weight = vec![3.0f32; cols];
    let measure = |cells: &[Vec<Span>], weight: &mut Vec<f32>| {
        for (ci, cell) in cells.iter().enumerate().take(cols) {
            let chars: usize = cell.iter().map(|s| s.text.chars().count()).sum();
            weight[ci] = weight[ci].max(chars as f32);
        }
    };
    measure(&table.header, &mut weight);
    for row in &table.rows {
        measure(row, &mut weight);
    }
    let total: f32 = weight.iter().sum::<f32>().max(1.0);
    let inner = (width - cols as f32 * pad_x * 2.0).max(cols as f32 * 20.0);
    let mut out: Vec<f32> = weight
        .iter()
        .map(|w| (inner * w / total).max(20.0) + pad_x * 2.0)
        .collect();
    let sum: f32 = out.iter().sum();
    if sum > width {
        let k = width / sum;
        for w in &mut out {
            *w *= k;
        }
    }
    out
}

/// The URL of a web image, if `url` names one.
fn remote_url(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(url.to_owned())
    } else {
        None
    }
}

/// Resolves an image URL to a local file, if it names one: relative paths are
/// joined to the Markdown file's directory, a `file:` URL is taken as a path, and
/// anything with a web scheme or a data URL is not a local file.
fn local_image_path(url: &str, base: Option<&Path>) -> Option<PathBuf> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("mailto:")
    {
        return None;
    }
    if lower.starts_with("file://") {
        // `file:///C:/a.png` on Windows and `file:///home/a.png` elsewhere.
        let rest = &url["file://".len()..];
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        let rest = if cfg!(windows) {
            rest.to_owned()
        } else {
            format!("/{rest}")
        };
        return Some(PathBuf::from(rest));
    }
    let p = Path::new(url);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else {
        base.map(|b| b.join(p))
    }
}

/// Rect covering `[start, end)` in character indices.
fn range_rect(galley: &Galley, start: usize, end: usize) -> Rect {
    let a = galley.pos_from_cursor(CCursor::new(start));
    let b = galley.pos_from_cursor(CCursor::new(end));
    if (a.min.y - b.min.y).abs() > 1.0 {
        // Wrapped across lines: cover the first row only.
        return Rect::from_min_max(a.min, Pos2::new(galley.size().x, a.max.y));
    }
    Rect::from_min_max(a.min, b.max)
}

fn wrap(mut job: LayoutJob, width: f32) -> LayoutJob {
    job.wrap = TextWrapping::wrap_at_width(width);
    job
}

/// The format for one run of body text, in the colour of the surrounding block
/// (`c::TEXT_DIM` inside a blockquote, `c::TEXT` elsewhere).
fn body_format(span: &Span, text: Color32) -> egui::text::TextFormat {
    if span.code {
        return egui::text::TextFormat::simple(mono_font(tfs::MONO), text);
    }
    let mut fmt = egui::text::TextFormat::simple(ui_font(tfs::BODY), text);
    if span.bold {
        fmt.font_id = bold_font(tfs::BODY);
    }
    if span.italic {
        fmt.font_id = egui::FontId::new(tfs::BODY, FontFamily::Name(FAMILY_UI.into()));
        fmt.italics = true;
    }
    if span.strike {
        fmt.strikethrough = Stroke::new(1.0, c::TEXT_DIM);
    }
    raise_or_lower(&mut fmt, span);
    fmt
}

/// Makes a run superscript or subscript: a smaller font aligned to the top or the
/// bottom of its row, which is what egui's `valign` is for.
fn raise_or_lower(fmt: &mut egui::text::TextFormat, span: &Span) {
    if span.sup {
        fmt.font_id = ui_font(tfs::SMALL);
        fmt.valign = egui::Align::TOP;
    } else if span.sub {
        fmt.font_id = ui_font(tfs::SMALL);
        fmt.valign = egui::Align::BOTTOM;
    }
}

/// Builds a wrapping job from inline spans.
///
/// Returns the job plus the URL of each link section, keyed by section index,
/// so link hit areas can be recovered once the text is shaped.
fn inline_job(
    spans: &[Span],
    mut fmt_for: impl FnMut(&Span) -> egui::text::TextFormat,
) -> (LayoutJob, Vec<(usize, String)>) {
    let mut job = LayoutJob::default();
    let mut links: Vec<(usize, String)> = Vec::new();

    for span in spans {
        let mut fmt = fmt_for(span);
        if let Some(url) = &span.link {
            // Links are the one place a second value from the palette is used, so
            // they read as links rather than as faint body text.
            fmt.color = c::ACCENT;
            fmt.underline = Stroke::new(1.0, c::ACCENT);
            links.push((job.sections.len(), url.clone()));
        }
        job.append(&span.text, 0.0, fmt);
    }
    (job, links)
}

fn plain_job(text: String, fmt: egui::text::TextFormat) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(&text, 0.0, fmt);
    job
}

/// A cache key for the `i`th child of `parent`, mixed so that two different places in
/// the document never share one. Keys used to be `parent * 64 + i`, which collided as
/// soon as anything had more than sixty-three children, and table cells used one fixed
/// range, so every table after the first drew the first table's text.
fn key_of(parent: u64, i: usize) -> u64 {
    // The parent and the index are scaled by different odd constants before they are
    // combined, so no two pairs cancel, and the result is then thoroughly mixed.
    let mut z = parent.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(
        (i as u64)
            .wrapping_add(1)
            .wrapping_mul(0xD6E8_FEB8_6659_FD93),
    );
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn child(key: u64, i: usize) -> u64 {
    key_of(key, i)
}

/// The key for a table cell, inside the table whose key is `table`.
fn table_key(table: u64, row: usize, col: usize) -> u64 {
    key_of(key_of(table, row), col)
}

/// About how tall a block will be, from its size alone, for the blocks that have not
/// been laid out yet. Close enough that the scrollbar means something and that
/// laying a block out for real moves the rest of the document by a little.
fn estimate(block: &Block, width: f32) -> f32 {
    const LINE: f32 = 22.0;
    let wrapped = |chars: usize, per: f32| {
        let cols = (width / per).max(8.0);
        ((chars as f32 / cols).ceil().max(1.0)) * LINE
    };
    match block {
        Block::Heading { level, spans } => {
            let chars: usize = spans.iter().map(|s| s.text.chars().count()).sum();
            wrapped(chars, 12.0 - f32::from(*level).min(4.0)) + 18.0
        }
        Block::Para(spans) => {
            let chars: usize = spans.iter().map(|s| s.text.chars().count()).sum();
            wrapped(chars, 7.4) + sp::SM
        }
        Block::Code { text, .. } => {
            text.lines().count().max(1) as f32 * 17.0 + 2.0 * sp::SM + sp::SM
        }
        Block::Quote(blocks) => blocks
            .iter()
            .map(|b| estimate(b, width - sp::MD))
            .sum::<f32>(),
        Block::List { items, .. } => {
            items
                .iter()
                .map(|i| {
                    i.blocks
                        .iter()
                        .map(|b| estimate(b, width - 20.0))
                        .sum::<f32>()
                        + sp::XS
                })
                .sum::<f32>()
                + sp::XS
        }
        Block::Table(t) => (t.rows.len() + 1) as f32 * 28.0 + sp::SM,
        Block::Rule => 20.0,
        Block::Html(h) => h.lines().count().max(1) as f32 * 17.0 + sp::SM,
        Block::Image { .. } => LINE + sp::SM,
        Block::Details { blocks, .. } => {
            // Estimated as open, since the estimate has no access to the toggle;
            // the real height replaces it as soon as the block is on screen.
            LINE + sp::XS
                + blocks
                    .iter()
                    .map(|b| estimate(b, width - sp::LG))
                    .sum::<f32>()
                + sp::SM
        }
        Block::Footnote { blocks, .. } => {
            LINE + blocks
                .iter()
                .map(|b| estimate(b, width - sp::MD))
                .sum::<f32>()
        }
    }
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => tfs::H1,
        2 => tfs::H2,
        3 => tfs::H3,
        _ => tfs::H4,
    }
}

fn heading_color(level: u8) -> Color32 {
    match level {
        1..=3 => c::TEXT,
        _ => c::TEXT_DIM,
    }
}

/// A monospace, syntax-tinted job for a fenced code block.
///
/// The same tokenizer the editor uses, so a code block in the preview is
/// coloured exactly like the same code in the editor.
fn code_job(text: &str, lang: &str) -> LayoutJob {
    let syntax = crate::editing::lang_for(std::path::Path::new(lang));
    let mut job = LayoutJob::default();
    if !text.is_empty() {
        // Appended a line at a time, because that is the unit the tokenizer
        // works in. What it carries from one line to the next is only whether a
        // block comment or a triple-quoted string is still open, and it starts
        // closed for every block, so nothing can run past the end of the fence.
        let mut state = crate::codeedit::State::default();
        let mut lines = text.split('\n').peekable();
        while let Some(line) = lines.next() {
            let (runs, next) = crate::codeedit::tokenize_with(line, &syntax, state);
            state = next;
            for (run, token) in runs {
                job.append(
                    &run,
                    0.0,
                    egui::text::TextFormat::simple(mono_font(tfs::MONO), code_token_color(token)),
                );
            }
            if lines.peek().is_some() {
                job.append(
                    "\n",
                    0.0,
                    egui::text::TextFormat::simple(mono_font(tfs::MONO), c::TEXT),
                );
            }
        }
    }
    job
}

/// Monochrome mapping of token classes onto shades of grey.
fn code_token_color(token: crate::codeedit::Token) -> Color32 {
    match token {
        crate::codeedit::Token::Comment => c::TEXT_GHOST,
        crate::codeedit::Token::Keyword | crate::codeedit::Token::Str => c::ACCENT,
        crate::codeedit::Token::Number => c::TEXT_DIM,
        crate::codeedit::Token::Plain => c::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(blocks: &[Block]) -> Vec<Span> {
        blocks
            .iter()
            .find_map(|b| match b {
                Block::Para(s) => Some(s.clone()),
                _ => None,
            })
            .expect("paragraph")
    }

    #[test]
    fn heading_level_and_text() {
        let b = parse("### Third level\n");
        match &b[0] {
            Block::Heading { level, spans } => {
                assert_eq!(*level, 3);
                assert_eq!(spans[0].text, "Third level");
            }
            other => panic!("expected heading, got {other:?}"),
        }
    }

    #[test]
    fn emphasis_bold_italic_strike() {
        let b = parse("Hello **world** and *soft* and ~~gone~~.\n");
        let p = para(&b);
        assert!(p.iter().any(|s| s.bold && s.text == "world"));
        assert!(p.iter().any(|s| s.italic && s.text == "soft"));
        assert!(p.iter().any(|s| s.strike && s.text == "gone"));
    }

    #[test]
    fn code_block_captures_language_and_body() {
        let b = parse("```rust\nfn main() {}\n```\n");
        let (lang, text) = b
            .iter()
            .find_map(|x| match x {
                Block::Code { lang, text } => Some((lang.clone(), text.clone())),
                _ => None,
            })
            .expect("code block");
        assert_eq!(lang, "rust");
        assert!(text.contains("fn main()"));
    }

    #[test]
    fn task_list_markers() {
        let b = parse("- one\n- [x] done\n- [ ] todo\n");
        let items = b
            .iter()
            .find_map(|x| match x {
                Block::List { items, .. } => Some(items.clone()),
                _ => None,
            })
            .expect("list");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].checked, None);
        assert_eq!(items[1].checked, Some(true));
        assert_eq!(items[2].checked, Some(false));
    }

    #[test]
    fn nested_list_structure() {
        let b = parse("- outer\n  - inner\n");
        let items = b
            .iter()
            .find_map(|x| match x {
                Block::List { items, .. } => Some(items.clone()),
                _ => None,
            })
            .expect("list");
        assert!(
            items[0]
                .blocks
                .iter()
                .any(|b| matches!(b, Block::List { .. }))
        );
    }

    #[test]
    fn table_alignment_is_kept() {
        let b = parse("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        let t = b
            .iter()
            .find_map(|x| match x {
                Block::Table(t) => Some(t.clone()),
                _ => None,
            })
            .expect("table");
        assert_eq!(t.header.len(), 2);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.aligns[0], Alignment::Left);
        assert_eq!(t.aligns[1], Alignment::Right);
    }

    #[test]
    fn links_and_inline_code_are_flagged() {
        let b = parse("See [docs](https://example.com) and `code`.\n");
        let p = para(&b);
        assert!(
            p.iter()
                .any(|s| s.link.as_deref() == Some("https://example.com"))
        );
        assert!(p.iter().any(|s| s.code && s.text == "code"));
    }

    #[test]
    fn plain_text_merges_to_one_span() {
        let b = parse("one two three\n");
        assert_eq!(para(&b).len(), 1);
        assert_eq!(para(&b)[0].text, "one two three");
    }

    #[test]
    fn blockquote_keeps_inner_paragraph() {
        let b = parse("> quoted text\n");
        assert!(b.iter().any(
            |x| matches!(x, Block::Quote(inner) if inner.iter().any(|i| matches!(i, Block::Para(_))))
        ));
    }

    #[test]
    fn images_become_alt_and_url() {
        let b = parse("![diagram](img/a.png)\n");
        assert!(b.iter().any(
            |x| matches!(x, Block::Image { alt, url, .. } if alt == "diagram" && url == "img/a.png")
        ));
    }

    #[test]
    fn a_raw_img_tag_becomes_an_image() {
        let b = parse("<img width=\"800\" alt=\"logo\" src=\"https://x/a.png\" />\n");
        assert!(b.iter().any(|x| matches!(
            x,
            Block::Image { alt, url, width: Some(800.0), .. }
                if alt == "logo" && url == "https://x/a.png"
        )));
    }

    #[test]
    fn image_attributes_are_read_in_any_order() {
        assert_eq!(
            parse_img_tag("<img src='a b.png' alt=\"x\" />"),
            Some((String::from("x"), String::from("a b.png"), None, None))
        );
        assert_eq!(
            parse_img_tag("<img src=a.png>"),
            Some((String::new(), String::from("a.png"), None, None))
        );
        assert_eq!(
            parse_img_tag("<img width=\"320\" height=\"170px\" src=x.png>"),
            Some((
                String::new(),
                String::from("x.png"),
                Some(320.0),
                Some(170.0)
            ))
        );
        assert_eq!(parse_img_tag("<div src=x>"), None);
    }

    #[test]
    fn an_image_is_never_drawn_larger_than_its_own_pixels() {
        // A 320x170 tag keeps its size; a natural image is capped to the measure;
        // nothing is ever enlarged.
        let measure = 760.0;
        assert_eq!(
            fit_size(Vec2::new(1000.0, 500.0), Some(320.0), Some(170.0), measure),
            Vec2::new(320.0, 170.0)
        );
        assert_eq!(
            fit_size(Vec2::new(1000.0, 500.0), None, None, measure),
            Vec2::new(760.0, 380.0)
        );
        assert_eq!(
            fit_size(Vec2::new(120.0, 20.0), None, None, measure),
            Vec2::new(120.0, 20.0),
            "a small badge is left at its own size"
        );
    }

    #[test]
    fn superscript_and_subscript_are_flagged() {
        let b = parse("a ^2^ b and H ~2~ O\n");
        let p = para(&b);
        assert!(p.iter().any(|s| s.sup && s.text == "2"));
        assert!(p.iter().any(|s| s.sub && s.text == "2"));
    }

    #[test]
    fn details_become_a_disclosure() {
        let b = parse(
            "<details>\n<summary><strong>WiFi Features</strong></summary>\n\n- a\n- b\n\n</details>\n",
        );
        let (summary, body, open) = b
            .iter()
            .find_map(|x| match x {
                Block::Details {
                    summary,
                    blocks,
                    open,
                } => Some((summary.clone(), blocks.len(), *open)),
                _ => None,
            })
            .expect("details block");
        assert_eq!(summary, "WiFi Features");
        assert_eq!(body, 1, "one body block");
        assert!(!open, "closed by default");
    }

    #[test]
    fn an_open_details_starts_expanded() {
        let b = parse("<details open>\n<summary>X</summary>\n\nhi\n\n</details>\n");
        assert!(
            b.iter()
                .any(|x| matches!(x, Block::Details { open: true, .. }))
        );
    }

    #[test]
    fn image_paths_resolve_against_the_document() {
        let base = Path::new("docs");
        assert_eq!(
            local_image_path("img/a.png", Some(base)),
            Some(base.join("img/a.png"))
        );
        assert_eq!(
            local_image_path("https://example.com/a.png", Some(base)),
            None
        );
        assert_eq!(
            local_image_path("data:image/png;base64,AAAA", Some(base)),
            None
        );
    }

    #[test]
    fn rule_and_html() {
        let b = parse("---\n\n<div>raw</div>\n");
        assert!(b.iter().any(|x| matches!(x, Block::Rule)));
        assert!(
            b.iter()
                .any(|x| matches!(x, Block::Html(h) if h.contains("raw")))
        );
    }

    #[test]
    fn preview_reparses_only_on_new_version() {
        let mut p = Preview::new();
        p.sync("one", 1);
        assert_eq!(p.blocks.len(), 1);
        p.sync("one", 1);
        assert_eq!(p.blocks.len(), 1, "same version must not re-parse");
        p.sync("one\n\ntwo", 2);
        assert_eq!(p.blocks.len(), 2);
    }

    #[test]
    fn cache_keys_are_unique() {
        assert_ne!(child(1, 0), 1);
        assert_ne!(child(1, 0), child(1, 1));
        assert_ne!(table_key(1, 0, 0), 1);
        // The collisions that used to happen: more than sixty-three children, and the
        // same cell of two different tables.
        assert_ne!(child(child(1, 64), 0), child(child(2, 0), 0));
        assert_ne!(table_key(1, 0, 0), table_key(2, 0, 0));
        let mut seen = std::collections::HashSet::new();
        for parent in 0..40u64 {
            for i in 0..200usize {
                assert!(seen.insert(child(parent, i)), "key {parent}/{i} repeats");
            }
        }
    }

    #[test]
    fn empty_document_is_safe() {
        let b = parse("");
        assert!(b.is_empty());
        let mut p = Preview::new();
        p.sync("", 1);
        assert_eq!(p.blocks.len(), 0);
        p.reset();
        assert_eq!(p.version, None);
    }

    /// A bulleted list must keep one block per item, with its text intact.
    #[test]
    fn list_items_keep_their_text() {
        let blocks = parse("- one\n- two\n- three\n");
        let list = blocks
            .iter()
            .find_map(|b| match b {
                Block::List { items, .. } => Some(items),
                _ => None,
            })
            .expect("no list parsed");
        assert_eq!(list.len(), 3, "expected 3 items");
        for (n, item) in list.iter().enumerate() {
            let text = item
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Para(spans) => {
                        Some(spans.iter().map(|s| s.text.as_str()).collect::<String>())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(text, ["one", "two", "three"][n], "item {n} lost its text");
        }
    }

    /// A table must keep every cell of every row.
    #[test]
    fn table_cells_survive_parsing() {
        let blocks = parse("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let table = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .expect("no table parsed");
        let text = |spans: &Vec<Span>| spans.iter().map(|s| s.text.as_str()).collect::<String>();
        assert_eq!(table.header.len(), 2, "header cells");
        assert_eq!(table.header[0].len(), 1);
        assert_eq!(text(&table.header[0]), "a");
        assert_eq!(text(&table.header[1]), "b");
        assert_eq!(table.rows.len(), 1, "body rows");
        assert_eq!(table.rows[0].len(), 2, "body cells");
        assert_eq!(text(&table.rows[0][0]), "1");
        assert_eq!(text(&table.rows[0][1]), "2");
    }

    /// A remote image is painted from the cache once it has been fetched, so the
    /// preview path that draws web pictures is covered without touching the network.
    #[test]
    fn a_remote_image_is_drawn_once_cached() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let url = "https://img.shields.io/badge/version-2.2-7c5cff?style=flat-square";
        let mut p = Preview::new();
        p.sync(&format!("![Version]({url})\n"), 1);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut remote = crate::remote::RemoteImages::new(tx);
        // A 10x10 image, cached at a size above any the layout will ask for, so no
        // fetch is started.
        remote.insert(url.to_owned(), 2048, vec![200; 10 * 10 * 4], 10, 10, &ctx);

        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_size(egui::vec2(400.0, 600.0));
            p.show_ctx(ui, 8.0, None, Some(&mut remote), None);
        });
        out.textures_delta.clear();
        // An image is painted as a textured `Rect`; the placeholder is a much wider
        // rect, so a narrow one proves the cached texture was used.
        let drew_image = out.shapes.iter().any(|cs| match &cs.shape {
            egui::epaint::Shape::Rect(r) => r.rect.width() < 40.0,
            _ => false,
        });
        assert!(
            drew_image,
            "the cached remote image is painted at its own size"
        );
    }

    /// A `<details>` block keeps its body hidden until the summary is opened.
    #[test]
    fn a_details_body_is_hidden_until_opened() {
        let doc = "<details>\n<summary>More</summary>\n\nsecret body\n\n</details>\n";
        let mut p = Preview::new();
        p.sync(doc, 1);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let render = |p: &mut Preview| -> String {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_max_size(egui::vec2(400.0, 600.0));
                p.show(ui, 8.0);
            });
            out.textures_delta.clear();
            out.shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.job.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let closed = render(&mut p);
        assert!(closed.contains("More"), "the summary shows: {closed:?}");
        assert!(
            !closed.contains("secret body"),
            "the body is hidden while closed: {closed:?}"
        );
        p.details_open.insert(key_of(0, 0), true);
        let open = render(&mut p);
        assert!(
            open.contains("secret body"),
            "the body shows once opened: {open:?}"
        );
    }

    /// The preview must actually paint something: a real `Ui`, a real font
    /// atlas, and a document covering headings, text, lists, tables and code.
    #[test]
    fn renders_a_document_into_a_real_ui() {
        const DOC: &str = concat!(
            "# Title\n\n",
            "## Section\n\n",
            "A paragraph with **bold**, *italic* and `inline code`, long enough ",
            "to need shaping across more than one line in a narrow pane.\n\n",
            "- one\n- two\n- three\n\n",
            "1. first\n2. second\n\n",
            "> a quote\n\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n\n",
            "```rust\nfn main() {}\n```\n\n",
            "---\n\n",
            "- [x] done\n- [ ] todo\n\n",
        );
        let mut p = Preview::new();
        p.sync(DOC, 7);
        assert!(!p.blocks.is_empty(), "parsed no blocks");

        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let mut height = 0.0f32;
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_size(egui::vec2(400.0, 600.0));
            height = p.show(ui, 8.0);
        });
        if std::env::var_os("RHUMB_DUMP_SHAPES").is_some() {
            for cs in &out.shapes {
                if let egui::epaint::Shape::Text(t) = &cs.shape {
                    println!(
                        "TEXT {:?} at {:?}",
                        t.galley.job.text.chars().take(24).collect::<String>(),
                        t.pos
                    );
                }
            }
        }
        // Everything the document mentions has to reach the screen, not just
        // the blocks that happened to survive parsing.
        let painted: String = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Title",
            "Section",
            "one",
            "two",
            "three",
            "first",
            "second",
            "a quote",
            "a",
            "b",
            "1",
            "2",
            "fn main()",
            "done",
            "todo",
        ] {
            assert!(
                painted.contains(expected),
                "`{expected}` never reached the screen"
            );
        }
        out.drop_without_applying_deltas();
        assert!(height > 40.0, "preview collapsed to {height}px");
    }
    /// Everything a preview of `markdown` paints, as one string, in a pane big enough
    /// to show all of it.
    fn painted_text(markdown: &str) -> String {
        let mut p = Preview::new();
        p.sync(markdown, 1);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_size(egui::vec2(500.0, 100_000.0));
            p.show(ui, 8.0);
        });
        let text = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        out.drop_without_applying_deltas();
        text
    }

    #[test]
    fn every_table_in_a_document_shows_its_own_cells() {
        // Tables used to share cache keys, so the second table drew the first one's
        // text in every cell that both had.
        let md = "| alpha | beta |\n|---|---|\n| one | two |\n\n\
                  text between\n\n\
                  | gamma | delta |\n|---|---|\n| three | four |\n";
        let shown = painted_text(md);
        for word in [
            "alpha", "beta", "one", "two", "gamma", "delta", "three", "four",
        ] {
            assert!(
                shown.contains(word),
                "`{word}` is missing from what was painted:\n{shown}"
            );
        }
    }

    #[test]
    fn a_list_with_more_than_sixty_four_items_shows_every_item() {
        // Keys for nested blocks were `parent * 64 + n`, which runs into the next
        // block's keys once a list has more than 63 items.
        let mut md = String::from("before\n\n");
        for i in 0..150 {
            md.push_str(&format!("- item number {i}\n"));
        }
        md.push_str("\nafter\n");
        let shown = painted_text(&md);
        for i in [0, 1, 63, 64, 65, 100, 149] {
            assert!(
                shown.contains(&format!("item number {i}")),
                "item {i} is missing from what was painted"
            );
        }
    }

    /// A document of `n` sections, each a heading, a paragraph and a list.
    fn sections(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "## Section {i}\n\nA paragraph in section {i} that is long enough to wrap onto \
                     a second line in a narrow pane, so that heights differ from any estimate.\n\n\
                     - item {i}a\n- item {i}b\n\n"
                )
            })
            .collect()
    }

    /// The text of every piece of text on a frame, with its y.
    fn text_at(out: &egui::FullOutput) -> Vec<(String, f32)> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some((t.galley.job.text.to_string(), t.pos.y)),
                _ => None,
            })
            .collect()
    }

    /// One frame of the preview in a pane, with `events` delivered.
    fn pane_frame(
        ctx: &egui::Context,
        p: &mut Preview,
        pane: egui::Vec2,
        time: &mut f64,
        events: Vec<egui::Event>,
    ) -> Vec<(String, f32)> {
        *time += 1.0 / 60.0;
        let out = ctx.run_ui(
            egui::RawInput {
                events,
                time: Some(*time),
                ..Default::default()
            },
            |ui| pane_ui(ui, pane, p),
        );
        let shown = text_at(&out);
        out.drop_without_applying_deltas();
        shown
    }

    fn wheel(delta: f32) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, delta),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            },
        ]
    }

    #[test]
    fn a_big_document_paints_only_what_is_near_the_view() {
        let mut p = Preview::new();
        p.sync(&sections(3_000), 1);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let mut time = 0.0;
        let pane = egui::vec2(500.0, 600.0);
        pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        let shown = pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        assert!(
            shown.len() < 120,
            "painted {} pieces of text for a document of 9,000 blocks",
            shown.len()
        );
        assert!(
            shown.iter().any(|(t, _)| t.starts_with("Section 0")),
            "and the top is there"
        );
    }

    #[test]
    fn scrolling_a_big_preview_all_the_way_down_reaches_the_last_section() {
        let mut p = Preview::new();
        p.sync(&sections(400), 1);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let (mut time, pane) = (0.0, egui::vec2(500.0, 600.0));
        pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        let mut reached = false;
        for _ in 0..3_000 {
            let shown = pane_frame(&ctx, &mut p, pane, &mut time, wheel(-25.0));
            if shown.iter().any(|(t, _)| t.starts_with("item 399b")) {
                reached = true;
                break;
            }
        }
        assert!(reached, "the last item was never on screen");
    }

    /// The vertical steps of one heading, frame to frame, while a wheel scrolls a
    /// preview up after it has been taken most of the way down.
    fn scroll_up_steps(mut p: Preview) -> Vec<f32> {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let (mut time, pane) = (0.0, egui::vec2(500.0, 600.0));
        pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        for _ in 0..40 {
            pane_frame(&ctx, &mut p, pane, &mut time, wheel(-400.0));
        }
        for _ in 0..30 {
            pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        }
        // One heading is followed for as long as it is on screen, and another picked
        // when it leaves, so the steps are of a single thing moving.
        let mut follow: Option<(String, f32)> = None;
        let mut steps = Vec::new();
        for _ in 0..220 {
            let shown = pane_frame(&ctx, &mut p, pane, &mut time, wheel(1.5));
            let now = follow
                .as_ref()
                .and_then(|(name, _)| shown.iter().find(|(t, _)| t == name).cloned());
            match (&follow, now) {
                (Some((_, last_y)), Some((name, y))) => {
                    steps.push(y - last_y);
                    follow = Some((name, y));
                }
                _ => {
                    follow = shown
                        .iter()
                        .filter(|(t, _)| t.starts_with("Section "))
                        .min_by(|a, b| (a.1 - 300.0).abs().total_cmp(&(b.1 - 300.0).abs()))
                        .cloned();
                }
            }
        }
        steps
    }

    #[test]
    fn scrolling_up_through_blocks_that_were_never_measured_moves_the_page_as_if_they_had_been() {
        // Take the preview most of the way down, so everything above is an estimate,
        // then scroll back up a notch a frame and follow one heading. Estimates are
        // replaced by real heights as blocks arrive, and the scroll position is
        // adjusted to hide it. The proof is a comparison: a preview that has already
        // measured every block scrolls the same way, and this one has to match it.
        let doc = sections(600);
        let mut guessed = Preview::new();
        guessed.sync(&doc, 1);
        let mut exact = Preview::new();
        exact.sync(&doc, 1);
        // Measured by scrolling from the top to the bottom, which shows every block.
        {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::theme::fonts());
            let (mut time, pane) = (0.0, egui::vec2(500.0, 600.0));
            for _ in 0..6_000 {
                pane_frame(&ctx, &mut exact, pane, &mut time, wheel(-25.0));
                if exact.heights.iter().all(|h| h.is_some()) {
                    break;
                }
            }
            assert!(
                exact.heights.iter().all(|h| h.is_some()),
                "every block is measured"
            );
        }
        let (a, b) = (scroll_up_steps(guessed), scroll_up_steps(exact));
        let n = a.len().min(b.len());
        assert!(n > 20, "the heading was followed for {n} frames");
        for i in 0..n {
            assert!(
                (a[i] - b[i]).abs() < 8.0,
                "frame {i}: with estimates the page moved {:.1}, measured it moves {:.1}
{a:?}
{b:?}",
                a[i],
                b[i]
            );
        }
    }

    #[test]
    fn heights_survive_an_edit_that_touches_one_block() {
        let mut p = Preview::new();
        let doc = sections(50);
        p.sync(&doc, 1);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let (mut time, pane) = (0.0, egui::vec2(500.0, 600.0));
        for _ in 0..3 {
            pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        }
        assert!(p.laid_out(), "every block has an exact height");
        let before = p.heights.clone();
        // Change a paragraph a long way down, which touches one block.
        let edited = doc.replacen(
            "A paragraph in section 40",
            "A different paragraph in section 40",
            1,
        );
        p.sync(&edited, 2);
        let unknown: Vec<usize> = p
            .heights
            .iter()
            .enumerate()
            .filter(|(_, h)| h.is_none())
            .map(|(i, _)| i)
            .collect();
        assert!(!unknown.is_empty(), "the edited stretch is laid out again");
        for (i, h) in p.heights.iter().enumerate() {
            if !unknown.contains(&i) {
                assert_eq!(*h, before[i], "block {i} kept its height");
            }
        }
        for _ in 0..3 {
            pane_frame(&ctx, &mut p, pane, &mut time, Vec::new());
        }
        assert!(p.laid_out(), "and the map is exact again");
    }

    #[test]
    fn a_large_document_is_parsed_in_the_background_and_appears_when_it_is_ready() {
        let doc = sections(4_000);
        assert!(doc.len() > 300_000);
        let ctx = egui::Context::default();
        let mut p = Preview::new();
        let started = std::time::Instant::now();
        p.sync_in_background(&ctx, &doc, 1);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(15),
            "asking for the parse took {:?}, which is the parse itself",
            started.elapsed()
        );
        assert_eq!(p.stats().blocks, 0, "nothing is there yet");
        let mut ready = false;
        for _ in 0..500 {
            p.sync_in_background(&ctx, &doc, 1);
            if p.stats().blocks > 0 {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(ready, "the parse never arrived");
        assert!(p.stats().blocks > 10_000);
    }

    /// Runs the preview inside a scroll area the way the app runs it, and reports
    /// the y of the first line of text it painted.
    ///
    /// A helper rather than a test because it is the whole of what has to be true:
    /// the preview inside a scroll area, a wheel over it, and a look at where the
    /// heading ended up. Scrolling that does not move the heading is scrolling that
    /// a reader cannot see.
    /// The preview drawn exactly as the app draws it, into a pane of a known size.
    ///
    /// A child `Ui` of exactly `pane`, because the `Ui` a `run_ui` hands over has
    /// no bounded size of its own and a scroll area inside one is never smaller
    /// than its content — which would make the whole question vacuous.
    fn pane_ui(ui: &mut egui::Ui, pane: egui::Vec2, p: &mut Preview) {
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, pane)),
        );
        egui::ScrollArea::vertical()
            .id_salt("preview")
            .animated(false)
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                let h = p.show(ui, 8.0);
                ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), h + 8.0),
                    egui::Sense::hover(),
                );
            });
    }

    fn heading_y_after_wheel(
        ctx: &egui::Context,
        p: &mut Preview,
        pane: egui::Vec2,
        wheels: usize,
    ) -> f32 {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            pane_ui(ui, pane, p);
        });
        out.drop_without_applying_deltas();
        for _ in 0..wheels {
            let out = ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(egui::pos2(pane.x * 0.5, pane.y * 0.5)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line,
                            delta: egui::vec2(0.0, -8.0),
                            phase: egui::TouchPhase::Start,
                            modifiers: Default::default(),
                        },
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line,
                            delta: egui::vec2(0.0, -8.0),
                            phase: egui::TouchPhase::Move,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    pane_ui(ui, pane, p);
                },
            );
            out.drop_without_applying_deltas();
            // Let the gesture finish, as a real one does.
            for _ in 0..10 {
                let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    pane_ui(ui, pane, p);
                });
                out.drop_without_applying_deltas();
            }
        }
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            pane_ui(ui, pane, p);
        });
        // The topmost piece of text on the frame, which is the heading when the
        // scroll is at the top and something else once it is not.
        let y = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t.pos.y),
                _ => None,
            })
            .fold(f32::INFINITY, f32::min);
        out.drop_without_applying_deltas();
        if y.is_finite() { y } else { 0.0 }
    }

    #[test]
    fn a_tall_preview_scrolls_in_the_pane_it_is_drawn_in() {
        // The complaint this pins down: a long markdown file opens in the preview
        // and will not scroll. The preview itself is not at fault — it reports the
        // height of the whole document, and the test above proves every block
        // reaches the screen. So the fault is in how the pair is assembled: the
        // scroll area is never told how tall its content is, so it believes the
        // whole document fits and there is nothing to scroll to.
        //
        // Which is why this runs the scroll area and the preview together, exactly
        // as the app runs them, and looks at whether the heading moves. A height that
        // merely looks plausible in a return value is not the question; whether the
        // reader can reach the bottom of the file is.
        let mut doc = String::from("# Heading\n\n");
        for i in 0..40 {
            doc.push_str(&format!(
                "Paragraph {i}, long enough to occupy a line or two.\n\n"
            ));
        }
        let mut p = Preview::new();
        p.sync(&doc, 1);
        assert!(
            p.blocks.len() > 30,
            "the document should be tall, and parsed {} blocks",
            p.blocks.len()
        );

        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let pane = egui::vec2(300.0, 200.0);
        let before = heading_y_after_wheel(&ctx, &mut p, pane, 0);
        let after = heading_y_after_wheel(&ctx, &mut p, pane, 5);
        assert!(
            after < before - 10.0,
            "after five wheel steps the top of the document should have moved up out of \
             the way, but the first text is at y={after:.1} having started at \
             y={before:.1} — the preview does not scroll"
        );
    }

    #[test]
    fn the_users_own_document_scrolls_in_the_preview() {
        // The actual report, on the actual file: a markdown document with a
        // `<details>` block and a long list in it, opened with the preview showing.
        // Whatever that document is made of, the preview has to be scrollable, and
        // the way to know is to put the real file through the real arrangement.
        //
        // Read from the bench directory if it is there, and skipped rather than
        // failed if not — a test that fails because a scratch file was cleaned up is
        // worse than no test, and the shape of the document is reproduced below
        // either way.
        let mut doc = String::from(
            "<details>\n<summary>What it does</summary>\n\n- item one\n- item two\n\n\
             </details>\n\n## A section\n\n",
        );
        for i in 0..29 {
            doc.push_str(&format!("- a list item, number {i}\n"));
        }
        doc.push('\n');
        // A little of everything the parser can produce, so the case is not one
        // narrow shape.
        doc.push_str("Some prose with **bold** and `code`.\n\n");
        doc.push_str("```rust\nfn main() {}\n```\n\n");
        doc.push_str("> a quote\n\n");

        // A pane the size the split actually gives a preview, which is a fraction of
        // a normal window rather than the whole window.
        for pane in [
            egui::vec2(300.0, 200.0),
            egui::vec2(180.0, 140.0),
            egui::vec2(520.0, 600.0),
        ] {
            // A fresh context per pane, because a scroll area keeps its offset
            // between frames and would hand the next pane the last one's scroll
            // position — so the second and third cases would start already scrolled
            // to the bottom and could not show anything moving.
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::theme::fonts());
            let mut p = Preview::new();
            p.sync(&doc, 1);
            let before = heading_y_after_wheel(&ctx, &mut p, pane, 0);
            let after = heading_y_after_wheel(&ctx, &mut p, pane, 4);
            assert!(
                after < before - 5.0,
                "in a {:.0}x{:.0} pane the preview does not scroll: the first text is at \
                 y={after:.1} having started at y={before:.1}",
                pane.x,
                pane.y
            );
        }
    }
}

// ---- where each block is in the source, and the table fix ---------------------------------

#[cfg(test)]
mod line_tests {
    use super::*;

    fn lines(doc: &str) -> Vec<usize> {
        parse_with_lines(doc).1
    }

    #[test]
    fn every_block_has_a_line_and_they_never_go_backwards() {
        let doc = "# T\n\npara one\n\n- a\n- b\n\n| h |\n|---|\n| c |\n\n```\ncode\n```\n\n> q\n\n---\n\n<div>x</div>\n\nlast\n";
        let (blocks, l) = parse_with_lines(doc);
        assert_eq!(blocks.len(), l.len());
        assert!(l.windows(2).all(|w| w[0] <= w[1]), "{l:?}");
    }

    #[test]
    fn the_line_of_each_kind_of_block_is_where_it_starts() {
        let doc = "# Title\n\nA paragraph\nthat runs two lines.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n\n> quoted\n\n---\n\nEnd.\n";
        // Heading, paragraph, list, table, code, quote, rule, paragraph.
        assert_eq!(lines(doc), vec![0, 2, 5, 8, 12, 16, 18, 20]);
    }

    #[test]
    fn a_document_with_nothing_in_it_has_no_blocks_and_no_lines() {
        assert!(lines("").is_empty());
        assert!(lines("\n\n\n").is_empty());
    }

    #[test]
    fn a_single_line_is_line_zero() {
        assert_eq!(lines("just words"), vec![0]);
        assert_eq!(lines("# heading"), vec![0]);
    }

    #[test]
    fn leading_blank_lines_push_the_first_block_down() {
        assert_eq!(lines("\n\n\nfirst\n"), vec![3]);
    }

    #[test]
    fn crlf_line_endings_count_the_same_lines() {
        assert_eq!(lines("# a\r\n\r\npara\r\n\r\n- x\r\n"), vec![0, 2, 4]);
    }

    #[test]
    fn nested_lists_and_quotes_are_one_block_at_the_top() {
        let doc = "- a\n  - b\n    - c\n\n> q\n> > deeper\n\nafter\n";
        assert_eq!(lines(doc), vec![0, 4, 7]);
    }

    #[test]
    fn a_footnote_definition_is_a_block_at_its_own_line() {
        let doc = "text[^1]\n\n[^1]: the note\n";
        let l = lines(doc);
        assert!(l.contains(&0) && l.contains(&2), "{l:?}");
    }

    #[test]
    fn multibyte_text_does_not_throw_the_lines_off() {
        let doc = "# 日本語の見出し\n\nÄÖÜ päragraph ☕\n\n- 項目\n";
        assert_eq!(lines(doc), vec![0, 2, 4]);
    }

    #[test]
    fn a_long_document_is_numbered_correctly_to_the_end() {
        let doc: String = (0..2000)
            .map(|i| format!("## S{i}\n\ntext {i}\n\n"))
            .collect();
        let l = lines(&doc);
        assert_eq!(l.len(), 4000);
        assert_eq!(l[3999], 7998);
    }

    fn shown(doc: &str, width: f32) -> Preview {
        let mut p = Preview::new();
        p.sync(doc, 1);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let mut time = 0.0;
        for _ in 0..4 {
            time += 1.0 / 60.0;
            let out = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width + 20.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let h = p.show(ui, 16.0);
                        ui.allocate_exact_size(egui::vec2(100.0, h), egui::Sense::hover());
                    });
                },
            );
            out.drop_without_applying_deltas();
        }
        p
    }

    fn doc_with_table(filler: usize) -> String {
        let pad: String = (0..filler)
            .map(|i| format!("paragraph {i} with a few words in it\n\n"))
            .collect();
        format!("# T\n\n{pad}| name | value |\n|------|-------|\n| a | 1 |\n| b | 2 |\n\nafter\n")
    }

    #[test]
    fn a_table_is_as_tall_wherever_the_page_has_been_scrolled_to() {
        // Scrolled far enough, the position of the table is negative; its height must
        // not depend on it.
        let doc = doc_with_table(120);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let mut p = Preview::new();
        p.sync(&doc, 1);
        let mut time = 0.0;
        let table = p
            .blocks
            .iter()
            .position(|b| matches!(b, Block::Table(_)))
            .unwrap();
        let mut seen = Vec::new();
        for offset in [0.0f32, 500.0, 1500.0, 2500.0, 2800.0, 3000.0] {
            for _ in 0..3 {
                time += 1.0 / 60.0;
                let out = ctx.run_ui(
                    egui::RawInput {
                        time: Some(time),
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(520.0, 700.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::ScrollArea::vertical()
                            .vertical_scroll_offset(offset)
                            .show(ui, |ui| {
                                let h = p.show(ui, 16.0);
                                ui.allocate_exact_size(egui::vec2(100.0, h), egui::Sense::hover());
                            });
                    },
                );
                out.drop_without_applying_deltas();
            }
            if let Some(h) = p.heights[table] {
                seen.push(h);
            }
        }
        assert!(!seen.is_empty());
        for h in &seen {
            assert!(
                (*h - seen[0]).abs() < 1.0 && *h < 120.0,
                "the table was {h} tall at one scroll position and {} at another: {seen:?}",
                seen[0]
            );
        }
    }

    #[test]
    fn the_header_of_a_table_is_above_its_rows_and_apart_from_the_rule_beneath() {
        let doc = doc_with_table(0);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let mut p = Preview::new();
        p.sync(&doc, 1);
        let mut time = 0.0;
        let mut ys = Vec::new();
        for _ in 0..3 {
            time += 1.0 / 60.0;
            let out = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(520.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let h = p.show(ui, 16.0);
                    ui.allocate_exact_size(egui::vec2(100.0, h), egui::Sense::hover());
                },
            );
            ys = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Text(t) => Some((t.galley.job.text.to_string(), t.pos.y)),
                    _ => None,
                })
                .collect();
            out.drop_without_applying_deltas();
        }
        let y = |s: &str| ys.iter().find(|(t, _)| t == s).map(|(_, y)| *y).unwrap();
        assert!(y("name") < y("a"), "{ys:?}");
        assert!(y("a") < y("b"), "{ys:?}");
        // The header is at least a line above the first row.
        assert!(y("a") - y("name") > 15.0, "{ys:?}");
    }

    #[test]
    fn a_line_maps_to_a_height_and_back() {
        let doc: String = (0..60)
            .map(|i| format!("## S{i}\n\nsome words in section {i}\n\n- a\n- b\n\n"))
            .collect();
        let p = shown(&doc, 480.0);
        for line in [0.0f32, 1.0, 2.0, 3.5, 17.0, 50.25, 120.0, 250.0, 359.0] {
            let y = p.y_of_line(line);
            let back = p.line_at_y(y);
            assert!((back - line).abs() < 0.05, "line {line} -> {y} -> {back}");
        }
    }

    #[test]
    fn height_grows_with_line_and_never_goes_back() {
        let doc: String = (0..40)
            .map(|i| format!("## S{i}\n\ntext {i}\n\n```\nc\n```\n\n"))
            .collect();
        let p = shown(&doc, 480.0);
        let mut last = -1.0f32;
        for k in 0..400 {
            let y = p.y_of_line(k as f32 * 0.5);
            assert!(y >= last, "went back at {k}: {y} < {last}");
            last = y;
        }
        let mut last = -1.0f32;
        for k in 0..400 {
            let l = p.line_at_y(k as f32 * 20.0);
            assert!(l >= last, "went back at {k}: {l} < {last}");
            last = l;
        }
    }

    #[test]
    fn the_top_of_the_page_is_line_zero_and_below_the_end_is_the_last_line() {
        let doc = "# T\n\ntext\n\nmore\n";
        let p = shown(doc, 480.0);
        assert_eq!(p.y_of_line(0.0), 0.0);
        assert!(p.line_at_y(0.0).abs() < 0.01);
        let end = p.y_of_line(1000.0);
        assert!(p.line_at_y(end + 5000.0) <= p.source_lines as f32 + 0.5);
    }

    #[test]
    fn an_empty_preview_maps_everything_to_the_top() {
        let p = Preview::new();
        assert_eq!(p.y_of_line(40.0), 0.0);
        assert_eq!(p.line_at_y(400.0), 0.0);
    }

    #[test]
    fn a_tall_block_is_crossed_in_proportion_to_the_lines_it_covers() {
        // A code block of 20 lines: halfway down it is about halfway through its lines.
        let code: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let doc = format!("before\n\n```\n{code}```\n\nafter\n");
        let p = shown(&doc, 480.0);
        let top = p.y_of_line(2.0);
        let bottom = p.y_of_line(25.0);
        let mid = p.y_of_line(13.5);
        assert!(top < mid && mid < bottom, "{top} {mid} {bottom}");
        let half = (mid - top) / (bottom - top);
        assert!((half - 0.5).abs() < 0.12, "{half}");
    }

    #[test]
    fn heights_that_change_move_the_lines_with_them() {
        let doc: String = (0..30)
            .map(|i| format!("## S{i}\n\nparagraph {i}\n\n"))
            .collect();
        let mut p = shown(&doc, 480.0);
        let before = p.y_of_line(40.0);
        p.heights[2] = p.heights[2].map(|h| h + 100.0);
        p.recompute_tops();
        let after = p.y_of_line(40.0);
        assert!((after - before - 100.0).abs() < 0.01, "{before} -> {after}");
    }
}
