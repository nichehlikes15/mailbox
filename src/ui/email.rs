//use crate::app::AppState;
use crate::models::{Email, Theme};
use crate::ui::HtmlBody;
use gpui::{Context, Entity, Render, SharedString, Window, div, prelude::*, px, rgb, img};

// Shows one opened email: subject, sender and body, with a back button.
pub struct EmailView {
    //pub state: Entity<AppState>,
    pub theme: Entity<Theme>,
    email_id: Option<String>,
    // `SharedString` is gpui's cheap-to-clone string (reference counted), so
    // passing it to the UI each render doesn't copy the text. The old code
    // cloned the entire `Email`, body and all, on every render.
    subject: SharedString,
    from: SharedString,
    /// Cleaned-up body text, computed once when the email is shown rather
    /// than on every render. Only used for plain-text emails.
    body: SharedString,
    /// Renders the body as real HTML when the email has any.
    html_body: Entity<HtmlBody>,
    is_html: bool,
}

impl EmailView {
    pub fn new(theme: Entity<Theme>, cx: &mut Context<Self>) -> Self {
        // Re-render when the theme changes. This view is cached (see app.rs), so
        // without this it would keep the old colours until something else
        // notified it.
        cx.observe(&theme, |_, _, cx| cx.notify()).detach();

        Self {
            //state,
            theme,
            email_id: None,
            subject: SharedString::default(),
            from: SharedString::default(),
            body: SharedString::default(),
            html_body: cx.new(HtmlBody::new),
            is_html: false,
        }
    }

    /// Called by the inbox when an email is opened (or with `None` to clear).
    /// All the expensive work happens here, once: the raw body (often a huge
    /// blob of HTML) is converted to plain text by `html_text::display_body`.
    /// `render` then just displays the stored strings.
    pub fn show(&mut self, email: Option<Email>, cx: &mut Context<Self>) {
        match email {
            Some(email) => {
                let raw = if email.body.trim().is_empty() {
                    &email.intro
                } else {
                    &email.body
                };
                self.is_html = crate::html_text::looks_like_html(raw);
                if self.is_html {
                    self.body = SharedString::default();
                    let html = raw.clone();
                    self.html_body
                        .update(cx, |view, cx| view.set_html(Some(html), cx));
                } else {
                    self.body = crate::html_text::display_body(raw).into();
                    self.html_body.update(cx, |view, cx| view.set_html(None, cx));
                }
                self.subject = email.subject.clone().into();
                self.from = email.from.clone().into();
                self.email_id = Some(email.id);
            }
            None => {
                self.email_id = None;
                self.subject = SharedString::default();
                self.from = SharedString::default();
                self.body = SharedString::default();
                self.is_html = false;
                self.html_body.update(cx, |view, cx| view.set_html(None, cx));
            }
        }
        cx.notify();
    }
}

impl Render for EmailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.read(cx).clone();
        let Some(email_id) = self.email_id.as_deref() else {
            return div().into_any_element();
        };
        // let state = self.state.clone();

        div()
            .size_full()
            .min_h(px(0.0))
            .px(px(24.0))
            .py(px(20.0))
            .flex()
            .flex_col()
            .bg(rgb(theme.background))
            // .child(
            //     div()
            //         .id("back-to-inbox")
            //         .cursor_pointer()
            //         .text_color(rgb(theme.text))
            //         .on_click(move |_event, _window, cx| {
            //             state.update(cx, |state, cx| {
            //                 state.selected_message = None;
            //                 cx.notify();
            //             });
            //         })
            //         .child("Back to inbox"),
            // )
            .child(
                div()
                    .text_size(px(22.0))
                    .text_color(rgb(theme.text))
                    .child(self.subject.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .mt(px(12.0))
                    .child(
                        img("images/default.png")
                            .w(px(25.0))
                            .h(px(25.0))
                            .rounded_full(),
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(rgb(theme.text_muted))
                            .child(self.from.clone()),
                    ),
            )
            .child(
                div()
                    // Keyed by email so each one opens scrolled to the top.
                    // Scrolling needs an element id, because gpui stores the scroll offset
                    // per id. Putting the email id in it means each email gets its own
                    // scroll position, so a new email opens at the top.
                    .id(format!("email-body-{email_id}"))
                    .mt(px(24.0))
                    .flex_1()
                    .min_h(px(0.0))
                    // Was commented out before, so long emails just overflowed the window.
                    // `flex_1` + `min_h(0)` give this box the remaining height; anything
                    // taller than that scrolls.
                    .overflow_y_scroll()
                    .pr(px(12.0))
                    .when(self.is_html, |el| el.child(self.html_body.clone()))
                    .when(!self.is_html, |el| {
                        el.child(
                            div()
                                .text_size(px(15.0))
                                .text_color(rgb(theme.text))
                                .child(self.body.clone()),
                        )
                    }),
            )
            .into_any_element()
    }
}
