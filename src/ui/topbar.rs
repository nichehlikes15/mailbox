use crate::app::AppState;
use crate::models::Theme;
use gpui::{
    Bounds, Context, Entity, Window, WindowBounds, WindowControlArea, WindowHandle, WindowOptions,
    div, prelude::*, px, rgb, size, svg,
};

// The custom title bar: app name, settings button and window controls
// (the window is created with a transparent system title bar, so we draw
// our own).
pub struct TopBar {
    pub theme: Entity<Theme>,
    pub state: Entity<AppState>,
    // Handle to the settings window if it's open, so clicking the button
    // again focuses it instead of opening a second one.
    pub settings_window: Option<WindowHandle<crate::ui::settings::Settings>>,
}

impl TopBar {
    pub fn new(theme: Entity<Theme>, state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Cached view (see app.rs), so redraw ourselves when the theme changes.
        cx.observe(&theme, |_, _, cx| cx.notify()).detach();
        Self {
            theme,
            state,
            settings_window: None,
        }
    }
}

impl Render for TopBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.read(cx).clone();
        div()
            .w_full()
            .h(px(35.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(rgb(theme.surface_hover))
            .child(
                div()
                    .h_full()
                    .px(px(18.0))
                    .flex()
                    .items_center()
                    .text_color(rgb(theme.text))
                    .text_size(px(15.0))
                    .child("MailBox"),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    // An empty area you can drag to move the window.
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("settings-button")
                            .h(px(26.0))
                            .px(px(8.0))
                            .rounded(px(8.0))
                            .flex()
                            .items_center()
                            .hover(|this| this.bg(rgb(theme.selected_option)))
                            .child(
                                svg()
                                    .path("images/settings.svg")
                                    .text_color(rgb(theme.text))
                                    .w(px(15.0))
                                    .h(px(15.0)),
                            )
                            .on_click(cx.listener(|topbar, _, _, cx| {
                                // If the settings window is still open, just bring it to the front.
                                // `update` fails if it was closed, so we fall through and open a new one.
                                if let Some(settings_window) = topbar.settings_window && settings_window
                                        .update(cx, |_, window, _| window.activate_window())
                                        .is_ok()
                                    {
                                        return;
                                    }

                                let bounds = Bounds::centered(None, size(px(900.0), px(650.0)), cx);
                                let theme_entity = topbar.theme.clone();
                                let state_entity = topbar.state.clone();

                                let settings_window = cx
                                    .open_window(
                                        WindowOptions {
                                            window_bounds: Some(WindowBounds::Windowed(bounds)),
                                            titlebar: None,
                                            is_resizable: true,
                                            is_minimizable: true,
                                            is_movable: true,
                                            ..Default::default()
                                        },
                                        |_window, cx| {
                                            let selected_theme = theme_entity.read(cx).name.clone();
                                            cx.new(|_| crate::ui::settings::Settings {
                                                theme: theme_entity,
                                                state: state_entity,
                                                selected_theme,
                                                theme_dropdown_open: false,
                                                active_section: "general".to_string(),
                                                avatar: None,
                                                avatar_task: None,
                                            })
                                        },
                                    )
                                    .unwrap();
                                topbar.settings_window = Some(settings_window);
                            })),
                    )
                    .child(
                        div()
                            .id("minimize-button")
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
                            .id("maximize-button")
                            .w(px(46.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|this| this.bg(rgb(0x303030)))
                            .window_control_area(WindowControlArea::Max)
                            // Icons are loaded by path from the embedded assets. The old
                            // `svg().data(include_bytes!(...))` copied and hashed the file bytes
                            // on every render; with `.path()` gpui loads it once and caches it.
                            .child(if _window.is_maximized() {
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
                            .id("close-button")
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
            )
            .into_any_element()
    }
}
