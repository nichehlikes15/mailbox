use gpui::{ClipboardItem, Context, Entity, Render, Window, div, prelude::*, px, rgb, svg, deferred, anchored, Anchor};

use crate::app::SidebarEmail;
use crate::models::{Theme, create_account, login};
use crate::ui::Popout;
// The right-hand panel: Gmail accounts and temp-mail addresses. Clicking an
// account selects it (the inbox reacts through its observer on `state`);
// clicking the already-selected one copies its address.
pub struct Sidebar {
    pub state: Entity<crate::app::AppState>,
    pub theme: Entity<Theme>,
    pub popout: Entity<Popout>,
    temp_menu_open: bool,
    mail_menu_open: bool,
    temp_email_hovered: Option<usize>,
}

impl Sidebar {
    pub fn new(state: Entity<crate::app::AppState>, theme: Entity<Theme>, popout: Entity<Popout>, cx: &mut Context<Self>) -> Self {
        // This view is cached, so it must re-render itself when what it
        // shows changes.
        // e.g. a new account was added or a different one selected, so redraw
        // the list.
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.observe(&theme, |_, _, cx| cx.notify()).detach();
        cx.observe(&popout, |_, _, cx| cx.notify()).detach();
        Self { state, theme, popout, temp_menu_open: false, mail_menu_open: false, temp_email_hovered: None }
    }

