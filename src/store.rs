use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::i18n::{Localize as _, LocalizedError};

const HISTORY_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    #[default]
    Local,
    Remote,
    Folder,
    Group,
}

impl ItemKind {
    /// Whether items of this kind carry hosts text of their own.
    pub fn has_content(self) -> bool {
        matches!(self, Self::Local | Self::Remote)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderMode {
    /// Follows the top-level choice mode (`Store::multi_select`).
    #[default]
    Default,
    /// Enabling a child disables its siblings.
    Single,
    /// Any number of children may be enabled.
    Multi,
}

/// Where a dragged item lands relative to the row it is dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPosition {
    Before,
    After,
    /// At the end of a folder's children.
    Inside,
}

impl DropPosition {
    /// The position for a pointer `y` pixels below the top of a row `height`
    /// high, as in SwitchHosts: halves for items, and quarters before and
    /// after the inside band for folders.
    pub fn at(y: f32, height: f32, folder: bool) -> Self {
        let edge = if folder { height / 4. } else { height / 2. };
        if y <= edge {
            Self::Before
        } else if y >= height - edge {
            Self::After
        } else {
            Self::Inside
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub kind: ItemKind,
    #[serde(default)]
    pub on: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Item>,
    #[serde(default)]
    pub folder_mode: FolderMode,
    #[serde(default)]
    pub collapsed: bool,
    /// Ids of content items (local or remote) whose content a group contributes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Where a remote item fetches its content from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Seconds between remote refreshes; 0 means manual only.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub refresh_interval: u64,
    /// Unix seconds of the last remote refresh attempt, success or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<i64>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl Item {
    pub fn new(kind: ItemKind, title: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            title: title.into(),
            kind,
            ..Default::default()
        }
    }

    fn contains(&self, id: &str) -> bool {
        self.id == id || self.children.iter().any(|c| c.contains(id))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrashItem {
    pub item: Item,
    pub parent_id: Option<String>,
    pub deleted_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrashedProfile {
    pub profile: crate::browser::BrowserProfile,
    pub deleted_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub trash: Vec<TrashItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<crate::browser::BrowserProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trash_profiles: Vec<TrashedProfile>,
    /// Whether multiple top-level items may be enabled at the same time.
    #[serde(default = "default_true")]
    pub multi_select: bool,
}

fn default_true() -> bool {
    true
}

pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("SwitchHostsLite")
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value)?;
    // Write a sibling file first, so a crash mid-write never leaves a truncated file.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}

/// Read a JSON file, or `default()` when it is missing or unreadable.
pub(crate) fn load_json<T: DeserializeOwned>(path: &Path, default: impl FnOnce() -> T) -> T {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("failed to parse {}: {err}", path.display());
            // Keep the unreadable file, the next save would overwrite it.
            fs::rename(path, path.with_extension("json.bak")).ok();
            default()
        }),
        Err(_) => default(),
    }
}

impl Store {
    fn path() -> PathBuf {
        data_dir().join("data.json")
    }

    pub fn load() -> Self {
        load_json(&Self::path(), Self::with_defaults)
    }

    fn with_defaults() -> Self {
        Self {
            items: Vec::new(),
            trash: Vec::new(),
            profiles: Vec::new(),
            trash_profiles: Vec::new(),
            multi_select: true,
        }
    }

    pub fn save(&self) -> Result<()> {
        write_json(&Self::path(), self)
    }

