use std::path::PathBuf;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use rust_i18n::t;

use super::{Selection, SwitchHostsApp};
use crate::i18n::error_text;
use crate::store::{self, Item};

impl SwitchHostsApp {
    /// Export in a layout SwitchHosts can import.
    pub(super) fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = format!("swh_data_{}.json", chrono::Local::now().format("%Y%m%d_%H%M%S"));
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| match store::export(&this.store, &path) {
                Ok(()) => window.push_notification(Notification::success(t!("export_done")), cx),
                Err(err) => {
                    let message = t!("export_failed", error = error_text(&err));
                    Self::notify_error(window, message, cx)
                }
            })
            .ok();
        })
        .detach();
    }

    /// Import a backup of SwitchHosts or of this app.
    pub(super) fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("import").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let result = std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|text| store::import(&text));
            this.update_in(cx, |this, window, cx| match result {
                Ok((items, multi)) => this.confirm_import(items, multi, window, cx),
                Err(err) => {
                    let message = t!("import_failed", error = error_text(&err));
                    Self::notify_error(window, message, cx)
                }
            })
            .ok();
        })
        .detach();
    }

    fn confirm_import(&mut self, items: Vec<Item>, multi: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let count = items.len();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, items) = (view.clone(), items.clone());
            alert
                .confirm()
                .title(t!("import"))
                .description(t!("import_confirm", count = count))
                .on_ok(move |_, window, cx| {
                    let items = items.clone();
                    view.update(cx, |this, cx| {
                        this.store.replace_items(items);
                        if let Some(multi) = multi {
                            this.store.multi_select = multi;
                        }
                        this.select(Selection::System, window, cx);
                        this.store_changed(window, cx);
                        window.push_notification(Notification::success(t!("import_done")), cx);
                    });
                    true
                })
        });
    }
}
