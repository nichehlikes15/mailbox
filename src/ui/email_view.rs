use crate::html_email::{
    self, Align, Block, BoxStyle, Cell, Document, ImageBlock, ImageSrc, Length, ListBlock,
    TableBlock, TextBlock, VAlign,
};
use gpui::{
    AnyElement, Context, Div, ElementId, Image, ImageFormat, InteractiveText, Render,
    SharedString, StyledText, Task, Window, div, img, prelude::*, px, relative, rgb,
};
use std::{collections::HashMap, sync::Arc};

/// Images bigger than this are not downloaded.
const MAX_IMAGE_BYTES: usize = 15 * 1024 * 1024;
const PLACEHOLDER_COLOR: u32 = 0xf1f3f4;
const ALT_TEXT_COLOR: u32 = 0x5f6368;
const ALT_BORDER_COLOR: u32 = 0xdadce0;

enum ImageState {
    Loading,
    Loaded(Arc<Image>),
    Failed,
}

// Renders an HTML email body with gpui elements. The HTML is parsed into a
// block tree (see `html_email`) on a background thread; remote images are
// then downloaded one by one and appear as they arrive.
pub struct HtmlBody {
    doc: Option<Arc<Document>>,
    images: HashMap<SharedString, ImageState>,
    /// Parsing and image downloads. Cleared when the email changes, which
    /// cancels anything still running for the old one.
    tasks: Vec<Task<()>>,
}

