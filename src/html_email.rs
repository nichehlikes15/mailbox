//! Turns an HTML email into a small tree of blocks that `ui::HtmlBody` draws
//! with ordinary gpui elements.
//!
//! This is not a browser. It understands the subset of HTML/CSS that emails
//! actually lean on: paragraphs and headings, inline formatting, links,
//! lists, layout tables, images, background colours, padding, widths and
//! alignment. `<style>` sheets and classes are ignored; only inline `style`
//! attributes and the old presentational attributes (`bgcolor`, `align`,
//! `width`, `<font>`, ...) are read.
//!
//! Parsing happens once, off the UI thread (see `HtmlBody::set_html`), so the
//! result is plain `Send` data with the text styling already worked out.
//! Rendering then just walks the tree.

use base64::Engine as _;
use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, Image, ImageFormat, SharedString,
    StrikethroughStyle, UnderlineStyle, px, rgb,
};
use scraper::{ElementRef, Html, Node};
use std::{ops::Range, sync::Arc};

/// Default text size, in px, before the email sets its own.
pub const BASE_SIZE: f32 = 15.0;
/// Default text colour. Emails are designed for a white page, so the body is
/// always drawn on white with dark text, whatever the app theme is.
pub const TEXT_COLOR: u32 = 0x1f1f1f;
pub const PAGE_COLOR: u32 = 0xffffff;
const LINK_COLOR: u32 = 0x1a5fd0;
const MUTED_COLOR: u32 = 0x5f6368;
const RULE_COLOR: u32 = 0xdadce0;
const CODE_BACKGROUND: u32 = 0xf1f3f4;
const MARK_BACKGROUND: u32 = 0xfff3a3;

pub const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

/// Past this many characters of text the rest of the email is dropped.
const MAX_TEXT_CHARS: usize = 200_000;
/// Remote images beyond this many are shown as their alt text instead.
const MAX_REMOTE_IMAGES: usize = 64;
/// Elements nested deeper than this are skipped (protects the stack).
const MAX_DEPTH: usize = 96;

pub struct Document {
    pub blocks: Vec<Block>,
    /// Background colour the email asked for on `<body>`, if any.
    pub background: Option<u32>,
    /// Every remote image URL, deduplicated, in document order.
    pub remote_images: Vec<SharedString>,
}

pub enum Block {
    Text(TextBlock),
    /// One or more images sitting next to each other on a line.
    Images { align: Align, images: Vec<ImageBlock> },
    Rule,
    Box(BoxBlock),
    List(ListBlock),
    Table(TableBlock),
}

pub struct TextBlock {
    pub text: SharedString,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    pub mono_ranges: Vec<(Range<usize>, SharedString)>,
    pub links: Vec<(Range<usize>, SharedString)>,
    pub size: f32,
    pub align: Align,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
}

#[derive(Clone, Debug, Default)]
pub struct BoxStyle {
    pub background: Option<u32>,
    /// top, right, bottom, left
    pub padding: [f32; 4],
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub width: Option<Length>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub border: Option<(f32, u32)>,
    pub border_left: Option<(f32, u32)>,
    pub radius: f32,
    /// Where the box sits in its parent when it's narrower than it.
    pub align: Align,
    /// Size to content (inline-block), e.g. email "buttons".
    pub shrink: bool,
    /// Makes the whole box clickable.
    pub link: Option<SharedString>,
}

impl BoxStyle {
    /// A box that draws nothing and takes no space can be dropped, and its
    /// children spliced into the parent.
    fn is_plain(&self) -> bool {
        self.background.is_none()
            && self.padding == [0.0; 4]
            && self.margin_top == 0.0
            && self.margin_bottom == 0.0
            && self.width.is_none()
            && self.max_width.is_none()
            && self.min_height.is_none()
            && self.border.is_none()
            && self.border_left.is_none()
            && !self.shrink
            && self.link.is_none()
    }
}

pub struct BoxBlock {
    pub style: BoxStyle,
    pub children: Vec<Block>,
}

pub struct ListBlock {
    pub ordered: bool,
    pub start: u32,
    pub items: Vec<Vec<Block>>,
}

pub struct TableBlock {
    pub style: BoxStyle,
    pub spacing: f32,
    pub rows: Vec<Vec<Cell>>,
}

pub struct Cell {
    pub style: BoxStyle,
    pub valign: VAlign,
    pub children: Vec<Block>,
}

pub struct ImageBlock {
    pub source: ImageSrc,
    pub alt: SharedString,
    pub width: Option<Length>,
    pub height: Option<f32>,
    pub link: Option<SharedString>,
}

pub enum ImageSrc {
    Remote(SharedString),
    Inline(Arc<Image>),
    /// `cid:` attachments, unsupported schemes, over the image limit, ...
    Unavailable,
}