    pub fn find(&self, id: &str) -> Option<&Item> {
        fn walk<'a>(items: &'a [Item], id: &str) -> Option<&'a Item> {
            items
                .iter()
                .find_map(|i| if i.id == id { Some(i) } else { walk(&i.children, id) })
        }
        walk(&self.items, id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Item> {
        fn walk<'a>(items: &'a mut [Item], id: &str) -> Option<&'a mut Item> {
            for item in items {
                if item.id == id {
                    return Some(item);
                }
                if let Some(found) = walk(&mut item.children, id) {
                    return Some(found);
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    /// `Some(None)` for a top-level item, `None` when the id is unknown.
    pub fn parent_of(&self, id: &str) -> Option<Option<String>> {
        fn walk(items: &[Item], parent: Option<&str>, id: &str) -> Option<Option<String>> {
            for item in items {
                if item.id == id {
                    return Some(parent.map(str::to_string));
                }
                if let Some(found) = walk(&item.children, Some(&item.id), id) {
                    return Some(found);
                }
            }
            None
        }
        walk(&self.items, None, id)
    }

    fn take(&mut self, id: &str) -> Option<Item> {
        fn walk(items: &mut Vec<Item>, id: &str) -> Option<Item> {
            if let Some(ix) = items.iter().position(|i| i.id == id) {
                return Some(items.remove(ix));
            }
            items.iter_mut().find_map(|i| walk(&mut i.children, id))
        }
        walk(&mut self.items, id)
    }

    /// Insert into the folder `parent`, or at the top level when it is not a folder.
    pub fn insert(&mut self, item: Item, parent: Option<&str>) {
        match parent.and_then(|p| self.find_mut(p)) {
            Some(folder) if folder.kind == ItemKind::Folder => {
                folder.collapsed = false;
                folder.children.push(item);
            }
            _ => self.items.push(item),
        }
    }

    pub fn move_to(&mut self, id: &str, parent: Option<&str>) {
        if let (Some(item), Some(parent)) = (self.find(id), parent)
            && item.contains(parent)
        {
            return;
        }
        if let Some(item) = self.take(id) {
            self.insert(item, parent);
        }
    }

    /// Whether `id` can be dropped at `position` relative to `target`: never
    /// onto itself or into its own subtree, and inside folders only.
    pub fn can_drop(&self, id: &str, target: &str, position: DropPosition) -> bool {
        let (Some(item), Some(target)) = (self.find(id), self.find(target)) else {
            return false;
        };
        !item.contains(&target.id) && (position != DropPosition::Inside || target.kind == ItemKind::Folder)
    }

    /// Move `id` next to `target`, or to the end of the folder `target`, like
    /// SwitchHosts' tree drag. Switches stay as they are.
    pub fn move_item(&mut self, id: &str, target: &str, position: DropPosition) -> bool {
        if !self.can_drop(id, target, position) {
            return false;
        }
        let Some(item) = self.take(id) else {
            return false;
        };
        if position == DropPosition::Inside {
            self.insert(item, Some(target));
            return true;
        }
        let Some(path) = path_of(&self.items, target) else {
            return false;
        };
        let (&ix, parent) = path.split_last().expect("paths are never empty");
        let siblings = match parent {
            [] => &mut self.items,
            _ => &mut node_mut(&mut self.items, parent).children,
        };
        let ix = if position == DropPosition::After { ix + 1 } else { ix };
        siblings.insert(ix, item);
        true
    }

    pub fn delete(&mut self, id: &str) {
        let parent_id = self.parent_of(id).flatten();
        if let Some(item) = self.take(id) {
            self.trash.insert(
                0,
                TrashItem {
                    item,
                    parent_id,
                    deleted_at: now(),
                },
            );
        }
    }

    pub fn restore(&mut self, id: &str) -> Option<String> {
        let ix = self.trash.iter().position(|t| t.item.id == id)?;
        let TrashItem {
            mut item,
            parent_id,
            ..
        } = self.trash.remove(ix);
        set_subtree(&mut item, false);
        let id = item.id.clone();
        self.insert(item, parent_id.as_deref());
        Some(id)
    }

    pub fn purge(&mut self, id: &str) {
        self.trash.retain(|t| t.item.id != id);
    }

    // -- browser profiles ----------------------------------------------------

    pub fn profile(&self, id: &str) -> Option<&crate::browser::BrowserProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn profile_mut(&mut self, id: &str) -> Option<&mut crate::browser::BrowserProfile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    pub fn trashed_profile(&self, id: &str) -> Option<&TrashedProfile> {
        self.trash_profiles.iter().find(|t| t.profile.id == id)
    }

    pub fn add_profile(&mut self, name: impl Into<String>) -> String {
        let mut profile = crate::browser::BrowserProfile::new(name);
        profile.tile_color =
            Some(crate::browser::TILE_PALETTE[self.profiles.len() % crate::browser::TILE_PALETTE.len()]);
        let id = profile.id.clone();
        self.profiles.push(profile);
        id
    }

    pub fn duplicate_profile(&mut self, id: &str) -> Option<String> {
        let ix = self.profiles.iter().position(|p| p.id == id)?;
        let mut copy = self.profiles[ix].clone();
        copy.id = new_id();
        copy.revs.clear();
        copy.last_synced_name = None;
        copy.last_synced_color = None;
        let new_id = copy.id.clone();
        if copy.custom_image
            && std::fs::copy(crate::tile::image_path(id), crate::tile::image_path(&new_id)).is_err()
        {
            copy.custom_image = false;
        }
        self.profiles.insert(ix + 1, copy);
        Some(new_id)
    }

    pub fn delete_profile(&mut self, id: &str) {
        let Some(ix) = self.profiles.iter().position(|p| p.id == id) else {
            return;
        };
        let profile = self.profiles.remove(ix);
        self.trash_profiles.insert(
            0,
            TrashedProfile {
                profile,
                deleted_at: chrono::Local::now().timestamp(),
            },
        );
    }

    /// Restore the profile as it was deleted; profiles only read the enabled
    /// hosts rules, so there is no switch to reset.
    pub fn restore_profile(&mut self, id: &str) -> Option<String> {
        let ix = self.trash_profiles.iter().position(|t| t.profile.id == id)?;
        let profile = self.trash_profiles.remove(ix).profile;
        let id = profile.id.clone();
        self.profiles.push(profile);
        Some(id)
    }

    pub fn purge_profile(&mut self, id: &str) {
        self.trash_profiles.retain(|t| t.profile.id != id);
        crate::tile::remove(id);
        crate::tile::remove_image(id);
    }

    /// Whether enabling a child of `folder` disables its siblings.
    pub fn single_choice(&self, folder: &Item) -> bool {
        match folder.folder_mode {
            FolderMode::Default => !self.multi_select,
            mode => mode == FolderMode::Single,
        }
    }

    /// Switch an item like SwitchHosts' `setOnStateOfItem` with "sync child
    /// items when toggling folders" enabled: a folder passes the switch down
    /// to its children, ancestor folders follow their children, and turning
    /// an item on under single choice turns its siblings off. Folders in
    /// single mode neither pass the switch down nor follow their children.
    pub fn set_on(&mut self, id: &str, on: bool) {
        let Some(path) = path_of(&self.items, id) else {
            return;
        };
        let item = node_mut(&mut self.items, &path);
        item.on = on;
        cascade(item, on);
        for depth in (1..path.len()).rev() {
            let parent = node_mut(&mut self.items, &path[..depth]);
            if parent.folder_mode == FolderMode::Single {
                break;
            }
            parent.on = on && parent.children.iter().all(|c| c.on);
        }
        if !on {
            return;
        }

        let Some((&ix, parent)) = path.split_last() else {
            return;
        };
        let single = match parent {
            [] => !self.multi_select,
            _ => self.single_choice(node(&self.items, parent)),
        };
        if !single {
            return;
        }
        let siblings = match parent {
            [] => &mut self.items,
            _ => &mut node_mut(&mut self.items, parent).children,
        };
        for (i, sibling) in siblings.iter_mut().enumerate() {
            if i != ix {
                sibling.on = false;
                cascade(sibling, false);
            }
        }
    }

    /// Folders in display order with their depth, excluding `exclude` and its subtree.
    pub fn folders(&self, exclude: &str) -> Vec<(String, String, usize)> {
        fn walk(items: &[Item], depth: usize, exclude: &str, out: &mut Vec<(String, String, usize)>) {
            for item in items {
                if item.kind == ItemKind::Folder && item.id != exclude {
                    out.push((item.id.clone(), item.title.clone(), depth));
                    walk(&item.children, depth + 1, exclude, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, 0, exclude, &mut out);
        out
    }

    pub fn locals(&self) -> Vec<(String, String)> {
        fn walk(items: &[Item], out: &mut Vec<(String, String)>) {
            for item in items {
                if item.kind.has_content() {
                    out.push((item.id.clone(), item.title.clone()));
                }
                walk(&item.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    /// Content contributed by a group: its included items, in order.
    pub fn group_content(&self, group: &Item) -> String {
        let mut seen = HashSet::new();
        let mut parts = Vec::new();
        for id in &group.include {
            if let Some(item) = self.find(id)
                && item.kind.has_content()
                && seen.insert(item.id.clone())
            {
                parts.push(section(item));
            }
        }
        parts.join("\n")
    }

    /// Content of every local or remote item that is on or included by a group
    /// that is on, in tree order. A folder's own switch contributes nothing.
    pub fn enabled_content(&self) -> String {
        self.collect(&self.items, true)
    }

    /// Sections contributed by `items`, recursing into folders and groups.
    /// With `only_on`, items that are switched off are skipped.
    fn collect(&self, items: &[Item], only_on: bool) -> String {
        self.collect_with(items, |item| item.on || !only_on)
    }

    /// Sections contributed by `items` whose switch `on` reports as on; a
    /// switched-on group adds the content items it includes.
    fn collect_with(&self, items: &[Item], on: impl Fn(&Item) -> bool) -> String {
        fn walk<'a>(
            store: &'a Store,
            items: &'a [Item],
            on: &dyn Fn(&Item) -> bool,
            seen: &mut HashSet<&'a str>,
            out: &mut Vec<String>,
        ) {
            for item in items {
                match item.kind {
                    ItemKind::Local | ItemKind::Remote => {
                        if on(item) && seen.insert(&item.id) {
                            out.push(section(item));
                        }
                    }
                    ItemKind::Folder => walk(store, &item.children, on, seen, out),
                    ItemKind::Group if on(item) => {
                        for id in &item.include {
                            if let Some(local) = store.find(id)
                                && local.kind.has_content()
                                && seen.insert(&local.id)
                            {
                                out.push(section(local));
                            }
                        }
                    }
                    ItemKind::Group => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(self, items, &on, &mut HashSet::new(), &mut out);
        out.join("\n")
    }

    /// The text an item stands for: a content item's own text, what the
    /// enabled items inside a folder contribute (every item with
    /// `whole_folder`), or a group's included items.
    pub fn content_of(&self, item: &Item, whole_folder: bool) -> String {
        match item.kind {
            ItemKind::Local | ItemKind::Remote => item.content.clone(),
            ItemKind::Folder => self.collect(&item.children, !whole_folder),
            ItemKind::Group => self.group_content(item),
        }
    }

    /// Remote items whose refresh interval has elapsed: `(id, url)` pairs in
    /// tree order. A failed attempt stamps `last_refresh` too, so retries
    /// wait a full interval instead of hammering on every scan.
    pub fn due_remotes(&self, now: i64) -> Vec<(String, String)> {
        fn walk(items: &[Item], now: i64, out: &mut Vec<(String, String)>) {
            for item in items {
                if item.kind == ItemKind::Remote
                    && item.refresh_interval > 0
                    && crate::remote::valid_url(&item.url)
                    && item.last_refresh.is_none_or(|t| now - t >= item.refresh_interval as i64)
                {
                    out.push((item.id.clone(), item.url.clone()));
                }
                walk(&item.children, now, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.items, now, &mut out);
        out
    }

    // -- browser profile switches -------------------------------------------

    /// The hosts content a profile's switches stand for: the content items that
    /// are on, or included by a group that is on, in tree order.
    pub fn switched_content(&self, switches: &[String]) -> String {
        let on: HashSet<&str> = switches.iter().map(String::as_str).collect();
        self.collect_with(&self.items, |item| on.contains(item.id.as_str()))
    }

    /// Switch `id` in a profile's own switch list, following the same rules
    /// as `set_on`: a folder passes the switch down, ancestor folders follow,
    /// and single choice turns the siblings off. The result is stored in tree
    /// order and unknown ids are dropped.
    pub fn set_switched(&self, switches: &[String], id: &str, on: bool) -> Vec<String> {
        let Some(path) = path_of(&self.items, id) else {
            return switches.to_vec();
        };
        let mut set: HashSet<String> = switches.iter().cloned().collect();
        set_member(&mut set, id, on);
        let item = node(&self.items, &path);
        cascade_members(item, &mut set, on);
        for depth in (1..path.len()).rev() {
            let parent = node(&self.items, &path[..depth]);
            if parent.folder_mode == FolderMode::Single {
                break;
            }
            let all = on && parent.children.iter().all(|c| set.contains(&c.id));
            set_member(&mut set, &parent.id, all);
        }
        if on {
            let Some((&ix, parent)) = path.split_last() else {
                return switches.to_vec();
            };
            let single = match parent {
                [] => !self.multi_select,
                _ => self.single_choice(node(&self.items, parent)),
            };
            if single {
                let siblings = match parent {
                    [] => &self.items,
                    _ => &node(&self.items, parent).children,
                };
                for (i, sibling) in siblings.iter().enumerate() {
                    if i != ix {
                        remove_subtree(sibling, &mut set);
                    }
                }
            }
        }
        let mut out = Vec::new();
        emit_members(&self.items, &set, &mut out);
        out
    }

    /// Whether the item's content currently ends up in the system hosts:
    /// it is on, or a group that is on includes it.
    pub fn is_effective(&self, id: &str) -> bool {
        fn included(items: &[Item], id: &str) -> bool {
            items.iter().any(|item| {
                (item.kind == ItemKind::Group && item.on && item.include.iter().any(|i| i == id))
                    || included(&item.children, id)
            })
        }
        self.find(id).is_some_and(|item| item.on) || included(&self.items, id)
    }

    /// Replace all items with `imported`, moving the current ones to the trash.
    pub fn replace_items(&mut self, imported: Vec<Item>) {
        let current = std::mem::take(&mut self.items);
        let deleted_at = now();
        for item in current.into_iter().rev() {
            self.trash.insert(
                0,
                TrashItem {
                    item,
                    parent_id: None,
                    deleted_at,
                },
            );
        }
        self.items = imported;
    }

    /// Replace all profiles with `imported`, moving the current ones to the trash.
    pub fn replace_profiles(&mut self, imported: Vec<crate::browser::BrowserProfile>) {
        let current = std::mem::take(&mut self.profiles);
        let deleted_at = now();
        for profile in current.into_iter().rev() {
            self.trash_profiles.insert(0, TrashedProfile { profile, deleted_at });
        }
        self.profiles = imported;
    }
}

/// Child indices leading from `items` to the item `id`.
fn path_of(items: &[Item], id: &str) -> Option<Vec<usize>> {
    items.iter().enumerate().find_map(|(ix, item)| {
        if item.id == id {
            return Some(vec![ix]);
        }
        let mut path = path_of(&item.children, id)?;
        path.insert(0, ix);
        Some(path)
    })
}

fn node<'a>(items: &'a [Item], path: &[usize]) -> &'a Item {
    let item = &items[path[0]];
    path[1..].iter().fold(item, |item, &ix| &item.children[ix])
}

fn node_mut<'a>(items: &'a mut [Item], path: &[usize]) -> &'a mut Item {
    let item = &mut items[path[0]];
    path[1..].iter().fold(item, |item, &ix| &mut item.children[ix])
}

/// Pass a folder's switch down to its children, except in single mode.
fn cascade(item: &mut Item, on: bool) {
    if item.kind != ItemKind::Folder || item.folder_mode == FolderMode::Single {
        return;
    }
    for child in &mut item.children {
        child.on = on;
        cascade(child, on);
    }
}

fn set_subtree(item: &mut Item, on: bool) {
    item.on = on;
    for child in &mut item.children {
        set_subtree(child, on);
    }
}

fn set_member(set: &mut HashSet<String>, id: &str, on: bool) {
    if on {
        set.insert(id.to_string());
    } else {
        set.remove(id);
    }
}

/// Pass a folder's switch down to a profile's switch set, except in single mode.
fn cascade_members(item: &Item, set: &mut HashSet<String>, on: bool) {
    if item.kind != ItemKind::Folder || item.folder_mode == FolderMode::Single {
        return;
    }
    for child in &item.children {
        set_member(set, &child.id, on);
        cascade_members(child, set, on);
    }
}

fn remove_subtree(item: &Item, set: &mut HashSet<String>) {
    set.remove(&item.id);
    // Like `cascade`, a single-mode folder does not pass the switch down, so
    // single choice leaves its children's switches alone.
    if item.kind == ItemKind::Folder && item.folder_mode == FolderMode::Single {
        return;
    }
    for child in &item.children {
        remove_subtree(child, set);
    }
}

fn emit_members(items: &[Item], set: &HashSet<String>, out: &mut Vec<String>) {
    for item in items {
        if set.contains(&item.id) {
            out.push(item.id.clone());
        }
        emit_members(&item.children, set, out);
    }
}

/// Pasted text may bring CRLF line ends, which would end up in host names.
fn section(item: &Item) -> String {
    let content = item.content.trim_end().replace("\r\n", "\n");
    format!("# --- {} ---\n{content}\n", item.title)
}

/// Number of lines that are neither blank nor comments.
pub fn count_rules(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count()
}

// ---------------------------------------------------------------------------
// History of written hosts files.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub content: String,
    pub created_at: i64,
    #[serde(skip)]
    line_count: std::cell::OnceCell<usize>,
}

impl HistoryEntry {
    pub fn line_count(&self) -> usize {
        *self.line_count.get_or_init(|| self.content.lines().count())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    fn path() -> PathBuf {
        data_dir().join("history.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        write_json(&Self::path(), self)
    }

    /// Record a write of the system hosts from `previous` to `next`: both are
    /// pushed, each skipped when it equals the newest entry, and nothing is
    /// recorded when the content did not change. Returns whether any entry
    /// was added.
    pub fn record_change(&mut self, previous: &str, next: &str) -> bool {
        if previous == next {
            return false;
        }
        let pushed = self.record(previous);
        self.record(next) || pushed
    }

    /// Record `content` as the newest entry unless it equals the latest one.
    fn record(&mut self, content: &str) -> bool {
        if self.entries.first().is_some_and(|e| e.content == content) {
            return false;
        }
        self.entries.insert(
            0,
            HistoryEntry {
                id: new_id(),
                content: content.to_string(),
                created_at: now(),
                line_count: Default::default(),
            },
        );
        self.entries.truncate(HISTORY_LIMIT);
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
    }
}

// ---------------------------------------------------------------------------
// Import / export.

/// The backup layout written by earlier versions of this app (`"app": "SwitchHostsLite"`).
#[derive(Deserialize)]
struct LegacyBackup {
    multi_select: bool,
    items: Vec<Item>,
}

/// Export in the SwitchHosts v4 layout, importable by SwitchHosts v4 and v5.
pub fn export(store: &Store, path: &Path) -> Result<()> {
    write_json(path, &crate::switchhosts::export_v4(store))
}

/// An imported backup: the items, the top-level choice mode when the backup
/// names it, and the browser profiles this app carries (`None` leaves the
/// current profiles untouched).
pub type Imported = (Vec<Item>, Option<bool>, Option<Vec<crate::browser::BrowserProfile>>);

/// Parse a backup written by this app or by SwitchHosts (v3/v4/v5). Imported
/// items get fresh ids so they never collide with items in the trash.
pub fn import(text: &str) -> Result<Imported> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let value: Value = serde_json::from_str(text).localize("err_invalid_json")?;

    if crate::switchhosts::is_backup(&value) {
        let multi = crate::switchhosts::multi_select(&value);
        let (items, profiles) = crate::switchhosts::parse(&value)?;
        return Ok((items, multi, profiles));
    }

    if value.get("app").and_then(Value::as_str) == Some("SwitchHostsLite") {
        let mut backup: LegacyBackup = serde_json::from_value(value).localize("err_invalid_backup")?;
        let mut ids = HashMap::new();
        refresh_ids(&mut backup.items, &mut ids);
        crate::switchhosts::remap_includes(&mut backup.items, &ids);
        return Ok((backup.items, Some(backup.multi_select), None));
    }

    Err(LocalizedError::new("err_unknown_format").into())
}

fn refresh_ids(items: &mut [Item], ids: &mut HashMap<String, String>) {
    for item in items {
        let id = new_id();
        ids.insert(std::mem::replace(&mut item.id, id.clone()), id);
        refresh_ids(&mut item.children, ids);
    }
}

pub fn now() -> i64 {
    chrono::Local::now().timestamp()
}

pub fn format_time(ts: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub(crate) fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = chrono::Local::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{nanos:x}{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(title: &str, content: &str, on: bool) -> Item {
        let mut item = Item::new(ItemKind::Local, title);
        item.content = content.into();
        item.on = on;
        item
    }

    fn folder(title: &str, mode: FolderMode, on: bool, children: Vec<Item>) -> Item {
        let mut item = Item::new(ItemKind::Folder, title);
        item.folder_mode = mode;
        item.on = on;
        item.children = children;
        item
    }

    fn store(items: Vec<Item>, multi_select: bool) -> Store {
        Store {
            items,
            trash: Vec::new(),
            profiles: Vec::new(),
            trash_profiles: Vec::new(),
            multi_select,
        }
    }

    /// Titles of the items that are on, in tree order.
    fn on_titles(store: &Store) -> Vec<&str> {
        fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a str>) {
            for item in items {
                if item.on {
                    out.push(&item.title);
                }
                walk(&item.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&store.items, &mut out);
        out
    }

    /// Titles of the profile switches, in stored (tree) order.
    fn switched_titles<'a>(store: &'a Store, ids: &'a [String]) -> Vec<&'a str> {
        ids.iter()
            .filter_map(|id| store.find(id).map(|item| item.title.as_str()))
            .collect()
    }

    fn id_of(store: &Store, title: &str) -> String {
        fn walk(items: &[Item], title: &str) -> Option<String> {
            items.iter().find_map(|i| {
                if i.title == title {
                    Some(i.id.clone())
                } else {
                    walk(&i.children, title)
                }
            })
        }
        walk(&store.items, title).unwrap()
    }

    fn switch(store: &mut Store, title: &str, on: bool) {
        let id = id_of(store, title);
        store.set_on(&id, on);
    }

    #[test]
    fn new_items_start_off() {
        for kind in [ItemKind::Local, ItemKind::Folder, ItemKind::Group] {
            let item = Item::new(kind, "x");
            assert!(!item.on);
            assert_eq!(item.folder_mode, FolderMode::Default);
        }
    }

    #[test]
    fn every_item_that_is_on_applies_whatever_its_folder() {
        let a = local("a", "1.1.1.1 a", false);
        let mut group = Item::new(ItemKind::Group, "g");
        group.include = vec![a.id.clone()];
        group.on = true;
        let f = folder("f", FolderMode::Multi, false, vec![local("b", "2.2.2.2 b", true)]);
        let mut store = store(vec![a, f, group], true);

        let content = store.enabled_content();
        assert!(content.contains("1.1.1.1 a") && content.contains("2.2.2.2 b"));
        assert!(store.is_effective(&id_of(&store, "a")) && store.is_effective(&id_of(&store, "b")));

        store.find_mut(&id_of(&store, "f")).unwrap().on = true;
        assert_eq!(store.enabled_content(), content);
        store.find_mut(&id_of(&store, "b")).unwrap().on = false;
        assert!(!store.enabled_content().contains("2.2.2.2"));
    }

    #[test]
    fn folder_switch_reaches_every_descendant_and_ancestors_follow() {
        let inner = folder("inner", FolderMode::Multi, false, vec![local("c", "", false)]);
        let f = folder("f", FolderMode::Default, false, vec![local("a", "", false), inner]);
        let mut store = store(vec![f, local("x", "", true)], true);

        switch(&mut store, "f", true);
        assert_eq!(on_titles(&store), ["f", "a", "inner", "c", "x"]);

        switch(&mut store, "c", false);
        assert_eq!(on_titles(&store), ["a", "x"], "every ancestor follows a child off");

        switch(&mut store, "c", true);
        assert_eq!(on_titles(&store), ["f", "a", "inner", "c", "x"], "all children on: ancestors on");

        switch(&mut store, "a", false);
        switch(&mut store, "inner", false);
        assert_eq!(on_titles(&store), ["x"]);
        switch(&mut store, "a", true);
        assert_eq!(on_titles(&store), ["a", "x"], "a parent stays off while a child is off");
    }

    #[test]
    fn single_mode_folders_neither_pass_down_nor_follow() {
        let f = folder(
            "f",
            FolderMode::Single,
            false,
            vec![local("a", "", true), local("b", "", false)],
        );
        let mut store = store(vec![f], true);

        switch(&mut store, "f", true);
        assert_eq!(on_titles(&store), ["f", "a"]);
        switch(&mut store, "b", true);
        assert_eq!(on_titles(&store), ["f", "b"], "siblings are mutually exclusive");
        switch(&mut store, "b", false);
        assert_eq!(on_titles(&store), ["f"], "the folder does not follow its children");
        switch(&mut store, "f", false);
        assert!(on_titles(&store).is_empty());
    }

    #[test]
    fn default_folder_mode_follows_the_top_level_choice_mode() {
        let items = || {
            vec![folder(
                "f",
                FolderMode::Default,
                false,
                vec![local("a", "", true), local("b", "", false)],
            )]
        };
        let mut multi = store(items(), true);
        switch(&mut multi, "b", true);
        assert_eq!(on_titles(&multi), ["f", "a", "b"]);

        // SwitchHosts syncs the parent before it turns the siblings off.
        let mut single = store(items(), false);
        switch(&mut single, "b", true);
        assert_eq!(on_titles(&single), ["f", "b"]);
        assert!(single.single_choice(single.find(&id_of(&single, "f")).unwrap()));

        let mut explicit = store(items(), false);
        explicit.items[0].folder_mode = FolderMode::Multi;
        switch(&mut explicit, "b", true);
        assert_eq!(on_titles(&explicit), ["f", "a", "b"]);
    }

    #[test]
    fn top_level_single_choice_switches_other_top_level_trees_off() {
        let items = || {
            vec![
                folder("f", FolderMode::Multi, true, vec![local("a", "", true)]),
                folder("s", FolderMode::Single, true, vec![local("b", "", true)]),
                local("x", "", false),
                local("y", "", true),
            ]
        };
        let mut multi = store(items(), true);
        switch(&mut multi, "x", true);
        assert_eq!(on_titles(&multi), ["f", "a", "s", "b", "x", "y"]);

        let mut single = store(items(), false);
        switch(&mut single, "x", true);
        assert_eq!(on_titles(&single), ["b", "x"], "single-mode folders keep their children");
        switch(&mut single, "x", false);
        assert_eq!(on_titles(&single), ["b"], "switching off never switches anything on");
    }

    #[test]
    fn trash_roundtrip_restores_switched_off() {
        let f = folder("f", FolderMode::Multi, true, vec![local("a", "", true)]);
        let mut store = store(vec![f, local("b", "", true)], true);
        let (fid, aid) = (id_of(&store, "f"), id_of(&store, "a"));

        store.delete(&aid);
        assert!(store.find(&aid).is_none());
        store.delete(&fid);
        store.restore(&aid);
        assert_eq!(store.parent_of(&aid), Some(None), "the old parent is gone");
        store.restore(&fid);
        assert_eq!(on_titles(&store), ["b"]);

        store.move_to(&fid, Some(&aid));
        assert_eq!(store.parent_of(&fid), Some(None));
        store.move_to(&aid, Some(&fid));
        assert_eq!(store.parent_of(&aid), Some(Some(fid)));
    }

    #[test]
    fn switched_content_follows_folders_and_groups() {
        let a = local("a", "1.1.1.1 a", false);
        let inner = folder("inner", FolderMode::Multi, false, vec![local("c", "3.3.3.3 c", false)]);
        let f = folder(
            "f",
            FolderMode::Multi,
            false,
            vec![local("b", "2.2.2.2 b", false), inner],
        );
        let mut group = Item::new(ItemKind::Group, "g");
        group.include = vec![a.id.clone()];
        let store = store(vec![a, f, group], true);
        let (fid, gid) = (id_of(&store, "f"), id_of(&store, "g"));

        let content = store.switched_content(&store.set_switched(&[], &fid, true));
        assert!(content.contains("2.2.2.2 b") && content.contains("3.3.3.3 c"));
        assert!(!content.contains("1.1.1.1 a"));

        let content = store.switched_content(&store.set_switched(&[], &gid, true));
        assert!(content.contains("1.1.1.1 a"));

        assert!(store.switched_content(&[]).is_empty());
        assert!(store.switched_content(&["missing".into()]).is_empty());
    }

    #[test]
    fn profile_switches_cascade_and_follow_like_the_hosts_list() {
        let a = local("a", "1.1.1.1 a", false);
        let f = folder(
            "f",
            FolderMode::Multi,
            false,
            vec![local("b", "2.2.2.2 b", false), local("c", "3.3.3.3 c", false)],
        );
        let store = store(vec![a, f], true);
        let (aid, fid, bid) = (
            id_of(&store, "a"),
            id_of(&store, "f"),
            id_of(&store, "b"),
        );

        let sw = store.set_switched(&[], &fid, true);
        assert_eq!(switched_titles(&store, &sw), ["f", "b", "c"]);
        let sw = store.set_switched(&sw, &bid, false);
        assert_eq!(switched_titles(&store, &sw), ["c"], "one child off turns the folder off");
        let sw = store.set_switched(&sw, &bid, true);
        assert_eq!(
            switched_titles(&store, &sw),
            ["f", "b", "c"],
            "the folder follows once all children are on"
        );
        assert!(store.set_switched(&sw, &fid, false).is_empty());
        let sw = store.set_switched(&[], &aid, true);
        assert_eq!(switched_titles(&store, &sw), ["a"]);
        assert_eq!(
            store.set_switched(&sw, "missing", true),
            sw,
            "unknown ids leave the switches alone"
        );
    }

    #[test]
    fn profile_switches_follow_single_choice_and_groups() {
        let a = local("a", "1.1.1.1 a", false);
        let b = local("b", "2.2.2.2 b", false);
        let mut group = Item::new(ItemKind::Group, "g");
        group.include = vec![a.id.clone(), b.id.clone()];
        let grouped = store(vec![a, b, group], false);
        let (aid, gid) = (id_of(&grouped, "a"), id_of(&grouped, "g"));

        let sw = grouped.set_switched(&[], &aid, true);
        assert_eq!(switched_titles(&grouped, &sw), ["a"]);
        let sw = grouped.set_switched(&sw, &gid, true);
        assert_eq!(
            switched_titles(&grouped, &sw),
            ["g"],
            "top-level single choice turns the sibling off"
        );
        let content = grouped.switched_content(&sw);
        assert!(content.contains("1.1.1.1 a") && content.contains("2.2.2.2 b"));
        let sw = grouped.set_switched(&sw, &aid, true);
        assert_eq!(switched_titles(&grouped, &sw), ["a"]);

        let inner = folder(
            "single",
            FolderMode::Single,
            false,
            vec![local("x", "4.4.4.4 x", false), local("y", "5.5.5.5 y", false)],
        );
        let single = store(vec![inner], true);
        let (sid, xid) = (id_of(&single, "single"), id_of(&single, "x"));
        let sw = single.set_switched(&[], &sid, true);
        assert_eq!(
            switched_titles(&single, &sw),
            ["single"],
            "a single-mode folder does not pass the switch down"
        );
        let sw = single.set_switched(&[], &xid, true);
        assert_eq!(
            switched_titles(&single, &sw),
            ["x"],
            "a single-mode folder does not follow its children"
        );
    }

    #[test]
    fn single_choice_spares_a_single_mode_folders_children_like_set_on() {
        let make = || {
            let a = local("a", "1.1.1.1 a", false);
            let x = local("x", "4.4.4.4 x", true);
            let f = folder("f", FolderMode::Single, false, vec![x]);
            store(vec![a, f], false)
        };
        let mut hosts = make();
        let aid = id_of(&hosts, "a");
        hosts.set_on(&aid, true);
        assert!(hosts.enabled_content().contains("4.4.4.4 x"));

        let profiles = make();
        let (aid, xid) = (id_of(&profiles, "a"), id_of(&profiles, "x"));
        let sw = profiles.set_switched(&[xid], &aid, true);
        assert_eq!(switched_titles(&profiles, &sw), ["a", "x"]);
        assert!(profiles.switched_content(&sw).contains("4.4.4.4 x"));
    }

    #[test]
    fn new_profiles_get_palette_colors_round_robin() {
        let mut store = Store::default();
        let a = store.add_profile("a");
        let b = store.add_profile("b");
        assert_eq!(store.profile(&a).unwrap().tile_color, Some(crate::browser::TILE_PALETTE[0]));
        assert_eq!(store.profile(&b).unwrap().tile_color, Some(crate::browser::TILE_PALETTE[1]));
        let copy = store.duplicate_profile(&a).unwrap();
        assert_eq!(store.profile(&copy).unwrap().tile_color, Some(crate::browser::TILE_PALETTE[0]));
    }

    /// Titles in tree order, children in brackets.
    fn tree(store: &Store) -> String {
        fn walk(items: &[Item]) -> String {
            let titles: Vec<String> = items
                .iter()
                .map(|i| match i.kind {
                    ItemKind::Folder => format!("{}[{}]", i.title, walk(&i.children)),
                    _ => i.title.clone(),
                })
                .collect();
            titles.join(" ")
        }
        walk(&store.items)
    }

    fn drag(store: &mut Store, title: &str, target: &str, position: DropPosition) -> bool {
        let (id, target) = (id_of(store, title), id_of(store, target));
        store.move_item(&id, &target, position)
    }

    fn drag_store() -> Store {
        let inner = folder("inner", FolderMode::Default, false, vec![local("c", "", false)]);
        let f = folder("f", FolderMode::Default, false, vec![local("b", "", false), inner]);
        let empty = folder("empty", FolderMode::Default, false, Vec::new());
        store(vec![local("a", "", false), f, empty, local("d", "", false)], true)
    }

    #[test]
    fn drop_position_follows_switchhosts_zones() {
        use DropPosition::*;
        let at = |y, folder| DropPosition::at(y, 32., folder);
        let items = [at(0., false), at(16., false), at(17., false), at(32., false)];
        assert_eq!(items, [Before, Before, After, After]);
        let folders = [at(8., true), at(9., true), at(23., true), at(24., true)];
        assert_eq!(folders, [Before, Inside, Inside, After]);
    }

    #[test]
    fn move_item_reorders_and_reparents() {
        use DropPosition::*;
        let mut store = drag_store();
        assert!(drag(&mut store, "d", "a", Before));
        assert_eq!(tree(&store), "d a f[b inner[c]] empty[]");
        assert!(drag(&mut store, "d", "a", After));
        assert_eq!(tree(&store), "a d f[b inner[c]] empty[]");
        assert!(drag(&mut store, "a", "c", After), "into a nested folder");
        assert_eq!(tree(&store), "d f[b inner[c a]] empty[]");
        assert!(drag(&mut store, "c", "d", Before), "out to the top level");
        assert_eq!(tree(&store), "c d f[b inner[a]] empty[]");
        assert!(drag(&mut store, "d", "f", Inside), "appended to the folder");
        assert_eq!(tree(&store), "c f[b inner[a] d] empty[]");
        assert!(drag(&mut store, "inner", "empty", Inside));
        assert_eq!(tree(&store), "c f[b d] empty[inner[a]]");
        assert!(drag(&mut store, "f", "inner", Before), "folders move with their subtree");
        assert_eq!(tree(&store), "c empty[f[b d] inner[a]]");
    }

    #[test]
    fn move_item_rejects_invalid_drops() {
        use DropPosition::*;
        let mut store = drag_store();
        let before = tree(&store);
        assert!(!drag(&mut store, "f", "f", After), "onto itself");
        assert!(!drag(&mut store, "f", "c", Before), "into its own subtree");
        assert!(!drag(&mut store, "f", "inner", Inside), "into a descendant folder");
        assert!(!drag(&mut store, "a", "d", Inside), "inside an item");
        assert!(!store.move_item("missing", &id_of(&store, "a"), Before));
        assert!(!store.move_item(&id_of(&store, "a"), "missing", Before));
        assert_eq!(tree(&store), before);
    }

    #[test]
    fn move_item_keeps_switches_and_content_follows_tree_order() {
        use DropPosition::*;
        let mut store = store(
            vec![
                local("a", "1.1.1.1 a", true),
                folder("f", FolderMode::Multi, true, vec![local("b", "2.2.2.2 b", true)]),
                local("c", "3.3.3.3 c", false),
            ],
            true,
        );
        let fid = id_of(&store, "f");
        store.find_mut(&fid).unwrap().collapsed = true;
        assert!(drag(&mut store, "a", "f", Inside));
        assert!(!store.find(&fid).unwrap().collapsed, "the folder opens to show the item");
        assert_eq!(on_titles(&store), ["f", "b", "a"]);
        let content = store.enabled_content();
        assert!(content.find("2.2.2.2 b").unwrap() < content.find("1.1.1.1 a").unwrap());

        assert!(drag(&mut store, "c", "f", Before));
        assert_eq!(on_titles(&store), ["f", "b", "a"], "an item switched off stays off");
        assert!(drag(&mut store, "b", "c", Before));
        let content = store.enabled_content();
        assert!(content.find("2.2.2.2 b").unwrap() < content.find("1.1.1.1 a").unwrap());
        assert_eq!(on_titles(&store), ["b", "f", "a"]);
    }

    #[test]
    fn previews_of_folders_and_groups() {
        let a = local("a", "1.1.1.1 a\n# note", true);
        let b = local("b", "2.2.2.2 b", false);
        let idb = b.id.clone();
        let f = folder("f", FolderMode::Multi, false, vec![a, b]);
        let mut group = Item::new(ItemKind::Group, "g");
        group.include = vec![idb.clone()];
        let mut store = store(vec![f, group], true);

        assert!(!store.is_effective(&idb));
        store.find_mut(&id_of(&store, "g")).unwrap().on = true;
        assert!(store.is_effective(&idb));

        let f = store.find(&id_of(&store, "f")).unwrap();
        assert!(store.content_of(f, false).contains("1.1.1.1 a"));
        assert!(!store.content_of(f, false).contains("2.2.2.2"));
        assert!(store.content_of(f, true).contains("2.2.2.2"));
        assert_eq!(count_rules(&store.content_of(f, true)), 2);
        let g = store.find(&id_of(&store, "g")).unwrap();
        assert_eq!(store.content_of(g, false), "# --- b ---\n2.2.2.2 b\n");
    }

    #[test]
    fn enabled_content_has_no_crlf() {
        let store = Store {
            items: vec![local("a", "1.1.1.1 a\r\n2.2.2.2 b\r\n", true)],
            ..Default::default()
        };
        assert_eq!(store.enabled_content(), "# --- a ---\n1.1.1.1 a\n2.2.2.2 b\n");
    }

    fn remote(title: &str, url: &str, interval: u64, on: bool) -> Item {
        let mut item = Item::new(ItemKind::Remote, title);
        item.url = url.into();
        item.refresh_interval = interval;
        item.content = "1.1.1.1 remote".into();
        item.on = on;
        item
    }

    #[test]
    fn remote_items_contribute_like_local_ones() {
        let r = remote("r", "https://x/hosts", 60, true);
        let rid = r.id.clone();
        let mut group = Item::new(ItemKind::Group, "g");
        group.include = vec![rid.clone()];
        group.on = true;
        let store = store(vec![r, group], true);
        // The group includes it, but it appears only once.
        assert_eq!(store.enabled_content(), "# --- r ---\n1.1.1.1 remote\n");
        assert!(store.is_effective(&rid));
        assert_eq!(store.locals().len(), 1);
        assert_eq!(store.switched_content(std::slice::from_ref(&rid)), "# --- r ---\n1.1.1.1 remote\n");
    }

    #[test]
    fn due_remotes_respect_interval_and_last_attempt() {
        let mut never = remote("never", "https://x/a", 0, false);
        never.last_refresh = None;
        let mut fresh = remote("fresh", "https://x/b", 60, false);
        fresh.last_refresh = Some(1000);
        let mut due = remote("due", "https://x/c", 60, false);
        due.last_refresh = Some(940);
        let mut first = remote("first", "https://x/d", 300, false);
        first.last_refresh = None;
        let bad = remote("bad", "not a url", 60, false);
        let store = store(
            vec![folder("f", FolderMode::Multi, false, vec![never, fresh, due, first, bad])],
            true,
        );
        let due_ids: Vec<String> = store.due_remotes(1000).into_iter().map(|(id, _)| id).collect();
        assert_eq!(due_ids, [id_of(&store, "due"), id_of(&store, "first")]);
        // A failed attempt stamps last_refresh, so the next scan waits.
        let mut store = store;
        store.find_mut(&id_of(&store, "due")).unwrap().last_refresh = Some(1000);
        assert!(store.due_remotes(1059).iter().all(|(id, _)| *id != id_of(&store, "due")));
        assert!(store.due_remotes(1060).iter().any(|(id, _)| *id == id_of(&store, "due")));
    }

    #[test]
    fn remote_fields_survive_json_round_trip_with_defaults() {
        let r = remote("r", "https://x/hosts", 300, true);
        let text = serde_json::to_string(&r).unwrap();
        let back: Item = serde_json::from_str(&text).unwrap();
        assert_eq!(back.kind, ItemKind::Remote);
        assert_eq!(back.url, "https://x/hosts");
        assert_eq!(back.refresh_interval, 300);
        // Old data files carry none of the new fields.
        let old: Item = serde_json::from_str(r#"{"id":"a","title":"A","kind":"local"}"#).unwrap();
        assert_eq!(old.kind, ItemKind::Local);
        assert!(old.url.is_empty() && old.refresh_interval == 0 && old.last_refresh.is_none());
        let plain = serde_json::to_string(&old).unwrap();
        assert!(!plain.contains("url") && !plain.contains("refresh_interval") && !plain.contains("last_refresh"));
    }

    #[test]
    fn import_switchhosts_v4() {
        let text = r#"{"data":{"list":{"tree":[
            {"id":"x","title":"Dev","type":"local","on":true},
            {"id":"f","title":"Folder","type":"folder","on":true,"folder_mode":1,"children":[
                {"id":"y","title":"Remote","type":"remote","on":true}]}
        ]},"collection":{"hosts":{"data":[{"id":"x","content":"127.0.0.1 dev"}]}}},
        "version":[4,2,0,6100]}"#;
        let (items, multi, _) = import(text).unwrap();
        let store = store(items, true);
        assert_eq!(store.items[0].content, "127.0.0.1 dev");
        assert!(on_titles(&store).is_empty() && multi.is_none());
        assert_eq!(store.items[1].folder_mode, FolderMode::Single);
        assert_eq!(store.items[1].children[0].kind, ItemKind::Remote);
    }

    #[test]
    fn imports_legacy_backup() {
        let text = r#"{"app":"SwitchHostsLite","version":1,"multi_select":false,"items":[
            {"id":"a","title":"A","kind":"local","on":true,"content":"1.1.1.1 a"},
            {"id":"g","title":"G","kind":"group","include":["a"]}]}"#;
        let (items, multi, _) = import(text).unwrap();
        assert_eq!(multi, Some(false));
        assert!(items[0].on && items[0].id != "a");
        assert_eq!(items[1].include, [items[0].id.clone()]);
        assert!(import("[]").is_err() && import("{").is_err());
    }

    #[test]
    fn export_then_import_keeps_switches_and_applied_content() {
        let c = local("c", "3.3.3.3 c", false);
        let single = folder(
            "single",
            FolderMode::Single,
            true,
            vec![local("b", "2.2.2.2 b", true), c.clone()],
        );
        let off = folder("off", FolderMode::Default, false, vec![local("d", "4.4.4.4 d", true)]);
        let multi = folder("multi", FolderMode::Multi, false, vec![local("e", "5.5.5.5 e", false)]);
        let mut group = Item::new(ItemKind::Group, "g");
        group.include = vec![c.id.clone()];
        group.on = true;
        let original = store(
            vec![local("a", "1.1.1.1 a", true), single, off, multi, group],
            false,
        );

        let text = serde_json::to_string(&crate::switchhosts::export_v4(&original)).unwrap();
        let (items, multi, _) = import(&text).unwrap();
        let imported = store(items, multi.unwrap());
        assert!(!imported.multi_select);
        assert_eq!(imported.enabled_content(), original.enabled_content());
        assert!(imported.enabled_content().contains("4.4.4.4"));
        assert_eq!(on_titles(&imported), on_titles(&original));
        let modes: Vec<_> = imported.items[1..4].iter().map(|f| f.folder_mode).collect();
        assert_eq!(modes, [FolderMode::Single, FolderMode::Default, FolderMode::Multi]);
    }

    #[test]
    fn export_then_import_carries_profiles_with_remapped_switches() {
        let a = local("a", "1.1.1.1 a", true);
        let b = local("b", "2.2.2.2 b", false);
        let mut original = store(vec![a.clone(), b.clone()], false);
        let mut profile = crate::browser::BrowserProfile::new("Work");
        profile.switches = vec![a.id.clone()];
        profile.collapsed = vec![b.id.clone()];
        profile.tile_color = Some([1, 2, 3]);
        profile.custom_image = true;
        profile.revs.push(crate::browser::ProfileRev {
            id: new_id(),
            created_at: 0,
            kind: crate::browser::RevKind::Name,
            snapshot: profile.snapshot(),
        });
        original.profiles.push(profile);

        let text = serde_json::to_string(&crate::switchhosts::export_v4(&original)).unwrap();
        let (items, _multi, profiles) = import(&text).unwrap();
        let profiles = profiles.expect("this app's export carries profiles");
        assert_eq!(profiles.len(), 1);
        let imported = &profiles[0];
        assert_eq!(imported.name, "Work");
        assert_eq!(imported.tile_color, Some([1, 2, 3]));
        assert!(!imported.custom_image && imported.revs.is_empty());
        assert_ne!(imported.id, original.profiles[0].id);
        assert_eq!(imported.switches, vec![items[0].id.clone()]);
        assert_eq!(imported.collapsed, vec![items[1].id.clone()]);
        assert_ne!(imported.switches[0], a.id);
    }

    #[test]
    fn switchhosts_backup_carries_no_profiles() {
        let text = r#"{"version":[4],"data":{"list":{"tree":[{"id":"a","type":"local","title":"A"}]},"collection":{"hosts":{"data":[{"id":"a","content":"1.1.1.1 a"}]}}}}"#;
        let (_items, _multi, profiles) = import(text).unwrap();
        assert!(profiles.is_none());
    }

    #[test]
    fn fresh_store_has_no_items() {
        let path = std::env::temp_dir().join(format!("shl-missing-{}.json", new_id()));
        let store = load_json(&path, Store::with_defaults);
        assert!(store.items.is_empty() && store.trash.is_empty());
        assert!(store.multi_select);
        assert!(!path.exists());
    }

    fn contents(history: &History) -> Vec<&str> {
        history.entries.iter().map(|e| e.content.as_str()).collect()
    }

    #[test]
    fn history_records_nothing_for_unchanged_content() {
        let mut history = History::default();
        assert!(!history.record_change("a", "a"));
        assert!(history.entries.is_empty());
    }

    #[test]
    fn history_records_previous_then_next() {
        let mut history = History::default();
        assert!(history.record_change("a", "b"));
        assert_eq!(contents(&history), ["b", "a"]);

        assert!(history.record_change("b", "c"));
        assert_eq!(contents(&history), ["c", "b", "a"]);

        // Changed outside the app since the last write.
        assert!(history.record_change("x", "y"));
        assert_eq!(contents(&history), ["y", "x", "c", "b", "a"]);

        assert!(!history.record_change("y", "y"));
        assert!(history.record_change("x", "y"));
        assert_eq!(contents(&history), ["y", "x", "y", "x", "c", "b", "a"]);
    }

    #[test]
    fn history_keeps_the_newest_entries() {
        let mut history = History::default();
        for i in 0..HISTORY_LIMIT {
            history.record_change(&i.to_string(), &(i + 1).to_string());
        }
        assert_eq!(history.entries.len(), HISTORY_LIMIT);
        assert_eq!(history.entries[0].content, HISTORY_LIMIT.to_string());
        assert_eq!(history.entries[HISTORY_LIMIT - 1].content, "1");
    }
}
