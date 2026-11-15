//! Ace Term (`ace-term`) GPU-accelerated terminal emulator entry point.

mod terminal_view;

use std::sync::Arc;

use ace_config::{AceConfig, ConfigStore};
use gpui_kit::*;
use tokio::sync::watch;
use tracing::warn;

use crate::terminal_view::TerminalCanvasView;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build Tokio runtime"),
    );

    let (wake_tx, wake_rx) = watch::channel(0u64);
    let wake_for_cfg = wake_tx.clone();

    let global_cfg = AceConfig::default_global_path();
    let workspace_cfg = std::env::current_dir()
        .ok()
        .map(|d| d.join(".ace").join("config.toml"));

    let config_store = Arc::new(
        ConfigStore::load_and_watch(global_cfg, workspace_cfg, move |_| {
            let next = wake_for_cfg.borrow().wrapping_add(1);
            let _ = wake_for_cfg.send(next);
        })
        .unwrap_or_else(|err| {
            warn!(error = %err, "failed to watch config; using defaults");
            ConfigStore::load_and_watch(None, None, |_| {}).expect("default config store")
        }),
    );

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);

        let rt = Arc::clone(&rt);
        let config_store = Arc::clone(&config_store);
        let wake_rx = wake_rx.clone();
        let wake_tx = wake_tx.clone();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1080.0), px(720.0)),
                cx,
            ))),
            window_min_size: Some(size(px(480.0), px(320.0))),
            is_resizable: true,
            is_minimizable: true,
            app_id: Some("ace-term".to_string()),
            ..gpui_kit::component::TitleBar::window_options()
        };

        gpui_kit::open_window(options, cx, move |window, cx| {
            let view = cx.new(|cx| TerminalCanvasView::new(rt, config_store, wake_rx, wake_tx, cx));
            let focus_handle = view.read(cx).focus_handle(cx);
            window.focus(&focus_handle, cx);
            view
        })
        .expect("failed to open Ace Term window");
    });
}