pub fn parse(html: &str) -> Document {
    let html = Html::parse_document(html);
    let mut parser = Parser {
        remote_images: Vec::new(),
        background: None,
        chars: 0,
        truncated: false,
    };
    let mut flow = Flow::new(Align::Left);
    parser.walk_children(html.root_element(), &Ctx::default(), &mut flow, 0);
    flow.flush();
    if parser.truncated {
        let mut line = Line::default();
        line.push(
            "[… message truncated]",
            &Ctx {
                italic: true,
                color: Some(MUTED_COLOR),
                ..Ctx::default()
            },
        );
        flow.blocks.extend(line.finish(Align::Left).map(Block::Text));
    }
    Document {
        blocks: flow.blocks,
        background: parser.background,
        remote_images: parser.remote_images,
    }
}

/// Works out an image format from its first bytes. Servers often send the
/// wrong `Content-Type`, so this is checked first.
pub fn sniff_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"\x89PNG") {
        Some(ImageFormat::Png)
    } else if starts(b"\xFF\xD8\xFF") {
        Some(ImageFormat::Jpeg)
    } else if starts(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(ImageFormat::Webp)
    } else if starts(b"BM") {
        Some(ImageFormat::Bmp)
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else if starts(b"\0\0\x01\0") {
        Some(ImageFormat::Ico)
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
        head.contains("<svg").then_some(ImageFormat::Svg)
    }
}

// --- Inherited state -------------------------------------------------------

/// Everything that inherits down the tree, like CSS inherited properties.
#[derive(Clone)]
struct Ctx {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    pre: bool,
    color: Option<u32>,
    highlight: Option<u32>,
    size: f32,
    align: Align,
    link: Option<SharedString>,
}

impl Default for Ctx {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            code: false,
            pre: false,
            color: None,
            highlight: None,
            size: BASE_SIZE,
            align: Align::Left,
            link: None,
        }
    }
}

/// The part of `Ctx` that styles a run of text. Adjacent text with the same
/// `InlineStyle` is merged into one run.
#[derive(Clone, PartialEq)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    color: Option<u32>,
    highlight: Option<u32>,
    link: Option<SharedString>,
}

impl Ctx {
    fn inline_style(&self) -> InlineStyle {
        InlineStyle {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            code: self.code,
            color: self.color,
            highlight: self.highlight,
            link: self.link.clone(),
        }
    }
}

impl InlineStyle {
    fn highlight_style(&self) -> Option<HighlightStyle> {
        let style = HighlightStyle {
            color: self.color.map(hsla),
            font_weight: self.bold.then_some(FontWeight::BOLD),
            font_style: self.italic.then_some(FontStyle::Italic),
            background_color: self
                .highlight
                .or(self.code.then_some(CODE_BACKGROUND))
                .map(hsla),
            underline: self.underline.then_some(UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            }),
            strikethrough: self.strike.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            }),
            fade_out: None,
        };
        (style != HighlightStyle::default()).then_some(style)
    }
}

fn hsla(color: u32) -> Hsla {
    rgb(color).into()
}

// --- Text accumulation -----------------------------------------------------

/// Inline text being collected until the next block boundary.
#[derive(Default)]
struct Line {
    text: String,
    runs: Vec<(Range<usize>, InlineStyle)>,
    /// Largest font size used by visible text in this line.
    size: f32,
}

impl Line {
    fn push(&mut self, text: &str, ctx: &Ctx) {
        let start = self.text.len();
        let mut visible = false;
        for ch in text.chars() {
            if crate::html_text::is_invisible(ch) {
                continue;
            }
            if ctx.pre {
                match ch {
                    '\r' => {}
                    '\t' => self.text.push_str("    "),
                    ch => self.text.push(ch),
                }
                visible |= !ch.is_whitespace();
            } else if ch == '\u{a0}' {
                // Non-breaking spaces don't collapse.
                self.text.push(' ');
            } else if ch.is_whitespace() {
                if !self.text.is_empty() && !self.text.ends_with([' ', '\n']) {
                    self.text.push(' ');
                }
            } else {
                self.text.push(ch);
                visible = true;
            }
        }
        if visible {
            self.size = self.size.max(ctx.size);
        }
        let end = self.text.len();
        if end > start {
            let style = ctx.inline_style();
            match self.runs.last_mut() {
                Some((range, last)) if range.end == start && *last == style => range.end = end,
                _ => self.runs.push((start..end, style)),
            }
        }
    }

    fn line_break(&mut self) {
        while self.text.ends_with(' ') {
            self.text.pop();
        }
        let len = self.text.len();
        self.runs.retain_mut(|(range, _)| {
            range.end = range.end.min(len);
            range.start < range.end
        });
        self.text.push('\n');
    }

    fn has_text(&self) -> bool {
        self.text.chars().any(|ch| !ch.is_whitespace())
    }