    /// Shut the two small button-attached menus. They deliberately have no
    /// dimmed backdrop (that is reserved for the centred modals), so they
    /// dismiss themselves via `on_mouse_down_out` instead.
    pub fn close_menus(&mut self, cx: &mut Context<Self>) {
        let was_open = self.temp_menu_open || self.mail_menu_open;
        self.temp_menu_open = false;
        self.mail_menu_open = false;
        if was_open {
            cx.notify();
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, root_cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.read(root_cx).clone();
        let temp_email_state = self.state.clone();
        let google_state = self.state.clone();
        let popout = self.popout.clone();
        let selected_sidebar_email = self.state.read(root_cx).selected_sidebar_email;
        let google_accounts = self.state.read(root_cx).google_accounts.clone();

        let temporary_emails = self
            .state
            .read(root_cx)
            .temp_email
            .iter()
            .enumerate()
            .map(|(index, email)| {
                let app_state = self.state.clone();
                let email_address = email.address.clone();

                let is_selected =
                    selected_sidebar_email == Some(SidebarEmail::Temp(index));

                div()
                    .id(format!("temp-email-{index}"))
                    .group(format!("temp-email-group-{index}"))
                    .px(px(8.0))
                    .py(px(6.0))
                    // Fixed height so revealing the 22px copy/settings buttons on hover
                    // can't stretch the row. 22 (button) + 6 + 6 (padding) = 34, so the
                    // buttons fit exactly and the row is the same size hovered or not.
                    .h(px(34.0))
                    .flex_shrink_0()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .hover(|row| {
                        row.bg(rgb(theme.selected_option))
                            .text_color(rgb(theme.text_muted))
                    })
                    .when(is_selected, |row| {
                        row.bg(rgb(theme.selected_option))
                            .text_color(rgb(theme.text))
                    })
                    .cursor_pointer()
                    .on_hover(root_cx.listener(
                        move |this, hovered, _window, cx| {
                            // A single `MouseMoveEvent` reaches the row being left
                            // *and* the row being entered, and gpui dispatches those
                            // listeners in document order (top to bottom). So when the
                            // pointer moves *up* the list, the row being entered sets
                            // `Some(index)` first, then the row just left below it
                            // overwrites it back to `None` -- and the icons vanish.
                            // The hover background is unaffected because it is a live
                            // per-element hit test rather than shared state, which is
                            // exactly the "colour but no icons" symptom.
                            //
                            // Fix: a row may only clear the flag if it currently owns
                            // it, so a stale `false` from a departing row can no
                            // longer clobber the row that just took over.
                            let next = if *hovered {
                                Some(index)
                            } else if this.temp_email_hovered == Some(index) {
                                None
                            } else {
                                return;
                            };
                            if this.temp_email_hovered != next {
                                this.temp_email_hovered = next;
                                cx.notify();
                            }
                        },
                    ))
                    .on_click(move |_event, _window, cx| {
                        app_state.update(cx, |state, cx| {
                            state.selected_email = Some(index);
                            state.selected_sidebar_email =
                                Some(SidebarEmail::Temp(index));
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .flex_1()
                            // Without this, the address text's intrinsic width is its
                            // floor and it refuses to shrink, shoving the row wider
                            // when the buttons appear. `min_w(0)` lets it give way so
                            // `overflow_hidden` can clip it instead.
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .child(email.address.clone()),
                    )
                    .when(self.temp_email_hovered == Some(index), |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                // Claim exactly the buttons' width and never shrink, so
                                // the address text gives up space instead of the row growing.
                                .w(px(46.0))
                                .h(px(22.0))
                                .flex_shrink_0()
                                .child(
                                div()
                                    .id(format!("copy-temp-email-{index}"))
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|button| {
                                        button.bg(rgb(theme.background))
                                    })
                                    .cursor_pointer()
                                    .on_click({
                                        let email_address = email_address.clone();

                                        move |_event, _window, cx| {
                                            cx.write_to_clipboard(
                                                ClipboardItem::new_string(
                                                    email_address.clone(),
                                                ),
                                            );
                                        }
                                    })
                                    .child(
                                        svg()
                                            .path("images/copy.svg")
                                            .w(px(13.0))
                                            .h(px(13.0))
                                            .text_color(rgb(theme.text_muted)),
                                    ),
                            )
                            .child(
                                div()
                                    .id(format!("settings-temp-email-{index}"))
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|button| {
                                        button.bg(rgb(theme.background))
                                    })
                                    .cursor_pointer()
                                    .on_click({
                                        let app_state = self.state.clone();

                                        root_cx.listener(
                                            move |this, _event, _window, cx| {
                                                // The details modal is drawn centred on
                                                // the window by `MailApp`, not here, so
                                                // the open index lives on the shared
                                                // state. Close the two button menus as
                                                // well, otherwise one would still be
                                                // open behind the modal.
                                                this.close_menus(cx);

                                                app_state.update(cx, |state, cx| {
                                                    state.temp_info_open =
                                                        if state.temp_info_open
                                                            == Some(index)
                                                        {
                                                            None
                                                        } else {
                                                            Some(index)
                                                        };
                                                    cx.notify();
                                                });
                                            },
                                        )
                                    })
                                    .child(
                                        svg()
                                            .path("images/settings.svg")
                                            .w(px(13.0))
                                            .h(px(13.0))
                                            .text_color(rgb(theme.text_muted)),
                                    ),
                            )
                        )
                    })
            })
            .collect::<Vec<_>>();

        div()
            .w(px(360.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(theme.surface))
            .border_l(px(1.0))
            .border_color(rgb(theme.border))
            .child(
                div()
                    .w_full()
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(
                        div()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(14.0))
                            .text_color(rgb(theme.text))
                            .child("Mail")
                            .child(
                                div()
                                    .h(px(25.0))
                                    .px(px(5.0))
                                    .rounded(px(8.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|this| this.bg(rgb(theme.selected_option)))
                                    .id("add-email")
                                    .cursor_pointer()

                                    .on_click(root_cx.listener(
                                        move |this, _event, _window, cx| {
                                            this.mail_menu_open = !this.mail_menu_open;
                                            cx.notify();
                                        },
                                    ))
                                    .child(
                                        svg()
                                            .path("images/add.svg")
                                            .text_color(rgb(theme.text_muted))
                                            .w(px(10.0))
                                            .h(px(10.0)),
                                    )
                                    .when(self.mail_menu_open, |button| {
                                        button.child(
                                            deferred(
                                                anchored()
                                                    .anchor(Anchor::TopLeft)
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .top(px(10.0))
                                                            .right(px(0.0))
                                                            .w(px(190.0))
                                                            .py(px(5.0))
                                                            .bg(rgb(theme.background))
                                                            .border(px(1.0))
                                                            .border_color(rgb(theme.border))
                                                            .rounded(px(6.0))
                                                            .shadow_lg()
                                                            .occlude()
                                                            // No dimmed backdrop for these small menus, so they dismiss
                                                            // themselves on an outside click.
                                                            .on_mouse_down_out(root_cx.listener(|this, _, _, cx| {
                                                                this.mail_menu_open = false;
                                                                cx.notify();
                                                            }))
                                                            .child(
                                                                div()
                                                                    .id("temp-menu-option-1")
                                                                    .px(px(10.0))
                                                                    .py(px(7.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(rgb(theme.text))
                                                                    .hover(|item| {
                                                                        item.bg(rgb(theme.selected_option))
                                                                    })
                                                                    .cursor_pointer()
                                                                    .on_click(root_cx.listener(
                                                                        move |_this, _event, _window, cx| {
                                                                            _this.mail_menu_open = false;

                                                                            let google_state = google_state.clone();
                                                                            if google_state.read(cx).google_login_status.as_deref()
                                                                                == Some("Opening Google login...")
                                                                            {
                                                                                return;
                                                                            }
                                                                            google_state.update(cx, |state, cx| {
                                                                                state.google_login_status =
                                                                                    Some("Opening Google login...".to_string());
                                                                                cx.notify();
                                                                            });
                                                                            // login() runs an axum server and HTTP calls, so it runs on tokio.
                                                                            // Start the work on tokio first, then wait for it from a gpui task.
                                                                            let io = crate::runtime::spawn(login());
                                                                            cx.spawn(async move |_this, cx2| {
                                                                                let result = io
                                                                                    .await
                                                                                    .map_err(anyhow::Error::from)
                                                                                    .and_then(|result| result);
                                                                                match result {
                                                                                    Ok(account) => {
                                                                                        google_state.update(cx2, |state, cx| {
                                                                                            state.google_accounts.push(account);
                                                                                            state.persist();
                                                                                            state.google_login_status = None;
                                                                                            state.selected_sidebar_email =
                                                                                                Some(SidebarEmail::Google(
                                                                                                    state.google_accounts.len() - 1,
                                                                                                ));
                                                                                            cx.notify();
                                                                                        });
                                                                                    }
                                                                                    Err(error) => {
                                                                                        google_state.update(cx2, |state, cx| {
                                                                                            state.google_login_status =
                                                                                                Some(format!(
                                                                                                    "Google login failed: {error:#}"
                                                                                                ));
                                                                                            cx.notify();
                                                                                        });
                                                                                        eprintln!("Google login failed: {error:#}");
                                                                                    }
                                                                                }
                                                                                Ok::<(), anyhow::Error>(())
                                                                            })
                                                                            .detach();
                                                                        },
                                                                    ))
                                                                    .child("Gmail"),
                                                            )
                                                            .child(
                                                                div()
                                                                    .id("temp-menu-option-2")
                                                                    .px(px(10.0))
                                                                    .py(px(7.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(rgb(theme.text))
                                                                    .hover(|item| {
                                                                        item.bg(rgb(theme.selected_option))
                                                                    })
                                                                    .cursor_pointer()
                                                                    .on_click(root_cx.listener(
                                                                        move |this, _event, _window, _cx| {
                                                                            println!("Menu option 2 clicked");
                                                                            this.mail_menu_open = false;
                                                                        },
                                                                    ))
                                                                    .child("Yahoo"),
                                                            ),
                                                    ),
                                            )
                                            .priority(1),
                                        )
                                    })
                            ),
                    )
                    .child(
                        div()
                            .ml(px(8.0))
                            .pl(px(14.0))
                            .border_l(px(1.0))
                            .border_color(rgb(theme.border))
                            .children(google_accounts.iter().enumerate().map(
                                |(index, account)| {
                                    let app_state = self.state.clone();
                                    let email_address = account.email.clone();
                                    let is_selected =
                                        selected_sidebar_email == Some(SidebarEmail::Google(index));
                                    div()
                                        .id(format!("google-account-{index}"))
                                        .px(px(8.0))
                                        .py(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(rgb(theme.text_muted))
                                        .hover(|row| {
                                            row.bg(rgb(theme.selected_option))
                                                .text_color(rgb(theme.text_muted))
                                        })
                                        .when(is_selected, |row| {
                                            row.bg(rgb(theme.selected_option))
                                                .text_color(rgb(theme.text))
                                        })
                                        .cursor_pointer()
                                        .on_click(root_cx.listener(
                                            move |_this, _event, _window, cx| {
                                                if is_selected && !email_address.is_empty() {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(
                                                            email_address.clone(),
                                                        ),
                                                    );
                                                    return;
                                                }
                                                app_state.update(cx, |state, cx| {
                                                    state.selected_email = None;
                                                    state.selected_sidebar_email =
                                                        Some(SidebarEmail::Google(index));
                                                    cx.notify();
                                                });
                                            },
                                        ))
                                        .child(if account.email.is_empty() {
                                            "Gmail".to_string()
                                        } else {
                                            account.email.clone()
                                        })
                                },
                            )),
                    )
                    .when_some(
                        self.state.read(root_cx).google_login_status.clone(),
                        |this, status| {
                            this.child(
                                div()
                                    .px(px(8.0))
                                    .text_size(px(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(status),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .w_full()
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(
                        div()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(14.0))
                            .text_color(rgb(theme.text))
                            .child("Temp Emails")
                            .child(
                                div()
                                    .h(px(25.0))
                                    .px(px(5.0))
                                    .rounded(px(8.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|this| this.bg(rgb(theme.selected_option)))
                                    .id("generate-email")
                                    .cursor_pointer()
                                    .on_click(root_cx.listener(
                                        move |this, _event, _window, cx| {
                                            this.temp_menu_open = !this.temp_menu_open;
                                            cx.notify();
                                        },
                                    ))
                                    .child(
                                        svg()
                                            .path("images/add.svg")
                                            .text_color(rgb(theme.text))
                                            .w(px(10.0))
                                            .h(px(10.0)),
                                    )
                                    .when(self.temp_menu_open, |button| {
                                        button.child(
                                            deferred(
                                                anchored()
                                                    .anchor(Anchor::TopLeft)
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .top(px(10.0))
                                                            .right(px(0.0))
                                                            .w(px(190.0))
                                                            .py(px(5.0))
                                                            .bg(rgb(theme.background))
                                                            .border(px(1.0))
                                                            .border_color(rgb(theme.border))
                                                            .rounded(px(6.0))
                                                            .shadow_lg()
                                                            .occlude()
                                                            // No dimmed backdrop for these small menus, so they dismiss
                                                            // themselves on an outside click.
                                                            .on_mouse_down_out(root_cx.listener(|this, _, _, cx| {
                                                                this.temp_menu_open = false;
                                                                cx.notify();
                                                            }))
                                                            .child(
                                                                div()
                                                                    .id("temp-menu-option-1")
                                                                    .px(px(10.0))
                                                                    .py(px(7.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(rgb(theme.text))
                                                                    .hover(|item| {
                                                                        item.bg(rgb(theme.selected_option))
                                                                    })
                                                                    .cursor_pointer()
                                                                    .on_click(root_cx.listener(
                                                                        move |_this: &mut Sidebar, _event, _window, cx| {
                                                                            _this.temp_menu_open = false;
                                                                            let app_state = temp_email_state.clone();

                                                                            // Same pattern as login above: the network call runs on tokio, and the
                                                                            // gpui task below waits for it and then updates `state` on the UI side.
                                                                            //
                                                                            // `io.await` gives `Result<Result<TempEmail>, JoinError>`: the outer
                                                                            // error means the tokio task panicked/was cancelled, the inner one that
                                                                            // mail.tm said no. `map_err` + `and_then` flatten both into one
                                                                            // `Result` so we only need one `match`.
                                                                            let io = crate::runtime::spawn(create_account());
                                                                            cx.spawn(async move |_this, cx2| {
                                                                                let result = io
                                                                                    .await
                                                                                    .map_err(anyhow::Error::from)
                                                                                    .and_then(|result| result);
                                                                                match result {
                                                                                    Ok(email) => {
                                                                                        app_state.update(cx2, |state, cx| {
                                                                                            state.temp_email.push(email);
                                                                                            state.persist();
                                                                                            cx.notify();
                                                                                        });
                                                                                    }
                                                                                    Err(error) => {
                                                                                        eprintln!(
                                                                                            "Failed to create temp email: {error:#}"
                                                                                        );
                                                                                    }
                                                                                }
                                                                                Ok::<(), anyhow::Error>(())
                                                                            })
                                                                            .detach();
                                                                        },
                                                                    ))
                                                                    .child("Quick Generate"),
                                                            )
                                                            .child(
                                                                div()
                                                                    .id("temp-menu-option-2")
                                                                    .px(px(10.0))
                                                                    .py(px(7.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(rgb(theme.text))
                                                                    .hover(|item| {
                                                                        item.bg(rgb(theme.selected_option))
                                                                    })
                                                                    .cursor_pointer()
                                                                    .on_click(root_cx.listener(
                                                                        move |this, _event, _window, cx| {
                                                                            println!("Menu option 2 clicked");
                                                                            this.temp_menu_open = false;
                                                                            popout.update(cx, |popout, cx| {
                                                                                popout.show(cx);
                                                                                cx.notify();
                                                                            });
                                                                        },
                                                                    ))
                                                                    .child("Custom Generate"),
                                                            )
                                                            .child(
                                                                div()
                                                                    .id("temp-menu-option-3")
                                                                    .px(px(10.0))
                                                                    .py(px(7.0))
                                                                    .text_size(px(12.0))
                                                                    .text_color(rgb(theme.text))
                                                                    .hover(|item| {
                                                                        item.bg(rgb(theme.selected_option))
                                                                    })
                                                                    .cursor_pointer()
                                                                    .on_click(root_cx.listener(
                                                                        move |this, _event, _window, cx| {
                                                                            println!("Menu option 3 clicked");
                                                                            this.temp_menu_open = false;
                                                                        },
                                                                    ))
                                                                    .child("Add Existing"),
                                                            ),
                                                    ),
                                            )
                                            .priority(1),
                                        )
                                    })
                            ),
                    )
                    .child(
                        div()
                            .ml(px(8.0))
                            .pl(px(14.0))
                            .border_l(px(1.0))
                            .border_color(rgb(theme.border))
                            .children(temporary_emails),
                    ),
            )
            .into_any_element()
    }
}
