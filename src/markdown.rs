//! Markdown preview.
//!
//! Markdown is parsed into a small block tree once per edit, and every text
//! block is shaped into a `Galley` that is cached afterwards. A frame then only
//! paints cached geometry, so scrolling stays smooth regardless of document
//! length. The caller debounces re-parsing while the user types.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    Color32, FontFamily, Galley, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2,
    epaint::text::{LayoutJob, TextWrapping},
    text::CCursor,
};
use pulldown_cmark::{Alignment, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::editor::syntax_for;
use crate::theme::{FAMILY_UI, bold_font, c, fs as tfs, mono_font, sp, ui_font};

/// One run of inline text sharing a style.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
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
#[derive(Clone, Debug, Default)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A table with its column alignments.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub aligns: Vec<Alignment>,
    pub header: Vec<Vec<Span>>,
    pub rows: Vec<Vec<Vec<Span>>>,
}

/// A block-level element.
#[derive(Clone, Debug)]
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
    Image {
        alt: String,
        url: String,
    },
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
}

/// Parses Markdown into blocks.
pub fn parse(source: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_SMART_PUNCTUATION;
    Builder::run(Parser::new_ext(source, options))
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
    stack: Vec<Frame>,
    spans: Vec<Span>,
    link: Option<String>,
    bold: u32,
    italic: u32,
    strike: u32,
    /// Cells collected so far in the current table row.
    cells: Vec<Vec<Span>>,
    /// Whether the row being built is the header row.
    in_header: bool,
}

