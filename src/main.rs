//! Mailbox: a desktop email client built on gpui (the UI framework behind Zed).
//!
//! How the pieces fit together:
//!
//!   main.rs         starts the tokio runtime, then hands the main thread to gpui
//!   runtime.rs      the tokio runtime + shared HTTP client, and `spawn()` for
//!                   running network work off the UI thread
//!   app.rs          the window, the shared `AppState`, and the top-level layout
//!   ui/*            one "view" per part of the screen (sidebar, inbox, ...)
//!   models/*        talking to Gmail and mail.tm, plus the colour theme
//!   html_text.rs    turns HTML email bodies into plain text for display
//!   storage.rs      saves/loads accounts and cached mail to a JSON file
//!
//! Two ideas show up everywhere, so they're worth knowing up front:
//!
//! 1. Two async worlds. gpui runs the UI on the main thread with its own
//!    executor, but it can't do networking itself. tokio does the networking
//!    on background threads. UI code sends network work to tokio with
//!    `crate::runtime::spawn(...)` and awaits the result (see runtime.rs).
//!
//! 2. Views only redraw when told to. Each view is an `Entity`; calling
//!    `cx.notify()` on it asks gpui to re-render it. Views subscribe to the
//!    data they show with `cx.observe(...)` so they notify themselves when
//!    that data changes (see app.rs for why this matters).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod assets;
mod html_email;
mod html_text;
mod models;
mod runtime;
mod storage;
mod ui;

use app::MailApp;
use assets::Assets;
use gpui_platform::application;

// Note: no `#[tokio::main]` here on purpose. That macro would run gpui's
// whole event loop *inside* tokio, mixing the two schedulers on the UI
// thread. Instead tokio gets its own threads and gpui owns the main thread.
fn main() {
    // Start the tokio runtime up front. gpui runs on the main thread with its
    // own executors; network work is sent to tokio via `runtime::spawn`.
    runtime::runtime();

    // `with_assets` gives gpui access to everything in /assets (embedded into
    // the binary by rust-embed), so things like `svg().path("images/add.svg")`
    // can be loaded by path. `run` blocks forever running the UI event loop.
    application().with_assets(Assets).run(|cx| {
        MailApp::open(cx);
    });
}