    /// Trims the collected text and turns it into a `TextBlock`.
    fn finish(self, align: Align) -> Option<TextBlock> {
        if !self.has_text() {
            return None;
        }
        let lead = self.text.len() - self.text.trim_start().len();
        let end = self.text.trim_end().len();
        let text = self.text[lead..end].to_string();

        let mut highlights = Vec::new();
        let mut mono_ranges = Vec::new();
        let mut links: Vec<(Range<usize>, SharedString)> = Vec::new();
        for (range, style) in self.runs {
            let range = range.start.clamp(lead, end) - lead..range.end.clamp(lead, end) - lead;
            if range.is_empty() {
                continue;
            }
            if let Some(highlight) = style.highlight_style() {
                highlights.push((range.clone(), highlight));
            }
            if style.code {
                mono_ranges.push((range.clone(), SharedString::from(MONO_FONT)));
            }
            if let Some(href) = style.link {
                match links.last_mut() {
                    Some((last, last_href)) if last.end == range.start && *last_href == href => {
                        last.end = range.end
                    }
                    _ => links.push((range, href)),
                }
            }
        }

        Some(TextBlock {
            text: text.into(),
            highlights,
            mono_ranges,
            links,
            size: if self.size > 0.0 { self.size } else { BASE_SIZE },
            align,
        })
    }
}

/// The content of one block element: finished blocks plus the inline text
/// and images still being collected.
struct Flow {
    align: Align,
    blocks: Vec<Block>,
    line: Line,
    images: Vec<ImageBlock>,
}

impl Flow {
    fn new(align: Align) -> Self {
        Self {
            align,
            blocks: Vec::new(),
            line: Line::default(),
            images: Vec::new(),
        }
    }

    fn push_text(&mut self, text: &str, ctx: &Ctx) {
        if text.chars().any(|ch| !ch.is_whitespace()) {
            self.flush_images();
        }
        self.line.push(text, ctx);
    }

    fn push_image(&mut self, image: ImageBlock) {
        if self.line.has_text() {
            self.flush_line();
        }
        self.images.push(image);
    }

    fn line_break(&mut self) {
        self.flush_images();
        self.line.line_break();
    }

    fn flush_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        self.blocks.extend(line.finish(self.align).map(Block::Text));
    }

    fn flush_images(&mut self) {
        if !self.images.is_empty() {
            self.blocks.push(Block::Images {
                align: self.align,
                images: std::mem::take(&mut self.images),
            });
        }
    }

    fn flush(&mut self) {
        self.flush_line();
        self.flush_images();
    }

    fn push_box(&mut self, style: BoxStyle, children: Vec<Block>) {
        if style.is_plain() {
            self.blocks.extend(children);
        } else if !children.is_empty() || style.background.is_some() || style.min_height.is_some()
        {
            self.blocks.push(Block::Box(BoxBlock { style, children }));
        }
    }
}

// --- Tree walk -------------------------------------------------------------

struct Parser {
    remote_images: Vec<SharedString>,
    background: Option<u32>,
    chars: usize,
    truncated: bool,
}

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "address" | "article" | "aside" | "blockquote" | "center" | "dd" | "details" | "dialog"
            | "div" | "dl" | "dt" | "fieldset" | "figcaption" | "figure" | "footer" | "form"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "hgroup" | "li" | "main"
            | "nav" | "p" | "pre" | "section" | "summary" | "tr" | "td" | "th" | "tbody"
            | "thead" | "tfoot" | "caption" | "legend" | "body" | "html"
    )
}

fn heading_size(tag: &str) -> Option<f32> {
    Some(match tag {
        "h1" => 26.0,
        "h2" => 22.0,
        "h3" => 18.0,
        "h4" => 16.0,
        "h5" => 14.0,
        "h6" => 13.0,
        _ => return None,
    })
}

