use std::borrow::Cow;
use std::cell::Cell;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _, Toggle, ToggleGroup, ToggleVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    radio::{Radio, RadioGroup},
    sheet::Sheet,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use super::{
    Selection, SwitchHostsApp, display_title, folder_mode_label, kind_label, sheet_footer, sheet_width,
};
use crate::store::{FolderMode, Item, ItemKind};

const KINDS: [ItemKind; 3] = [ItemKind::Local, ItemKind::Group, ItemKind::Folder];
const FOLDER_MODES: [FolderMode; 3] = [FolderMode::Default, FolderMode::Single, FolderMode::Multi];

/// State of the add / edit sheet while it is open.
pub(super) struct EditDraft {
    /// `None` when adding a new item.
    id: Option<String>,
    kind: ItemKind,
    title_input: Entity<InputState>,
    folder_mode: FolderMode,
    include: Vec<String>,
    parent_id: Option<String>,
    _subscription: Subscription,
}

fn kind_icon(kind: ItemKind) -> IconName {
    match kind {
        ItemKind::Local => IconName::FileText,
        ItemKind::Group => IconName::Layers,
        ItemKind::Folder => IconName::Folder,
    }
}

/// A labelled form block; callers append the control and any hints.
fn field(label: impl Into<SharedString>, color: Hsla) -> Div {
    v_flex()
        .gap_2()
        .child(div().text_sm().font_medium().text_color(color).child(label.into()))
}