impl HtmlBody {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            doc: None,
            images: HashMap::new(),
            tasks: Vec::new(),
        }
    }

    /// Show a new HTML body, or `None` to clear.
    pub fn set_html(&mut self, html: Option<String>, cx: &mut Context<Self>) {
        self.doc = None;
        self.images.clear();
        self.tasks.clear();
        cx.notify();

        let Some(html) = html else { return };
        let parse = cx.background_spawn(async move { html_email::parse(&html) });
        self.tasks.push(cx.spawn(async move |this, cx| {
            let doc = parse.await;
            this.update(cx, |this, cx| {
                for url in &doc.remote_images {
                    this.images.insert(url.clone(), ImageState::Loading);
                    this.load_image(url.clone(), cx);
                }
                this.doc = Some(Arc::new(doc));
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_image(&mut self, url: SharedString, cx: &mut Context<Self>) {
        let io = crate::runtime::spawn(download_image(url.to_string()));
        self.tasks.push(cx.spawn(async move |this, cx| {
            let image = io.await.ok().flatten();
            this.update(cx, |this, cx| {
                let state = match image {
                    Some(image) => ImageState::Loaded(image),
                    None => ImageState::Failed,
                };
                this.images.insert(url, state);
                cx.notify();
            })
            .ok();
        }));
    }
}

async fn download_image(url: String) -> Option<Arc<Image>> {
    // The URL comes from the email, so only fetch public addresses.
    let url = reqwest::Url::parse(&url).ok()?;
    if !crate::runtime::is_public_url(&url) {
        return None;
    }
    let response = crate::runtime::http_public()
        .get(url)
        // Some image hosts refuse requests without a browser-like user agent.
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0 (Macintosh) mailbox")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|len| len as usize > MAX_IMAGE_BYTES)
    {
        return None;
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let format = html_email::sniff_image_format(&bytes)
        .or_else(|| mime.and_then(|m| ImageFormat::from_mime_type(&m)))?;
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

fn open_link(url: &str) {
    if let Err(error) = webbrowser::open(url) {
        eprintln!("Failed to open link {url}: {error}");
    }
}

impl Render for HtmlBody {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(doc) = self.doc.clone() else {
            return div().into_any_element();
        };
        let mut renderer = Renderer {
            images: &self.images,
            next_id: 0,
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .p(px(16.0))
            .rounded(px(8.0))
            .bg(rgb(doc.background.unwrap_or(html_email::PAGE_COLOR)))
            .text_color(rgb(html_email::TEXT_COLOR))
            .text_size(px(html_email::BASE_SIZE))
            .line_height(relative(1.45))
            .children(renderer.blocks(&doc.blocks))
            .into_any_element()
    }
}

/// Turns the block tree into elements. Rebuilt every render; that's cheap
/// because all the parsing and styling work was done up front.
struct Renderer<'a> {
    images: &'a HashMap<SharedString, ImageState>,
    next_id: usize,
}

impl Renderer<'_> {
    /// Ids for interactive elements, unique within one render.
    fn id(&mut self) -> ElementId {
        self.next_id += 1;
        ElementId::named_usize("html-email", self.next_id)
    }

    fn blocks(&mut self, blocks: &[Block]) -> Vec<AnyElement> {
        blocks.iter().map(|block| self.block(block)).collect()
    }

    fn block(&mut self, block: &Block) -> AnyElement {
        match block {
            Block::Text(text) => self.text(text),
            Block::Images { align, images } => self.image_row(*align, images),
            Block::Rule => div()
                .w_full()
                .h(px(1.0))
                .my(px(8.0))
                .bg(rgb(0xdadce0))
                .into_any_element(),
            Block::Box(block) => {
                let children = self.blocks(&block.children);
                self.boxed(&block.style, div().children(children))
            }
            Block::List(list) => self.list(list),
            Block::Table(table) => self.table(table),
        }
    }

    fn text(&mut self, block: &TextBlock) -> AnyElement {
        let mut styled =
            StyledText::new(block.text.clone()).with_highlights(block.highlights.iter().cloned());
        if !block.mono_ranges.is_empty() {
            styled = styled.with_font_family_overrides(block.mono_ranges.iter().cloned());
        }

        let container = align_text(div().w_full().text_size(px(block.size)), block.align);
        if block.links.is_empty() {
            return container.child(styled).into_any_element();
        }

        let ranges = block.links.iter().map(|(range, _)| range.clone()).collect();
        let urls: Vec<SharedString> = block.links.iter().map(|(_, url)| url.clone()).collect();
        container
            .child(
                InteractiveText::new(self.id(), styled)
                    .on_click(ranges, move |ix, _, _| open_link(&urls[ix])),
            )
            .into_any_element()
    }

    fn image_row(&mut self, align: Align, images: &[ImageBlock]) -> AnyElement {
        let row = div().w_full().flex().flex_row().flex_wrap().items_end();
        let row = match align {
            Align::Left => row.justify_start(),
            Align::Center => row.justify_center(),
            Align::Right => row.justify_end(),
        };
        let children: Vec<AnyElement> = images.iter().map(|image| self.image(image)).collect();
        row.children(children).into_any_element()
    }

    fn image(&mut self, image: &ImageBlock) -> AnyElement {
        let loaded = match &image.source {
            ImageSrc::Inline(data) => Some(data.clone()),
            ImageSrc::Remote(url) => match self.images.get(url) {
                Some(ImageState::Loaded(data)) => Some(data.clone()),
                Some(ImageState::Loading) => return self.placeholder(image),
                Some(ImageState::Failed) | None => None,
            },
            ImageSrc::Unavailable => None,
        };

        let element = match loaded {
            Some(data) => {
                let element = img(data).max_w_full().flex_shrink(1.0);
                match image.width {
                    Some(Length::Px(w)) => element.w(px(w)).into_any_element(),
                    Some(Length::Percent(p)) => {
                        element.w(relative(p / 100.0)).into_any_element()
                    }
                    None => element.into_any_element(),
                }
            }
            None if !image.alt.is_empty() => div()
                .max_w_full()
                .px(px(8.0))
                .py(px(4.0))
                .border_1()
                .border_color(rgb(ALT_BORDER_COLOR))
                .rounded(px(4.0))
                .text_size(px(13.0))
                .italic()
                .text_color(rgb(ALT_TEXT_COLOR))
                .child(image.alt.clone())
                .into_any_element(),
            None => return div().into_any_element(),
        };

        match &image.link {
            Some(url) => {
                let url = url.clone();
                div()
                    .id(self.id())
                    .max_w_full()
                    .cursor_pointer()
                    .on_click(move |_, _, _| open_link(&url))
                    .child(element)
                    .into_any_element()
            }
            None => element,
        }
    }

    /// Grey box the size of a still-loading image, so the layout doesn't
    /// jump much when it arrives.
    fn placeholder(&mut self, image: &ImageBlock) -> AnyElement {
        let (Some(Length::Px(w)), Some(h)) = (image.width, image.height) else {
            return div().into_any_element();
        };
        div()
            .w(px(w))
            .max_w_full()
            .h(px(h))
            .bg(rgb(PLACEHOLDER_COLOR))
            .into_any_element()
    }

    fn list(&mut self, list: &ListBlock) -> AnyElement {
        let items: Vec<AnyElement> = list
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let marker: SharedString = if list.ordered {
                    format!("{}.", list.start as usize + ix).into()
                } else {
                    "•".into()
                };
                let content = self.blocks(item);
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.0))
                    .child(div().w(px(22.0)).flex_none().text_right().child(marker))
                    .child(div().flex_1().min_w_0().flex().flex_col().children(content))
                    .into_any_element()
            })
            .collect();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(items)
            .into_any_element()
    }

    fn table(&mut self, table: &TableBlock) -> AnyElement {
        let rows: Vec<AnyElement> = table
            .rows
            .iter()
            .map(|row| {
                let cells: Vec<AnyElement> = row.iter().map(|cell| self.cell(cell)).collect();
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .gap(px(table.spacing))
                    .children(cells)
                    .into_any_element()
            })
            .collect();
        self.boxed(&table.style, div().gap(px(table.spacing)).children(rows))
    }

    fn cell(&mut self, cell: &Cell) -> AnyElement {
        let children = self.blocks(&cell.children);
        let inner = div().children(children);
        let inner = match cell.valign {
            VAlign::Top => inner.justify_start(),
            VAlign::Middle => inner.justify_center(),
            VAlign::Bottom => inner.justify_end(),
        };
        // Cells without a width share the leftover space equally.
        let inner = match cell.style.width {
            Some(_) => inner.flex_shrink(1.0),
            None => inner.flex_1(),
        };
        self.boxed(&cell.style, inner)
    }

    /// Applies a `BoxStyle` to a container. Every box is a flex column so
    /// its children stack and stretch like normal block layout.
    fn boxed(&mut self, style: &BoxStyle, el: Div) -> AnyElement {
        let [top, right, bottom, left] = style.padding;
        let mut el = el
            .flex()
            .flex_col()
            .min_w_0()
            .pt(px(top))
            .pr(px(right))
            .pb(px(bottom))
            .pl(px(left))
            .mt(px(style.margin_top))
            .mb(px(style.margin_bottom));

        if let Some(bg) = style.background {
            el = el.bg(rgb(bg));
        }
        match style.width {
            Some(Length::Px(w)) => el = el.w(px(w)).max_w_full(),
            Some(Length::Percent(p)) => el = el.w(relative(p / 100.0)),
            None if style.max_width.is_some() => el = el.w_full(),
            None => {}
        }
        if let Some(max) = style.max_width {
            el = el.max_w(px(max));
        }
        if let Some(h) = style.min_height {
            el = el.min_h(px(h));
        }
        if let Some((width, color)) = style.border {
            el = el.border(px(width)).border_color(rgb(color));
        }
        if let Some((width, color)) = style.border_left {
            el = el.border_l(px(width)).border_color(rgb(color));
        }
        if style.radius > 0.0 {
            el = el.rounded(px(style.radius));
        }
        // Boxes narrower than their parent get placed according to `align`;
        // everything else stretches to the full width.
        if style.width.is_some() || style.max_width.is_some() || style.shrink {
            el = match style.align {
                Align::Left => el.self_start(),
                Align::Center => el.self_center(),
                Align::Right => el.self_end(),
            };
        }

        match &style.link {
            Some(url) => {
                let url = url.clone();
                el.id(self.id())
                    .cursor_pointer()
                    .on_click(move |_, _, _| open_link(&url))
                    .into_any_element()
            }
            None => el.into_any_element(),
        }
    }
}

fn align_text(el: Div, align: Align) -> Div {
    match align {
        Align::Left => el,
        Align::Center => el.text_center(),
        Align::Right => el.text_right(),
    }
}