impl Parser {
    fn walk_children(&mut self, el: ElementRef, ctx: &Ctx, flow: &mut Flow, depth: usize) {
        for child in el.children() {
            if self.truncated {
                return;
            }
            match child.value() {
                Node::Text(text) => {
                    self.chars += text.len();
                    if self.chars > MAX_TEXT_CHARS {
                        self.truncated = true;
                        return;
                    }
                    flow.push_text(text, ctx);
                }
                Node::Element(_) => {
                    if let Some(child) = ElementRef::wrap(child) {
                        self.walk_element(child, ctx, flow, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    /// Walks an element's children into a fresh flow and returns the blocks.
    fn block_children(&mut self, el: ElementRef, ctx: &Ctx, depth: usize) -> Vec<Block> {
        let mut flow = Flow::new(ctx.align);
        self.walk_children(el, ctx, &mut flow, depth);
        flow.flush();
        flow.blocks
    }

    fn walk_element(&mut self, el: ElementRef, parent: &Ctx, flow: &mut Flow, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let tag = el.value().name();
        if matches!(
            tag,
            "script" | "style" | "head" | "title" | "meta" | "link" | "noscript" | "template"
                | "iframe" | "object" | "embed" | "svg" | "input" | "select" | "textarea"
                | "button"
        ) {
            return;
        }
        let css = Css::parse(el.value().attr("style").unwrap_or_default());
        if el.value().attr("hidden").is_some() || css.hides() {
            return;
        }
        let ctx = self.element_ctx(el, tag, &css, parent);

        match tag {
            "br" => flow.line_break(),
            "img" => self.image(el, &css, &ctx, flow),
            "hr" => {
                flow.flush();
                flow.blocks.push(Block::Rule);
            }
            "ul" | "ol" => {
                flow.flush();
                let list = self.list(el, &ctx, tag == "ol", depth);
                let style = self.box_style(el, tag, &css, parent);
                flow.push_box(style, vec![list]);
            }
            "table" => {
                flow.flush();
                if let Some(table) = self.table(el, &css, &ctx, parent, depth) {
                    flow.blocks.push(table);
                }
            }
            "body" | "html" => {
                if let Some(bg) = attr_color(el, "bgcolor").or_else(|| css.background()) {
                    self.background = Some(bg);
                }
                flow.flush();
                let children = self.block_children(el, &ctx, depth);
                flow.blocks.extend(children);
            }
            _ if is_block(tag) || css.is_boxy_inline() || css.display_block() => {
                flow.flush();
                let children = self.block_children(el, &ctx, depth);
                let style = self.box_style(el, tag, &css, parent);
                flow.push_box(style, children);
            }
            _ => self.walk_children(el, &ctx, flow, depth),
        }
    }

    /// Applies an element's own formatting on top of what it inherits.
    fn element_ctx(&self, el: ElementRef, tag: &str, css: &Css, parent: &Ctx) -> Ctx {
        let mut ctx = parent.clone();
        let attr = |name| el.value().attr(name);

        match tag {
            "b" | "strong" => ctx.bold = true,
            "i" | "em" | "cite" | "var" | "dfn" => ctx.italic = true,
            "u" | "ins" => ctx.underline = true,
            "s" | "strike" | "del" => ctx.strike = true,
            "code" | "kbd" | "samp" | "tt" => ctx.code = !ctx.pre,
            "pre" => ctx.pre = true,
            "mark" => ctx.highlight = Some(MARK_BACKGROUND),
            "small" => ctx.size *= 0.85,
            "big" => ctx.size *= 1.2,
            "center" => ctx.align = Align::Center,
            "th" => {
                ctx.bold = true;
                ctx.align = Align::Center;
            }
            "blockquote" => ctx.color = Some(MUTED_COLOR),
            "a" => {
                if let Some(href) = attr("href").and_then(safe_link) {
                    ctx.link = Some(href);
                    ctx.color = Some(LINK_COLOR);
                    ctx.underline = true;
                }
            }
            "font" => {
                if let Some(color) = attr_color(el, "color") {
                    ctx.color = Some(color);
                }
                if let Some(size) = attr("size").and_then(font_tag_size) {
                    ctx.size = size;
                }
            }
            _ => {}
        }
        if let Some(size) = heading_size(tag) {
            ctx.size = size;
            ctx.bold = true;
        }
        if matches!(tag, "p" | "div" | "td" | "th" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
            && let Some(align) = attr("align").and_then(parse_align)
        {
            ctx.align = align;
        }

        // Inline CSS wins over tags and attributes.
        if let Some(color) = css.get("color").and_then(parse_color) {
            ctx.color = Some(color);
        }
        if let Some(weight) = css.get("font-weight") {
            ctx.bold = match weight {
                "bold" | "bolder" => true,
                "normal" | "lighter" => false,
                n => n.parse::<u32>().map_or(ctx.bold, |n| n >= 600),
            };
        }
        if let Some(style) = css.get("font-style") {
            ctx.italic = style == "italic" || style == "oblique";
        }
        if let Some(decoration) = css
            .get("text-decoration")
            .or_else(|| css.get("text-decoration-line"))
        {
            ctx.underline = decoration.contains("underline");
            ctx.strike = decoration.contains("line-through");
        }
        if let Some(size) = css.get("font-size").and_then(|v| parse_font_size(v, parent.size)) {
            ctx.size = size;
        }
        if let Some(family) = css.get("font-family")
            && family.contains("monospace")
        {
            ctx.code = !ctx.pre;
        }
        if let Some(align) = css.get("text-align").and_then(parse_align) {
            ctx.align = align;
        }
        // On inline elements a background just highlights the text; on blocks
        // it becomes the box background (see `box_style`).
        if !is_block(tag)
            && !css.is_boxy_inline()
            && !css.display_block()
            && let Some(bg) = css.background()
        {
            ctx.highlight = Some(bg);
        }
        ctx
    }

    fn box_style(&self, el: ElementRef, tag: &str, css: &Css, parent: &Ctx) -> BoxStyle {
        let attr = |name| el.value().attr(name);
        let mut style = BoxStyle {
            background: attr_color(el, "bgcolor").or_else(|| css.background()),
            align: parent.align,
            ..BoxStyle::default()
        };

        match tag {
            "p" | "dl" | "ul" | "ol" | "pre" | "figure" => style.margin_bottom = 12.0,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                style.margin_top = 8.0;
                style.margin_bottom = 8.0;
            }
            "blockquote" => {
                style.margin_top = 4.0;
                style.margin_bottom = 12.0;
                style.padding[3] = 12.0;
                style.border_left = Some((3.0, RULE_COLOR));
            }
            "dd" => style.padding[3] = 24.0,
            _ => {}
        }
        if tag == "pre" {
            style.background.get_or_insert(CODE_BACKGROUND);
            style.padding = [10.0, 12.0, 10.0, 12.0];
            style.radius = 4.0;
        }
        if tag == "a" {
            style.link = attr("href").and_then(safe_link);
        }

        if let Some(padding) = css.get("padding").and_then(parse_edges) {
            style.padding = padding;
        }
        for (ix, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
            if let Some(v) = css.get(&format!("padding-{side}")).and_then(parse_px) {
                style.padding[ix] = v;
            }
        }
        if let Some(margin) = css.get("margin") {
            let parts: Vec<&str> = margin.split_whitespace().collect();
            if let Some(edges) = parse_edges(margin) {
                style.margin_top = edges[0];
                style.margin_bottom = edges[2];
            }
            if parts.len() >= 2 && parts[1] == "auto" {
                style.align = Align::Center;
            }
        }
        if let Some(v) = css.get("margin-top").and_then(parse_px) {
            style.margin_top = v;
        }
        if let Some(v) = css.get("margin-bottom").and_then(parse_px) {
            style.margin_bottom = v;
        }
        // Negative margins are layout hacks that would only break ours.
        style.margin_top = style.margin_top.clamp(0.0, 48.0);
        style.margin_bottom = style.margin_bottom.clamp(0.0, 48.0);

        style.width = css
            .get("width")
            .and_then(parse_length)
            .or_else(|| attr("width").and_then(parse_length))
            .filter(|w| !matches!(w, Length::Percent(p) if *p >= 100.0));
        style.max_width = css.get("max-width").and_then(parse_px);
        style.min_height = css
            .get("height")
            .or_else(|| css.get("min-height"))
            .and_then(parse_px)
            .or_else(|| attr("height").and_then(parse_px))
            .filter(|h| *h > 0.0 && *h <= 400.0);

        style.border = css
            .get("border")
            .and_then(parse_border)
            .or_else(|| attr("border").and_then(parse_px).filter(|w| *w > 0.0).map(|w| (w.min(4.0), RULE_COLOR)));
        if let Some(left) = css.get("border-left").and_then(parse_border) {
            style.border_left = Some(left);
        }
        style.radius = css.get("border-radius").and_then(parse_px).unwrap_or(style.radius);

        if let Some(align) = attr("align").and_then(parse_align) {
            style.align = align;
        }
        if css.get("display").is_some_and(|d| d.starts_with("inline")) {
            style.shrink = true;
        }
        style
    }

    fn image(&mut self, el: ElementRef, css: &Css, ctx: &Ctx, flow: &mut Flow) {
        let attr = |name| el.value().attr(name);
        let width = css
            .get("width")
            .and_then(parse_length)
            .or_else(|| attr("width").and_then(parse_length));
        let height = css
            .get("height")
            .and_then(parse_px)
            .or_else(|| attr("height").and_then(parse_px));
        // 1x1 tracking pixels.
        if matches!(width, Some(Length::Px(w)) if w <= 2.0) || height.is_some_and(|h| h <= 2.0) {
            return;
        }

        let src = attr("src").unwrap_or_default().trim();
        let source = if let Some(rest) = src.strip_prefix("//") {
            self.remote(format!("https://{rest}"))
        } else if src.starts_with("https://") || src.starts_with("http://") {
            self.remote(src.to_string())
        } else if src.starts_with("data:") {
            decode_data_uri(src).map_or(ImageSrc::Unavailable, ImageSrc::Inline)
        } else {
            ImageSrc::Unavailable
        };
        let alt: SharedString = attr("alt").unwrap_or_default().trim().to_string().into();
        if matches!(source, ImageSrc::Unavailable) && alt.is_empty() {
            return;
        }

        flow.push_image(ImageBlock {
            source,
            alt,
            width,
            height,
            link: ctx.link.clone(),
        });
    }

    fn remote(&mut self, url: String) -> ImageSrc {
        let url = SharedString::from(url);
        if !self.remote_images.contains(&url) {
            if self.remote_images.len() >= MAX_REMOTE_IMAGES {
                return ImageSrc::Unavailable;
            }
            self.remote_images.push(url.clone());
        }
        ImageSrc::Remote(url)
    }

    fn list(&mut self, el: ElementRef, ctx: &Ctx, ordered: bool, depth: usize) -> Block {
        let start = el
            .value()
            .attr("start")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);
        let mut items: Vec<Vec<Block>> = Vec::new();
        for child in el.children().filter_map(ElementRef::wrap) {
            let css = Css::parse(child.value().attr("style").unwrap_or_default());
            if css.hides() {
                continue;
            }
            let tag = child.value().name();
            let child_ctx = self.element_ctx(child, tag, &css, ctx);
            if tag == "li" {
                let blocks = self.block_children(child, &child_ctx, depth + 1);
                let style = self.box_style(child, tag, &css, ctx);
                let mut flow = Flow::new(ctx.align);
                flow.push_box(style, blocks);
                items.push(flow.blocks);
            } else {
                // Stray content (often a nested list) belongs to the previous item.
                let mut flow = Flow::new(ctx.align);
                self.walk_element(child, ctx, &mut flow, depth + 1);
                flow.flush();
                match items.last_mut() {
                    Some(last) => last.extend(flow.blocks),
                    None if !flow.blocks.is_empty() => items.push(flow.blocks),
                    None => {}
                }
            }
        }
        Block::List(ListBlock {
            ordered,
            start,
            items,
        })
    }

    fn table(
        &mut self,
        el: ElementRef,
        css: &Css,
        ctx: &Ctx,
        parent: &Ctx,
        depth: usize,
    ) -> Option<Block> {
        let attr = |name| el.value().attr(name);
        let cell_padding = attr("cellpadding").and_then(parse_px).unwrap_or(0.0);
        let spacing = attr("cellspacing").and_then(parse_px).unwrap_or(0.0).min(24.0);
        let data_borders = attr("border").and_then(parse_px).is_some_and(|b| b > 0.0);

        let mut style = self.box_style(el, "table", css, parent);
        // `border="1"` is drawn on the cells, not around the whole table.
        if data_borders && css.get("border").is_none() {
            style.border = None;
        }

        let mut row_elements = Vec::new();
        for child in el.children().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "tr" => row_elements.push(child),
                "thead" | "tbody" | "tfoot" => row_elements.extend(
                    child
                        .children()
                        .filter_map(ElementRef::wrap)
                        .filter(|row| row.value().name() == "tr"),
                ),
                _ => {}
            }
        }

        let mut rows = Vec::new();
        for row in row_elements {
            if self.truncated {
                break;
            }
            let row_css = Css::parse(row.value().attr("style").unwrap_or_default());
            if row_css.hides() {
                continue;
            }
            let row_ctx = self.element_ctx(row, "tr", &row_css, ctx);
            let row_bg = attr_color(row, "bgcolor").or_else(|| row_css.background());
            let row_valign = row.value().attr("valign").and_then(parse_valign);

            let mut cells = Vec::new();
            for cell in row.children().filter_map(ElementRef::wrap) {
                let tag = cell.value().name();
                if tag != "td" && tag != "th" {
                    continue;
                }
                let cell_css = Css::parse(cell.value().attr("style").unwrap_or_default());
                if cell.value().attr("hidden").is_some() || cell_css.hides() {
                    continue;
                }
                let cell_ctx = self.element_ctx(cell, tag, &cell_css, &row_ctx);
                let mut cell_style = self.box_style(cell, tag, &cell_css, &row_ctx);
                if cell_css.get("padding").is_none() && cell_style.padding == [0.0; 4] {
                    cell_style.padding = [cell_padding; 4];
                }
                cell_style.background = cell_style.background.or(row_bg);
                if data_borders && cell_style.border.is_none() {
                    cell_style.border = Some((1.0, RULE_COLOR));
                }
                // Cells fill their row; `align` is about their content.
                cell_style.align = Align::Left;
                let valign = cell
                    .value()
                    .attr("valign")
                    .or_else(|| cell_css.get("vertical-align"))
                    .and_then(parse_valign)
                    .or(row_valign)
                    .unwrap_or_default();
                let children = self.block_children(cell, &cell_ctx, depth + 1);
                cells.push(Cell {
                    style: cell_style,
                    valign,
                    children,
                });
            }

            // Rows of empty, undecorated cells are just spacing noise.
            let meaningful = cells.iter().any(|c| {
                !c.children.is_empty() || c.style.background.is_some() || c.style.min_height.is_some()
            });
            if meaningful {
                rows.push(cells);
            }
        }

        if rows.is_empty() {
            return None;
        }
        Some(Block::Table(TableBlock {
            style,
            spacing,
            rows,
        }))
    }
}

/// Only links that are safe and useful to hand to the system browser.
fn safe_link(href: &str) -> Option<SharedString> {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:"))
        .then(|| href.to_string().into())
}

fn decode_data_uri(src: &str) -> Option<Arc<Image>> {
    let (meta, data) = src.strip_prefix("data:")?.split_once(',')?;
    let (mime, encoding) = meta.split_once(';').unwrap_or((meta, ""));
    if encoding != "base64" {
        return None;
    }
    let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    let format = sniff_image_format(&bytes).or_else(|| ImageFormat::from_mime_type(mime))?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

// --- Inline CSS ------------------------------------------------------------

/// The declarations from a `style="..."` attribute.
struct Css(Vec<(String, String)>);

impl Css {
    fn parse(style: &str) -> Self {
        Self(
            style
                .split(';')
                .filter_map(|decl| {
                    let (name, value) = decl.split_once(':')?;
                    let value = value.trim().trim_end_matches("!important").trim();
                    Some((name.trim().to_ascii_lowercase(), value.to_ascii_lowercase()))
                })
                .collect(),
        )
    }

    /// Last declaration wins, like in CSS.
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn hides(&self) -> bool {
        self.get("display") == Some("none")
            || self.get("visibility") == Some("hidden")
            || self.get("mso-hide") == Some("all")
            || self.get("opacity").and_then(|o| o.parse::<f32>().ok()) == Some(0.0)
            || self.get("max-height").and_then(parse_px) == Some(0.0)
    }

    fn background(&self) -> Option<u32> {
        self.get("background-color").and_then(parse_color).or_else(|| {
            self.get("background")?
                .split_whitespace()
                .find_map(parse_color)
        })
    }

    fn display_block(&self) -> bool {
        matches!(self.get("display"), Some("block" | "flex" | "table" | "list-item"))
    }

    /// Inline elements styled like a box, e.g. `<a>` "buttons" with padding
    /// and a background. These become real boxes instead of highlighted text.
    fn is_boxy_inline(&self) -> bool {
        let inline_block = self.get("display") == Some("inline-block");
        let padded = ["padding", "padding-top", "padding-left"]
            .iter()
            .any(|p| self.get(p).and_then(parse_edges).is_some_and(|e| e.iter().any(|v| *v > 0.0)));
        (inline_block && (padded || self.background().is_some()))
            || (padded && self.background().is_some())
    }
}

fn parse_color(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#').or_else(|| {
        (value.len() == 6 && value.chars().all(|c| c.is_ascii_hexdigit())).then_some(value)
    }) {
        return match hex.len() {
            3 => {
                let n = u32::from_str_radix(hex, 16).ok()?;
                let (r, g, b) = ((n >> 8) & 0xf, (n >> 4) & 0xf, n & 0xf);
                Some((r * 0x11) << 16 | (g * 0x11) << 8 | b * 0x11)
            }
            6 => u32::from_str_radix(hex, 16).ok(),
            8 => {
                let n = u32::from_str_radix(hex, 16).ok()?;
                (n & 0xff != 0).then_some(n >> 8)
            }
            _ => None,
        };
    }
    if let Some(args) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|v| v.strip_suffix(')'))
    {
        let parts: Vec<&str> = args
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        if let Some(alpha) = parts.get(3).and_then(|a| a.trim_end_matches('%').parse::<f32>().ok())
            && alpha == 0.0
        {
            return None;
        }
        let channel = |s: &str| -> Option<u32> {
            Some(match s.strip_suffix('%') {
                Some(pct) => (pct.parse::<f32>().ok()? * 2.55).round().clamp(0.0, 255.0) as u32,
                None => s.parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u32,
            })
        };
        return Some(channel(parts[0])? << 16 | channel(parts[1])? << 8 | channel(parts[2])?);
    }
    Some(match value {
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "navy" => 0x000080,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "orange" => 0xffa500,
        "yellow" => 0xffff00,
        "purple" => 0x800080,
        "maroon" => 0x800000,
        "teal" => 0x008080,
        "olive" => 0x808000,
        "lime" => 0x00ff00,
        "aqua" | "cyan" => 0x00ffff,
        "fuchsia" | "magenta" => 0xff00ff,
        "whitesmoke" => 0xf5f5f5,
        _ => return None,
    })
}

fn attr_color(el: ElementRef, name: &str) -> Option<u32> {
    el.value().attr(name).and_then(parse_color)
}

/// A length in px. Bare numbers (HTML attributes) count as px.
fn parse_px(value: &str) -> Option<f32> {
    let value = value.trim();
    let px = if let Some(n) = value.strip_suffix("px") {
        n.trim().parse().ok()?
    } else if let Some(n) = value.strip_suffix("pt") {
        n.trim().parse::<f32>().ok()? * 4.0 / 3.0
    } else if let Some(n) = value.strip_suffix("em") {
        n.trim_end_matches('r').trim().parse::<f32>().ok()? * BASE_SIZE
    } else {
        value.parse().ok()?
    };
    px.is_finite().then_some(px)
}

fn parse_length(value: &str) -> Option<Length> {
    match value.trim().strip_suffix('%') {
        Some(pct) => pct.trim().parse().ok().map(Length::Percent),
        None => parse_px(value).filter(|w| *w > 0.0).map(Length::Px),
    }
}

/// CSS 1-4 value shorthand (`padding`, `margin`) -> [top, right, bottom, left].
/// `auto` counts as 0.
fn parse_edges(value: &str) -> Option<[f32; 4]> {
    let values: Option<Vec<f32>> = value
        .split_whitespace()
        .map(|v| if v == "auto" { Some(0.0) } else { parse_px(v) })
        .collect();
    let v = values?;
    Some(match v.as_slice() {
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d, ..] => [*a, *b, *c, *d],
        [] => return None,
    })
}

