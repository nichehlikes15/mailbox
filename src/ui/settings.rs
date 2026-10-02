use gpui::{Context, Entity, Image, ImageFormat, Task, Window, WindowControlArea, div, img, prelude::*, px, rgb, svg};
use std::sync::Arc;

use crate::app::AppState;
use crate::models::Theme;

// The settings window. It's a separate window with its own root view, so
// it isn't cached and re-renders whenever gpui redraws that window.
pub struct Settings {
    pub theme: Entity<Theme>,
    pub state: Entity<AppState>,
    pub selected_theme: String,
    pub theme_dropdown_open: bool,
    pub active_section: String,
    pub avatar: Option<Arc<Image>>,
    pub avatar_task: Option<Task<()>>,
}

impl Settings {
    fn section_title(&self) -> &'static str {
        match self.active_section.as_str() {
            "appearance" => "Appearance",
            "accounts" => "Accounts",
            "credits" => "Credits",
            _ => "General",
        }
    }

    fn section_subtitle(&self) -> &'static str {
        match self.active_section.as_str() {
            "appearance" => "Personalize the way MailBox looks",
            "accounts" => "Manage the accounts connected to this app",
            "credits" => "People and services behind this project",
            _ => "A quiet place for all your messages",
        }
    }

    fn selected_theme_label(&self) -> String {
        Theme::available()
            .into_iter()
            .find(|theme| theme.id == self.selected_theme)
            .map(|theme| theme.name)
            .unwrap_or_else(|| "Unknown theme".to_string())
    }

    fn theme_options(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut options = div()
            .id("theme-options")
            .w(px(280.0))
            .mt(px(4.0))
            .p(px(4.0))
            .bg(rgb(theme.surface))
            .border_1()
            .border_color(rgb(theme.border));

        for available_theme in Theme::available() {
            let name = available_theme.id;
            let label = available_theme.name;
            let is_selected = name == self.selected_theme;

            options = options.child(
                div()
                    .id(format!("theme-option-{name}"))
                    .w_full()
                    .px(px(10.0))
                    .py(px(8.0))
                    .text_color(rgb(if is_selected {
                        theme.selected_text
                    } else {
                        theme.text
                    }))
                    .when(is_selected, |this| this.bg(rgb(theme.selected)))
                    .hover(|this| this.bg(rgb(theme.surface_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.selected_theme = name.clone();
                        let selected_theme = settings.selected_theme.clone();
                        settings.theme.update(cx, |theme, theme_cx| {
                            // Replace the shared theme and notify. Every view that observes
                            // `theme` (all of them, see their `new()`) then re-renders with the
                            // new colours, in both windows.
                            *theme = Theme::load_named(&selected_theme);
                            theme_cx.notify();
                        });
                        settings.theme_dropdown_open = false;
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        options
    }

    fn navigation_item(
        &self,
        id: &'static str,
        label: &'static str,
        section: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = self.active_section == section;

        div()
            .id(id)
            .w_full()
            .px(px(12.0))
            .py(px(10.0))
            .rounded(px(6.0))
            .text_size(px(13.0))
            .text_color(rgb(if is_active { theme.selected_text } else { theme.text_inactive }))
            .when(is_active, |this| this.bg(rgb(theme.selected)))
            .when(!is_active, |this| this.hover(|this| this.bg(rgb(theme.surface_hover))))
            .cursor_pointer()
            .on_click(cx.listener(move |settings, _, _, cx| {
                settings.active_section = section.to_string();
                settings.theme_dropdown_open = false;
                cx.notify();
            }))
            .child(label)
    }

    fn clear_storage(&self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.temp_email.clear();
            state.google_accounts.clear();
            state.email_cache.clear();
            state.selected_email = None;
            state.selected_message = None;
            state.selected_sidebar_email = None;
            state.google_login_status = None;
            state.temp_info_open = None;
            crate::storage::clear();
            cx.notify();
        });
    }

    fn delete_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("delete-all-button")
            .mt(px(10.0))
            .w(px(320.0))
            .px(px(12.0))
            .py(px(10.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x8f2d24))
            .text_color(rgb(0xffffff))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(0xb83a2d)))
            .on_click(cx.listener(|settings, _, _, cx| settings.clear_storage(cx)))
            .child("Delete everything")
    }

    fn start_avatar_load(&mut self, cx: &mut Context<Self>) {
        if self.avatar.is_some() || self.avatar_task.is_some() {
            return;
        }

        let io = crate::runtime::spawn(download_avatar());
        self.avatar_task = Some(cx.spawn(async move |settings, cx| {
            let avatar = io.await.ok().flatten();
            settings
                .update(cx, |settings, cx| {
                    settings.avatar = avatar;
                    settings.avatar_task = None;
                    cx.notify();
                })
                .ok();
        }));
    }

}