impl SwitchHostsApp {
    pub(super) fn open_add_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parent_id = match self.selected_item() {
            Some(item) if item.kind == ItemKind::Folder => Some(item.id.clone()),
            Some(item) => self.store.parent_of(&item.id).flatten(),
            None => None,
        };
        self.open_draft(None, Item::default(), parent_id, window, cx);
    }

    pub(super) fn open_edit_sheet(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.store.find(&id).cloned() else {
            return;
        };
        self.open_draft(Some(id), item, None, window, cx);
    }

    /// Open the sheet with fields taken from `template`.
    fn open_draft(
        &mut self,
        id: Option<String>,
        template: Item,
        parent_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("untitled"))
                .default_value(template.title)
        });
        let subscription = cx.subscribe_in(&title_input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_draft(window, cx);
            }
        });
        self.edit_draft = Some(EditDraft {
            id,
            kind: template.kind,
            title_input: title_input.clone(),
            folder_mode: template.folder_mode,
            include: template.include,
            parent_id,
            _subscription: subscription,
        });

        let view = cx.entity();
        let focus_once = Cell::new(true);
        window.open_sheet(cx, move |sheet, window, cx| {
            if focus_once.replace(false) {
                title_input.update(cx, |state, cx| {
                    state.focus(window, cx);
                    state.select_all(window, cx);
                });
            }
            Self::render_edit_sheet(&view, sheet, window, cx)
        });
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditDraft)) {
        if let Some(draft) = &mut self.edit_draft {
            f(draft);
            cx.notify();
        }
    }

    fn submit_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.edit_draft.take() else {
            return;
        };
        let title = draft.title_input.read(cx).value().to_string();
        window.close_sheet(cx);

        let Some(id) = draft.id else {
            let mut item = Item::new(draft.kind, title);
            item.folder_mode = draft.folder_mode;
            item.include = draft.include;
            let id = item.id.clone();
            let parent = draft.parent_id;
            self.mutate(window, cx, |store| store.insert(item, parent.as_deref()));
            self.select(Selection::Item(id), window, cx);
            if draft.kind == ItemKind::Local {
                self.editor.update(cx, |state, cx| state.focus(window, cx));
            }
            return;
        };

        let (mode, include) = (draft.folder_mode, draft.include);
        self.mutate(window, cx, |store| {
            let Some(item) = store.find_mut(&id) else {
                return;
            };
            item.title = title;
            match item.kind {
                ItemKind::Folder => item.folder_mode = mode,
                ItemKind::Group => item.include = include,
                ItemKind::Local => {}
            }
        });
    }

    fn toggle_include(&mut self, id: String, cx: &mut Context<Self>) {
        self.update_draft(cx, |d| match d.include.iter().position(|i| *i == id) {
            Some(ix) => {
                d.include.remove(ix);
            }
            None => d.include.push(id),
        });
    }

    fn render_edit_sheet(view: &Entity<Self>, sheet: Sheet, window: &Window, cx: &App) -> Sheet {
        let Some(draft) = &view.read(cx).edit_draft else {
            return sheet;
        };
        let theme = cx.theme();
        let (fg, muted, danger) = (theme.foreground, theme.muted_foreground, theme.danger);
        let (primary, primary_fg) = (theme.primary, theme.primary_foreground);
        let editing = draft.id.is_some();
        let kind = draft.kind;

        let kind_view = view.clone();
        let kinds = ToggleGroup::new("draft-kind")
            .outline()
            .segmented()
            .w_full()
            .disabled(editing)
            .when(editing, |this| this.opacity(0.6))
            .children(KINDS.map(|k| {
                let checked = k == kind;
                Toggle::new(ElementId::Name(format!("kind-{k:?}").into()))
                    .flex_1()
                    .gap_1p5()
                    .icon(kind_icon(k))
                    .label(kind_label(k))
                    .checked(checked)
                    .when(!editing, |this| this.cursor_pointer())
                    .when(checked, |this| {
                        this.bg(primary).text_color(primary_fg).border_color(primary)
                    })
            }))
            .on_click(move |checks: &Vec<bool>, _, cx| {
                let picked = checks
                    .iter()
                    .zip(KINDS)
                    .find(|(checked, k)| **checked && *k != kind)
                    .map(|(_, k)| k);
                if let Some(picked) = picked {
                    kind_view.update(cx, |this, cx| this.update_draft(cx, |d| d.kind = picked));
                }
            });
        let type_field = field(t!("hosts_type"), fg).child(kinds).when(editing, |this| {
            this.child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Lock).xsmall().text_color(muted))
                    .child(t!("type_locked")),
            )
        });

        let extra = match kind {
            ItemKind::Group => {
                Some(field(t!("content"), fg).child(Self::render_include_picker(view, &draft.include, cx)))
            }
            ItemKind::Folder => {
                let mode_view = view.clone();
                let description = match draft.folder_mode {
                    FolderMode::Default => t!("choice_default_effect"),
                    FolderMode::Single => t!("choice_single_effect"),
                    FolderMode::Multi => t!("choice_multi_effect"),
                };
                Some(
                    field(t!("choice_mode"), fg)
                        .child(
                            RadioGroup::horizontal("draft-folder-mode")
                                .selected_index(FOLDER_MODES.iter().position(|m| *m == draft.folder_mode))
                                .children(FOLDER_MODES.map(|mode| {
                                    Radio::new(ElementId::Name(format!("mode-{mode:?}").into()))
                                        .label(folder_mode_label(mode))
                                        .cursor_pointer()
                                }))
                                .on_click(move |ix: &usize, _, cx| {
                                    let mode = FOLDER_MODES[*ix];
                                    mode_view.update(cx, |this, cx| this.update_draft(cx, |d| d.folder_mode = mode));
                                }),
                        )
                        .child(div().text_xs().text_color(muted).child(description)),
                )
            }
            ItemKind::Local => None,
        };

        // The sheet pads the body horizontally only; the vertical rhythm is ours.
        let body = v_flex()
            .flex_shrink_0()
            .gap_5()
            .pt_2()
            .pb_5()
            .child(type_field)
            .child(field(t!("hosts_title"), fg).child(Input::new(&draft.title_input)))
            .children(extra);

        let delete = draft.id.clone().map(|id| {
            let view = view.clone();
            Button::new("draft-delete")
                .custom(
                    ButtonCustomVariant::new(cx)
                        .foreground(danger)
                        .hover(danger.opacity(0.1))
                        .active(danger.opacity(0.15)),
                )
                .icon(IconName::Trash)
                .label(t!("move_to_trash"))
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    view.update(cx, |this, cx| this.confirm_delete(id.clone(), true, window, cx))
                })
        });
        let (cancel_view, ok_view, close_view) = (view.clone(), view.clone(), view.clone());
        let footer = sheet_footer(cx).child(div().children(delete)).child(
            h_flex()
                .gap_2()
                .child(Button::new("draft-cancel").outline().label(t!("cancel")).cursor_pointer().on_click(
                    move |_, window, cx| {
                        cancel_view.update(cx, |this, _| this.edit_draft = None);
                        window.close_sheet(cx);
                    },
                ))
                .child(Button::new("draft-ok").primary().label(t!("ok")).cursor_pointer().on_click(
                    move |_, window, cx| ok_view.update(cx, |this, cx| this.submit_draft(window, cx)),
                )),
        );

        let title = if editing { t!("hosts_edit") } else { t!("hosts_add") };

        sheet
            .title(SharedString::from(title))
            .size(sheet_width(px(440.), window))
            .child(body)
            .footer(footer)
            // Esc, the close button and the overlay close the sheet without the footer.
            .on_close(move |_, _, cx| close_view.update(cx, |this, _| this.edit_draft = None))
    }

    /// Two lists of local items; clicking an entry moves it to the other list.
    fn render_include_picker(view: &Entity<Self>, include: &[String], cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (border, hover, muted) = (theme.border, theme.list_hover, theme.muted_foreground);
        let locals = view.read(cx).store.locals();
        if locals.is_empty() {
            return div()
                .h(px(72.))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(border)
                .rounded(px(6.))
                .text_sm()
                .text_color(muted)
                .child(t!("no_local_hosts"))
                .into_any_element();
        }

        let available: Vec<(String, SharedString)> = locals
            .iter()
            .filter(|(id, _)| !include.contains(id))
            .map(|(id, title)| (id.clone(), display_title(title)))
            .collect();
        let chosen: Vec<(String, SharedString)> = include
            .iter()
            .filter_map(|id| {
                let (_, title) = locals.iter().find(|(lid, _)| lid == id)?;
                Some((id.clone(), display_title(title)))
            })
            .collect();

        let list = |name: &'static str,
                    heading: Cow<'static, str>,
                    empty: Cow<'static, str>,
                    action: IconName,
                    entries: Vec<(String, SharedString)>| {
            let count = entries.len();
            let rows: Vec<_> = entries
                .into_iter()
                .map(|(id, title)| {
                    let view = view.clone();
                    h_flex()
                        .id(ElementId::Name(format!("{name}-{id}").into()))
                        .flex_shrink_0()
                        .h(px(28.))
                        .px_2()
                        .gap_2()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_sm()
                        .hover(|s| s.bg(hover))
                        .child(Icon::new(IconName::FileText).small().text_color(muted))
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(Icon::new(action).xsmall().text_color(muted))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.toggle_include(id.clone(), cx))
                        })
                })
                .collect();
            let content = if rows.is_empty() {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(muted)
                    .child(empty)
                    .into_any_element()
            } else {
                v_flex()
                    .id(name)
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .gap_0p5()
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            };
            v_flex()
                .flex_1()
                .min_w_0()
                .h(px(220.))
                .border_1()
                .border_color(border)
                .rounded(px(6.))
                .overflow_hidden()
                .child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_1p5()
                        .border_b_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{heading} ({count})")),
                )
                .child(content)
        };

        h_flex()
            .gap_2()
            .child(list(
                "available",
                t!("include_available"),
                t!("include_none_available"),
                IconName::Plus,
                available,
            ))
            .child(list(
                "chosen",
                t!("include_chosen"),
                t!("include_click_to_add"),
                IconName::Minus,
                chosen,
            ))
            .into_any_element()
    }
}