fn parse_border(value: &str) -> Option<(f32, u32)> {
    if value.starts_with("none") || value.starts_with('0') && parse_px(value.split_whitespace().next()?) == Some(0.0) {
        return None;
    }
    let width = value.split_whitespace().find_map(parse_px).unwrap_or(1.0);
    let color = value.split_whitespace().find_map(parse_color).unwrap_or(RULE_COLOR);
    (width > 0.0).then_some((width.min(8.0), color))
}

fn parse_font_size(value: &str, parent: f32) -> Option<f32> {
    let size = match value.trim() {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "smaller" => parent * 0.85,
        "larger" => parent * 1.2,
        v => {
            if let Some(pct) = v.strip_suffix('%') {
                parent * pct.trim().parse::<f32>().ok()? / 100.0
            } else if let Some(em) = v.strip_suffix("em").filter(|e| !e.ends_with('r')) {
                parent * em.trim().parse::<f32>().ok()?
            } else {
                parse_px(v)?
            }
        }
    };
    // `font-size: 0` is a trick to hide whitespace between inline blocks,
    // not a real size.
    (size >= 6.0).then(|| size.min(48.0))
}

/// `<font size="1..7">`, optionally relative (`+1`, `-2`).
fn font_tag_size(value: &str) -> Option<f32> {
    const SIZES: [f32; 7] = [10.0, 13.0, 16.0, 18.0, 24.0, 32.0, 48.0];
    let value = value.trim();
    let n: i32 = if let Some(rel) = value.strip_prefix('+') {
        3 + rel.parse::<i32>().ok()?
    } else if value.starts_with('-') {
        3 + value.parse::<i32>().ok()?
    } else {
        value.parse().ok()?
    };
    Some(SIZES[(n.clamp(1, 7) - 1) as usize])
}