impl Builder {
    fn run(parser: Parser<'_>) -> Vec<Block> {
        let mut b = Builder {
            root: Vec::new(),
            stack: Vec::new(),
            spans: Vec::new(),
            link: None,
            bold: 0,
            italic: 0,
            strike: 0,
            cells: Vec::new(),
            in_header: false,
        };
        for ev in parser {
            b.event(ev);
        }
        b.finish()
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
                    self.push(Block::Html(t.to_owned()));
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
            Tag::Superscript | Tag::Subscript => {}
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
                if !spans.is_empty() {
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
            TagEnd::Superscript | TagEnd::Subscript => {}
            TagEnd::Link => self.link = None,
            TagEnd::Image => {
                if let Some(Frame::Image { alt, url }) = self.stack.pop() {
                    let alt = if alt.is_empty() { url.clone() } else { alt };
                    self.push(Block::Image { alt, url });
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
        span.link = self.link.clone();
        merge(&mut self.spans, span);
    }

    /// Appends a finished block to the innermost container, or to the root.
    fn push(&mut self, block: Block) {
        match self.stack.last_mut() {
            None => self.root.push(block),
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
            Some(_) => self.root.push(block),
        }
    }

    fn finish(mut self) -> Vec<Block> {
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Heading(level) => {
                    let spans = std::mem::take(&mut self.spans);
                    if !spans.is_empty() {
                        self.root.push(Block::Heading { level, spans });
                    }
                }
                Frame::Code { lang, text } => self.root.push(Block::Code { lang, text }),
                Frame::Quote(blocks) | Frame::Footnote { blocks, .. } => self.root.extend(blocks),
                Frame::List { start, items } => self.root.push(Block::List { start, items }),
                Frame::Table(table) => self.root.push(Block::Table(table)),
                Frame::Image { alt, url } => self.root.push(Block::Image { alt, url }),
            }
        }
        if !self.spans.is_empty() {
            self.root.push(Block::Para(std::mem::take(&mut self.spans)));
        }
        self.root
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
    height: f32,
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

/// A parsed document plus its shaping cache.
pub struct Preview {
    blocks: Vec<Block>,
    cache: HashMap<usize, Shaped>,
    /// Document version the cache belongs to.
    version: Option<u64>,
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
            cache: HashMap::new(),
            version: None,
            width: 0.0,
            stats: Stats::default(),
        }
    }

    /// Re-parses when the document version moved on.
    pub fn sync(&mut self, text: &str, version: u64) {
        if self.version == Some(version) {
            return;
        }
        let started = std::time::Instant::now();
        self.blocks = parse(text);
        self.stats.parse_us = started.elapsed().as_micros();
        self.stats.blocks = self.blocks.len();
        self.cache.clear();
        self.version = Some(version);
        self.width = 0.0;
    }

    /// Forgets the parsed document, so the next `sync` rebuilds it.
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.cache.clear();
        self.version = None;
        self.width = 0.0;
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Renders the document, returning its total height.
    ///
    /// Blocks are painted at absolute positions, so the origin comes from the
    /// `Ui` itself. Inside a scroll area that origin already carries the scroll
    /// offset, which keeps every block on the same rhythm as it scrolls.
    pub fn show(&mut self, ui: &mut Ui, indent: f32) -> f32 {
        let avail = ui.available_width();
        let width = (avail - indent).clamp(120.0, sp::MD_MEASURE);
        if (self.width - width).abs() > 0.5 {
            self.cache.clear();
            self.width = width;
        }
        // Blocks are laid out left to right from this x, and `indent` is folded
        // into it, so the measuring width above stays correct.
        let origin = ui.min_rect().min;
        let x0 = origin.x + indent;
        let mut hits: Vec<(Rect, String)> = Vec::new();
        let mut y = origin.y;
        // The block tree is small and re-borrowed per block, which keeps the
        // shaping cache mutable while the document is read.
        for i in 0..self.blocks.len() {
            let block = self.blocks[i].clone();
            y = self.block(ui, i + 1, &block, x0, y, width, 0, &mut hits);
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
        (y - origin.y).max(0.0)
    }

    /// Renders one block, returning the y cursor after it.
    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        ui: &mut Ui,
        key: usize,
        block: &Block,
        indent: f32,
        y: f32,
        width: f32,
        depth: u32,
        hits: &mut Vec<(Rect, String)>,
    ) -> f32 {
        if depth > 6 {
            return y;
        }
        match block {
            Block::Heading { level, spans } => {
                let size = heading_size(*level);
                let color = heading_color(*level);
                let (job, links) = inline_job(spans, |span| {
                    let mut fmt = egui::text::TextFormat::simple(bold_font(size), color);
                    if span.italic {
                        fmt.font_id = egui::FontId::new(size, FontFamily::Name(FAMILY_UI.into()));
                        fmt.italics = true;
                    }
                    if *level == 1 {
                        fmt.extra_letter_spacing = 0.15;
                    }
                    fmt
                });
                let h = self.paint(ui, key, job, &links, indent, y, width, hits);
                if *level <= 2 {
                    let line_y = (y + h + sp::XS).round();
                    ui.painter().hline(
                        indent..=(indent + width.min(600.0)),
                        line_y,
                        Stroke::new(1.0, c::DIVIDER),
                    );
                    line_y + sp::SM
                } else {
                    y + h + sp::XS
                }
            }
            Block::Para(spans) => {
                let (job, links) = inline_job(spans, body_format);
                let h = self.paint(ui, key, job, &links, indent, y, width, hits);
                y + h + sp::SM
            }
            Block::Code { lang, text } => {
                let pad = Vec2::new(sp::MD, sp::SM);
                let job = code_job(text, lang);
                let galley = self.shape(ui, key, job, (width - pad.x * 2.0).max(40.0), &[]);
                let rect = Rect::from_min_size(
                    Pos2::new(indent, y),
                    Vec2::new(width, galley.size().y + pad.y * 2.0),
                );
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
                rect.max.y + sp::SM
            }
            Block::Quote(blocks) => {
                let inner_x = indent + sp::MD;
                let inner_w = (width - sp::MD).max(40.0);
                let start = y;
                let mut y = y;
                for (i, b) in blocks.iter().enumerate() {
                    y = self.block(ui, child(key, i), b, inner_x, y, inner_w, depth + 1, hits);
                }
                if y > start + 1.0 {
                    ui.painter().vline(
                        indent,
                        start..=(y - sp::SM).max(start),
                        Stroke::new(2.0, c::BORDER),
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
                        let rect =
                            Rect::from_min_size(Pos2::new(indent + 1.0, y + 3.0), Vec2::splat(s));
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
                        );
                    }
                    y += sp::XS;
                }
                y + sp::XS
            }
            Block::Table(table) => self.table(ui, table, indent, y, width),
            Block::Rule => {
                let line_y = (y + sp::SM).round();
                ui.painter().hline(
                    indent..=(indent + width),
                    line_y,
                    Stroke::new(1.0, c::DIVIDER),
                );
                line_y + sp::SM
            }
            Block::Html(html) => {
                let job = plain_job(
                    html.clone(),
                    egui::text::TextFormat::simple(mono_font(tfs::MONO), c::TEXT_GHOST),
                );
                let h = self.paint(ui, key, job, &[], indent, y, width, hits);
                y + h + sp::SM
            }
            Block::Image { alt, url } => {
                // Images are not decoded; a tidy reference keeps the file honest.
                let label = if alt.is_empty() {
                    url.clone()
                } else {
                    format!("{alt}  \u{2014}  {url}")
                };
                let job = plain_job(
                    label,
                    egui::text::TextFormat::simple(ui_font(tfs::SMALL), c::TEXT_FAINT),
                );
                let h = self.paint(ui, key, job, &[], indent, y, width, hits);
                y + h + sp::SM
            }
            Block::Footnote { label, blocks } => {
                let job = plain_job(
                    format!("[{label}]"),
                    egui::text::TextFormat::simple(ui_font(tfs::SMALL), c::TEXT_FAINT),
                );
                let mut y = y + self.paint(ui, key, job, &[], indent, y, width, hits);
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
                    );
                }
                y
            }
        }
    }

    /// Table layout, honouring the source column alignment.
    fn table(&mut self, ui: &mut Ui, table: &Table, indent: f32, y: f32, width: f32) -> f32 {
        // The column count is the widest row: a header cell holds spans, not
        // columns, so counting `header.first()` would count styled runs.
        let cols = table
            .header
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return y;
        }
        let col_w = (width / cols as f32).floor();
        let mut y = y;
        for (ci, cell) in table.header.iter().enumerate().take(cols) {
            let align = table.aligns.get(ci).copied().unwrap_or(Alignment::None);
            y = y.max(self.cell(
                ui,
                table_key(0, ci),
                cell,
                indent + col_w * ci as f32,
                y,
                col_w,
                true,
                align,
            ));
        }
        if !table.header.is_empty() {
            let line_y = (y + sp::XS).round();
            ui.painter().hline(
                indent..=(indent + width),
                line_y,
                Stroke::new(1.0, c::DIVIDER),
            );
            y = line_y + sp::SM;
        }
        for (ri, row) in table.rows.iter().enumerate() {
            let mut row_h = 0.0f32;
            for (ci, cell) in row.iter().enumerate().take(cols) {
                let align = table.aligns.get(ci).copied().unwrap_or(Alignment::None);
                row_h = row_h.max(self.cell(
                    ui,
                    table_key(ri + 1, ci),
                    cell,
                    indent + col_w * ci as f32,
                    y,
                    col_w - sp::SM,
                    false,
                    align,
                ));
            }
            y += row_h + sp::XS;
        }
        y + sp::SM
    }

    #[allow(clippy::too_many_arguments)]
    fn cell(
        &mut self,
        ui: &mut Ui,
        key: usize,
        spans: &[Span],
        x: f32,
        y: f32,
        w: f32,
        header: bool,
        align: Alignment,
    ) -> f32 {
        let (font, color) = if header {
            (bold_font(tfs::SMALL), c::TEXT_DIM)
        } else {
            (ui_font(tfs::BODY), c::TEXT)
        };
        let bold = if header {
            bold_font(tfs::SMALL)
        } else {
            font.clone()
        };
        let (job, links) = inline_job(spans, move |span| {
            if span.code {
                let mut fmt = egui::text::TextFormat::simple(mono_font(tfs::MONO), c::TEXT);
                fmt.background = c::CODE_BG;
                fmt.expand_bg = 2.0;
                fmt
            } else if span.bold {
                egui::text::TextFormat::simple(bold.clone(), color)
            } else {
                egui::text::TextFormat::simple(font.clone(), color)
            }
        });
        let galley = self.shape(ui, key, job, w.max(20.0), &links);
        let size = galley.size();
        let dx = match align {
            Alignment::Right => (w - size.x).max(0.0),
            Alignment::Center => ((w - size.x) / 2.0).max(0.0),
            _ => 0.0,
        };
        let pos = Pos2::new((x + dx).round(), y.round());
        crate::widgets::galley_at(ui.painter(), pos, &galley, color);
        size.y
    }

    /// Shapes a job at `width`, caching the result.
    fn shape(
        &mut self,
        ui: &mut Ui,
        key: usize,
        job: LayoutJob,
        width: f32,
        links: &[(usize, String)],
    ) -> Arc<Galley> {
        if let Some(s) = self.cache.get(&key) {
            return s.galley.clone();
        }
        let started = std::time::Instant::now();
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(wrap(job, width)));
        self.stats.shape_ms = started.elapsed().as_secs_f32() * 1000.0;

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
                height: galley.size().y,
                galley: galley.clone(),
                link_rects,
            },
        );
        galley
    }

    /// Shapes, paints and returns the height of a text block.
    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        ui: &mut Ui,
        key: usize,
        job: LayoutJob,
        links: &[(usize, String)],
        indent: f32,
        y: f32,
        width: f32,
        hits: &mut Vec<(Rect, String)>,
    ) -> f32 {
        let galley = self.shape(ui, key, job, width, links);
        let height = self.cache.get(&key).map_or(0.0, |s| s.height);
        let pos = Pos2::new(indent, y);
        if let Some(shaped) = self.cache.get(&key) {
            for (r, url) in &shaped.link_rects {
                hits.push((r.translate(pos.to_vec2()), url.clone()));
            }
        }
        crate::widgets::galley_at(ui.painter(), pos, &galley, c::TEXT);
        height
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

fn body_format(span: &Span) -> egui::text::TextFormat {
    if span.code {
        return egui::text::TextFormat::simple(mono_font(tfs::MONO), c::TEXT);
    }
    let mut fmt = egui::text::TextFormat::simple(ui_font(tfs::BODY), c::TEXT);
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
    fmt
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
            fmt.underline = Stroke::new(1.0, c::TEXT_FAINT);
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

fn table_key(row: usize, col: usize) -> usize {
    // Tables live in a key range far above block indices.
    1_000_000 + row * 1_000 + col
}

fn child(key: usize, i: usize) -> usize {
    key * 64 + i + 1
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
fn code_job(text: &str, lang: &str) -> LayoutJob {
    use egui_code_editor::highlighting::Token;
    let syntax = syntax_for(std::path::Path::new(lang));
    let mut job = LayoutJob::default();
    let mut any = false;
    for token in Token::default().tokens(&syntax, text) {
        let buffer = token.buffer().to_owned();
        if buffer.is_empty() {
            continue;
        }
        any = true;
        let color = code_token_color(token.ty());
        job.append(
            &buffer,
            0.0,
            egui::text::TextFormat::simple(mono_font(tfs::MONO), color),
        );
    }
    if !any {
        job.append(
            text,
            0.0,
            egui::text::TextFormat::simple(mono_font(tfs::MONO), c::TEXT),
        );
    }
    job
}

/// Monochrome mapping of editor token types onto shades of grey.
fn code_token_color(ty: egui_code_editor::TokenType) -> Color32 {
    use egui_code_editor::TokenType as T;
    match ty {
        T::Comment(_) => c::TEXT_GHOST,
        T::Keyword | T::Special => c::ACCENT,
        T::Str(_) | T::Literal | T::Numeric(_) => c::TEXT_DIM,
        T::Punctuation(_) => c::TEXT_FAINT,
        T::Type | T::Function | T::Hyperlink => c::TEXT,
        T::Whitespace(_) | T::Unknown => c::TEXT,
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
            |x| matches!(x, Block::Image { alt, url } if alt == "diagram" && url == "img/a.png")
        ));
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
        assert_ne!(table_key(0, 0), 1);
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
        if std::env::var_os("XPLOR_DUMP_SHAPES").is_some() {
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
}