async fn download_avatar() -> Option<Arc<Image>> {
    let response = crate::runtime::http_public()
        .get("https://github.com/nichehlikes15.png")
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0 mailbox")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or("image/png");
    let format = ImageFormat::from_mime_type(mime)?;
    let bytes = response.bytes().await.ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_avatar_load(cx);
        let theme = self.theme.read(cx).clone();
        let selected_label = self.selected_theme_label();
        let theme_options = self.theme_options(&theme, cx);
        let temp_accounts = self.state.read(cx).temp_email.len();
        let google_accounts = self.state.read(cx).google_accounts.len();
        let avatar = self.avatar.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme.background))
            .text_color(rgb(theme.text))
            .font_family("Lilex")
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .h(px(35.0))
                    .bg(rgb(theme.surface_hover))
                    .child(
                        div()
                            .h_full()
                            .px(px(18.0))
                            .flex()
                            .items_center()
                            .text_size(px(15.0))
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex()
                            .child(
                                div()
                                    .id("settings-minimize-button")
                                    .w(px(46.0))
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|this| this.bg(rgb(0x303030)))
                                    .window_control_area(WindowControlArea::Min)
                                    .child(
                                        svg()
                                            .path("images/minimize.svg")
                                            .text_color(rgb(theme.text))
                                            .w(px(18.0))
                                            .h(px(18.0)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("settings-maximize-button")
                                    .w(px(46.0))
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|this| this.bg(rgb(0x303030)))
                                    .window_control_area(WindowControlArea::Max)
                                    .child(if window.is_maximized() {
                                        svg()
                                            .path("images/restore.svg")
                                            .text_color(rgb(theme.text))
                                            .w(px(18.0))
                                            .h(px(18.0))
                                    } else {
                                        svg()
                                            .path("images/maximize.svg")
                                            .text_color(rgb(theme.text))
                                            .w(px(18.0))
                                            .h(px(18.0))
                                    }),
                            )
                            .child(
                                div()
                                    .id("settings-close-button")
                                    .w(px(46.0))
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|this| this.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff)))
                                    .window_control_area(WindowControlArea::Close)
                                    .child(
                                        svg()
                                            .path("images/close.svg")
                                            .text_color(rgb(theme.text))
                                            .w(px(18.0))
                                            .h(px(18.0)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .child(
                        div()
                            .w(px(220.0))
                            .h_full()
                            .px(px(14.0))
                            .py(px(20.0))
                            .bg(rgb(theme.surface))
                            .child(div().px(px(12.0)).pb(px(14.0)).text_size(px(11.0)).text_color(rgb(theme.text_muted)).child("PREFERENCES"))
                            .child(self.navigation_item("settings-general", "General", "general", &theme, cx))
                            .child(self.navigation_item("settings-appearance", "Appearance", "appearance", &theme, cx))
                            .child(self.navigation_item("settings-accounts", "Accounts", "accounts", &theme, cx))
                            .child(div().mt(px(24.0)).px(px(12.0)).pb(px(14.0)).text_size(px(11.0)).text_color(rgb(theme.text_muted)).child("ABOUT"))
                            .child(self.navigation_item("settings-credits", "Credits", "credits", &theme, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .p(px(36.0))
                            .child(div().text_size(px(20.0)).child(self.section_title()))
                            .child(div().mt(px(8.0)).text_size(px(13.0)).text_color(rgb(theme.text_muted)).child(self.section_subtitle()))
                            .when(self.active_section == "general", |this| this.child(div().mt(px(30.0)).w(px(440.0)).p(px(18.0)).rounded(px(8.0)).bg(rgb(theme.surface)).border_1().border_color(rgb(theme.border)).child(div().text_size(px(14.0)).child("Welcome to MailBox")).child(div().mt(px(8.0)).text_size(px(12.0)).text_color(rgb(theme.text_muted)).child("Your inbox, temporary mail, and connected accounts in one focused workspace."))))
                            .when(self.active_section == "appearance", |this| this.child(div().mt(px(30.0)).text_size(px(13.0)).child("Theme")).child(div().id("theme-selector").mt(px(8.0)).w(px(320.0)).px(px(12.0)).py(px(11.0)).flex().items_center().justify_between().bg(rgb(theme.surface)).border_1().border_color(rgb(theme.border)).cursor_pointer().hover(|this| this.bg(rgb(theme.surface_hover))).on_mouse_down_out(cx.listener(|settings, _, _, cx| { if settings.theme_dropdown_open { settings.theme_dropdown_open = false; cx.notify(); } })).on_click(cx.listener(|settings, _, _, cx| { settings.theme_dropdown_open = !settings.theme_dropdown_open; cx.notify(); })).child(selected_label).child(if self.theme_dropdown_open { "^" } else { "v" })).when(self.theme_dropdown_open, |this| this.child(theme_options)))
                            .when(self.active_section == "accounts", |this| this.child(div().mt(px(30.0)).flex().gap(px(12.0)).child(div().p(px(16.0)).w(px(150.0)).bg(rgb(theme.surface)).border_1().border_color(rgb(theme.border)).child(div().text_size(px(22.0)).child(temp_accounts.to_string())).child(div().mt(px(5.0)).text_size(px(12.0)).text_color(rgb(theme.text_muted)).child("Temporary mail"))).child(div().p(px(16.0)).w(px(150.0)).bg(rgb(theme.surface)).border_1().border_color(rgb(theme.border)).child(div().text_size(px(22.0)).child(google_accounts.to_string())).child(div().mt(px(5.0)).text_size(px(12.0)).text_color(rgb(theme.text_muted)).child("Google accounts")))).child(div().mt(px(32.0)).text_size(px(13.0)).child("Storage")).child(self.delete_button(cx)))
                            .when(self.active_section == "credits", |this| {
                                this.child(
                                    div()
                                        .mt(px(30.0))
                                        .w(px(440.0))
                                        .p(px(18.0))
                                        .bg(rgb(theme.surface))
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .child(
                                            div()
                                                .id("github-profile")
                                                .flex()
                                                .items_center()
                                                .gap(px(12.0))
                                                .cursor_pointer()
                                                .hover(|this| this.bg(rgb(theme.surface_hover)))
                                                .on_click(|_, _, _| {
                                                    let _ = webbrowser::open("https://github.com/nichehlikes15");
                                                })
                                                .child(match avatar {
                                                    Some(image) => img(image)
                                                        .w(px(56.0))
                                                        .h(px(56.0))
                                                        .rounded_full()
                                                        .into_any_element(),
                                                    None => div()
                                                        .w(px(56.0))
                                                        .h(px(56.0))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded_full()
                                                        .bg(rgb(theme.selected))
                                                        .text_color(rgb(theme.selected_text))
                                                        .text_size(px(18.0))
                                                        .child("n")
                                                        .into_any_element(),
                                                })
                                                .child(
                                                    div()
                                                        .child(div().text_size(px(16.0)).child("nichehlikes15"))
                                                        .child(
                                                            div()
                                                                .mt(px(4.0))
                                                                .text_size(px(12.0))
                                                                .text_color(rgb(theme.text_muted))
                                                                .child("Developer"),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id("mail-tm-credit-link")
                                                .mt(px(14.0))
                                                .text_size(px(13.0))
                                                .text_color(rgb(theme.text))
                                                .cursor_pointer()
                                                .hover(|this| this.bg(rgb(theme.surface_hover)))
                                                .on_click(|_, _, _| {
                                                    let _ = webbrowser::open("https://mail.tm");
                                                })
                                                .child("mail.tm API"),
                                        ),
                                )
                            })
                    ),
            )
    }
}