fn parse_align(value: &str) -> Option<Align> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" | "start" | "justify" => Some(Align::Left),
        "center" | "middle" | "-webkit-center" => Some(Align::Center),
        "right" | "end" => Some(Align::Right),
        _ => None,
    }
}

fn parse_valign(value: &str) -> Option<VAlign> {
    match value.trim().to_ascii_lowercase().as_str() {
        "top" | "baseline" | "text-top" => Some(VAlign::Top),
        "middle" | "center" => Some(VAlign::Middle),
        "bottom" | "text-bottom" => Some(VAlign::Bottom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(blocks: &[Block], out: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Text(t) => out.push(t.text.to_string()),
                Block::Box(b) => texts(&b.children, out),
                Block::List(l) => l.items.iter().for_each(|i| texts(i, out)),
                Block::Table(t) => t
                    .rows
                    .iter()
                    .flatten()
                    .for_each(|c| texts(&c.children, out)),
                Block::Images { .. } | Block::Rule => {}
            }
        }
    }

    fn all_text(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        texts(&parse(html).blocks, &mut out);
        out
    }

    #[test]
    fn collapses_whitespace_and_splits_blocks() {
        assert_eq!(
            all_text("<p>  Hello \n  <b>big</b>   world </p><div>Line one<br>  line two</div>"),
            vec!["Hello big world", "Line one\nline two"]
        );
    }

    #[test]
    fn skips_hidden_and_non_content() {
        assert_eq!(
            all_text(
                "<head><style>p{}</style></head><body>\
                 <div style=\"display:none\">preheader</div><script>x()</script>Shown</body>"
            ),
            vec!["Shown"]
        );
    }

    #[test]
    fn styles_bold_and_links() {
        let doc = parse("<p>Go <a href=\"https://x.test\">to <b>site</b></a> now</p>");
        let mut blocks = doc.blocks.iter();
        let Some(Block::Box(p)) = blocks.next() else { panic!("expected paragraph box") };
        let Block::Text(text) = &p.children[0] else { panic!("expected text") };
        assert_eq!(&*text.text, "Go to site now");
        assert_eq!(text.links, vec![(3..10, SharedString::from("https://x.test"))]);
        assert!(text.highlights.iter().any(|(r, h)| *r == (6..10) && h.font_weight == Some(FontWeight::BOLD)));
    }

    #[test]
    fn drops_unsafe_links_and_tracking_pixels() {
        let doc = parse(
            "<a href=\"javascript:alert(1)\">x</a>\
             <img src=\"https://t.test/p.gif\" width=\"1\" height=\"1\">\
             <img src=\"https://x.test/logo.png\" alt=\"Logo\">",
        );
        assert_eq!(doc.remote_images, vec![SharedString::from("https://x.test/logo.png")]);
        let Block::Text(t) = &doc.blocks[0] else { panic!("expected text") };
        assert!(t.links.is_empty());
    }

    #[test]
    fn parses_tables_and_colors() {
        let doc = parse(
            "<body bgcolor=\"#f4f4f4\"><table width=\"600\" align=\"center\" cellpadding=\"8\">\
             <tr><td bgcolor=\"#fff\">A</td><td style=\"background-color: rgb(0, 0, 255)\">B</td></tr>\
             </table></body>",
        );
        assert_eq!(doc.background, Some(0xf4f4f4));
        let Block::Table(table) = &doc.blocks[0] else { panic!("expected table") };
        assert_eq!(table.style.width, Some(Length::Px(600.0)));
        assert_eq!(table.style.align, Align::Center);
        let row = &table.rows[0];
        assert_eq!(row[0].style.background, Some(0xffffff));
        assert_eq!(row[1].style.background, Some(0x0000ff));
        assert_eq!(row[0].style.padding, [8.0; 4]);
    }

    #[test]
    fn parses_css_values() {
        assert_eq!(parse_color("#abc"), Some(0xaabbcc));
        assert_eq!(parse_color("rgba(0,0,0,0)"), None);
        assert_eq!(parse_edges("10px 20px"), Some([10.0, 20.0, 10.0, 20.0]));
        assert_eq!(parse_font_size("150%", 10.0), Some(15.0));
        assert_eq!(parse_font_size("0", 10.0), None);
        assert_eq!(font_tag_size("+1"), Some(18.0));
    }
}
