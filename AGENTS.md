# AGENTS.md

Guidance for coding agents working on **SwitchHosts Lite**, a native hosts switcher modeled on [SwitchHosts](https://github.com/oldj/SwitchHosts). It is a single Rust binary built with gpui and gpui-component, which come in through `gpui-kit` 0.9.0 (`use gpui_kit::*`, `gpui_kit::component::*`, `gpui_kit::assets::*`). User-facing docs live in `README.md` (Chinese, the primary version) and `README.en.md`; keep both in sync when behavior changes.

## Commands

```sh
cargo run                                    # debug build
cargo test                                   # all unit tests (CI: cargo test --locked)
cargo clippy --all-targets -- -D warnings    # must be clean (CI: --locked)
cargo build --release
scripts/bundle-macos.sh [--target <triple>] [--out <dir>]   # dist/SwitchHosts Lite.app
python3 scripts/release-manifest.py --dir DIR --version X.Y.Z --repo owner/name [--tag vX.Y.Z] [--notes-file F] [--base-url URL]
```

- The toolchain is pinned in `rust-toolchain.toml` (1.99.1), and the crate uses edition 2024.
- The cargo target directory may be overridden by the environment (for example a sandbox cache). Don't hard-code `target/`; ask `cargo metadata --format-version 1 --no-deps` for `target_directory`, as `scripts/bundle-macos.sh` does.
- CI (`.github/workflows/ci.yml`) runs test and clippy on macOS and Windows. `.github/workflows/release.yml` runs on `v*` tags: it checks that the tag equals `v` + the `Cargo.toml` version, builds 4 targets, and publishes macOS zip/dmg (lzma, `ULMO`) archives and Windows `-setup.exe` installers (`scripts/windows/installer.nsi`, the Windows updater asset), `SHA256SUMS.txt` and `latest.json`. Tags containing `-` become pre-releases, which the updater never sees.
- Manual updater test, macOS only and ignored by default: serve a release directory locally (`release-manifest.py --base-url http://127.0.0.1:8765`), then run
  `SHL_TEST_MANIFEST_URL=http://127.0.0.1:8765/latest.json SHL_TEST_BUNDLE=/tmp/x/App.app cargo test simulate_update -- --ignored`.
- Debug builds only: `SHL_UPDATE_MANIFEST_URL=<url> cargo run` points the updater at a local manifest.

### Type-checking Windows code from macOS

Windows-only code (`#[cfg(windows)]` in `hosts.rs` and `updater.rs`) is not compiled on a Mac. To type-check it:

```sh
rustup target add x86_64-pc-windows-msvc
cat > /tmp/fake-rc <<'EOF'
#!/bin/sh
for a in "$@"; do
  if [ "$a" = "/?" ]; then echo "OVERVIEW: LLVM Resource Converter"; exit 0; fi
done
prev=""
for a in "$@"; do
  case "$prev" in /fo|-fo|/FO|-FO) out="$a";; esac
  case "$a" in /fo?*|-fo?*) out="${a#?fo}";; esac
  prev="$a"
done
[ -n "$out" ] && touch "$out"
exit 0
EOF
chmod +x /tmp/fake-rc
RC=/tmp/fake-rc CC_x86_64_pc_windows_msvc=clang cargo clippy --target x86_64-pc-windows-msvc --all-targets
```

The stub answers the resource-compiler probe and creates an empty output file; `clang` stands in for `cl.exe`. This only checks the code; it does not produce a working binary.

## Architecture

`src/main.rs` registers the modules, installs `AppAssets` (the default gpui-kit icon bundle plus the extra Lucide icons declared with `icon_assets!(ExtraIcons, [...])`; add an icon there if `IconName::X` renders blank), and then runs `themes::load` → `app::init` → `app::bind_keys` → opens the window → `updater::init`. On Windows it first calls `hosts::run_as_elevated_helper` and `updater::run_update_installer_helper`, which only return for an ordinary launch. The app quits when the last window closes (`Quit` is bound to cmd-q, plus ctrl-q off macOS).

| Module | Responsibility |
| --- | --- |
| `store.rs` | Data model and persistence (`data.json`, `history.json`), import/export entry points |
| `switchhosts.rs` | SwitchHosts backup parsing (v3/v4/v5) and v4 export |
| `hosts.rs` | System hosts path, managed block, line endings, privileged writes |
| `app.rs` + `app/*.rs` | The single `SwitchHostsApp` view and its actions |
| `editor_ops.rs` | Pure line-editing commands |
| `highlight.rs` | Incremental hosts-syntax highlighter |
| `i18n.rs` | `Lang` (UI language), `LocalizedError`; texts live in `locales/app.yml` (rust-i18n) |
| `settings.rs` | `settings.json` preferences as a gpui `Global` |
| `themes.rs` | Bundled + user color themes, theme selection |
| `updater.rs` | Update check, download, verify, install, relaunch |
| `macos.rs` | Dock icon for unbundled debug runs (`cargo run`); release builds rely on the bundle's `AppIcon.icns` |

### store.rs

- `Item { id, title, kind: ItemKind, on, content, children, folder_mode: FolderMode, collapsed, include }` and `ItemKind::{Local, Folder, Group}`. `content` is used only by `Local`, `children` only by `Folder`, and `include` (ids of local items) only by `Group`.
- **Switch semantics follow SwitchHosts** (`setOnStateOfItem` in `src/common/hostsFn.ts`, with `multi_chose_folder_switch_all` always on). An item is effective when its own `on` is true, or when a group that is on includes it (`is_effective`); a folder's `on` never gates its subtree. `set_on` passes a folder's switch down to its descendants, makes each ancestor folder follow (off when a child turns off, on once all children are on), then, under single choice, switches the siblings off together with their subtrees. Folders in `FolderMode::Single` neither pass the switch down nor follow their children. `Item::new` creates everything switched off.
- `FolderMode::{Default, Single, Multi}` mirror SwitchHosts' `folder_mode` 0/1/2. `single_choice(folder)` resolves `Default` to `!Store::multi_select`, which is also the top-level choice mode. Changing either setting never touches existing switches.
- `enabled_content()` concatenates `# --- <title> ---` sections of the effective items in tree order. Each local item appears at most once, and CRLF is normalized. `content_of(item, whole_folder)` builds the read-only previews.
- `move_item(id, target, DropPosition::{Before, After, Inside})` follows SwitchHosts' tree drag (`treeMoveNode` in `src/renderer/components/Tree/fn.ts`): any item kind can move before/after any item or to the end of a folder (`Inside`, which also expands it). `can_drop` rejects unknown ids, the item's own subtree and `Inside` on non-folders. Switches are never touched. `DropPosition::at(y, height, folder)` is upstream's pointer zones: halves for items; quarter / middle / quarter for folders. `move_to` (the "Move to" menu) appends to a folder or the top level.
- `delete` moves an item to `trash` (`TrashItem { item, parent_id, deleted_at }`). `restore` reinserts it under its old parent with its whole subtree switched off. `replace_items` (used by import) moves all current items to the trash.
- `write_json` writes `*.json.tmp` and then renames it. `load_json` renames an unparsable file to `*.json.bak` and falls back to defaults; a fresh store has no items (like SwitchHosts, the list starts with only the System row). `History` keeps at most 50 entries. `record_change(previous, next)` follows SwitchHosts (`record_change` in `src-tauri/src/hosts_apply/history.rs`): nothing when they are equal, else the previous content and then the new one, each skipped when it equals the newest entry. Nothing is recorded at startup.
- `import()` strips a BOM, then tries a SwitchHosts backup first and the legacy `{"app":"SwitchHostsLite"}` format second. All imported ids are regenerated, and `include` is remapped to the new ids.
- `data_dir()` is `dirs::config_dir()/SwitchHostsLite`.

### switchhosts.rs

- `parse` follows SwitchHosts' own importer. System nodes are skipped. `remote` becomes `Local` with the cached content. Like SwitchHosts v5, everything (folders included) is imported switched off. Only this app's exports (top-level `switchhosts_lite` key) keep `on` and `multi_select`. `folder_mode` (v5: `folder.mode`) maps 1 → `Single`, 2 → `Multi`, anything else → `Default`.
- The importer rejects newer schemas, duplicate or invalid ids, nesting deeper than 60 levels, and a v4 backup whose content collection is missing.
- `export_v4` writes every `on` and `folder_mode` as is, since the switch semantics match SwitchHosts. Round-trip tests assert that the switches and `enabled_content()` are preserved; keep them passing.

### hosts.rs

- Markers: `# --- SWITCHHOSTS_LITE_CONTENT_START ---` / `..._END ---`. **Never translate or change them**: existing users' hosts files contain them. `build(system, enabled)` strips the old block (an unterminated start marker keeps the lines after it) and appends the new block. Content outside the block is preserved.
- Internally all text is LF without a BOM (`normalize`). `to_native` writes CRLF on Windows. `read_current` refuses non-UTF-8 hosts files instead of rewriting them lossily.
- `write_raw` tries a direct write first. On `PermissionDenied` it falls back to `write_elevated`:
  - macOS: `osascript … with administrator privileges`, then a DNS cache flush.
  - Windows: `ShellExecuteExW` with the `runas` verb starts this same executable with `--elevated-write-hosts <staged file>`, which shows the UAC prompt (`ERROR_CANCELLED` means it was refused). The child handles that argument at the top of `main` via `hosts::run_as_elevated_helper`, writes the staged file over the hosts, flushes DNS and exits, so no UI starts. The result is then read back and verified. Sharing violations are reported as "locked by antivirus". No PowerShell is involved: `-EncodedCommand` plus a hidden elevated `Start-Process` is what Defender's ML heuristics flag as a hosts hijacker. The launcher itself is `run_elevated_process`, which the updater reuses for the installer.
- `can_write_directly()` (open for append) gates every automatic write.
- On Windows, `hosts_path()` comes from the registry `Tcpip\Parameters\DataBasePath` (with env expansion) if `<dir>\hosts` exists, else `GetSystemWindowsDirectoryW()\System32\drivers\etc\hosts`. It is resolved once. The pure helpers (`select_hosts_path`, `join_windows`, `elevated_args`) are `cfg(any(windows, test))` so they are tested on every OS.

### app.rs and app/

- `SwitchHostsApp` owns the `Store`, the `Selection` (`System | Item(id) | Trash(Option<id>)`), a single main `EditorState`, and UI flags.
- Panels live in submodules: `icon_sidebar.rs`, `list_panel.rs` (tree, drag and drop, context menus, trash), `main_panel.rs` (title bar and editor), `right_panel.rs` (info), `edit_sheet.rs` (add/edit `EditDraft`), `history_sheet.rs`, `preferences.rs`, and `backup.rs` (import/export).
- **Two apply paths**:
  - `toggle(id, on)` snapshots the store, calls `set_on`, and always writes, prompting for privileges if needed. If the write fails *and* `revision` hasn't changed in the meantime, it rolls back to the snapshot.
  - `mutate(f)` is used for structural edits. It writes only when `enabled_content()` changed **and** `hosts::can_write_directly()`, and never prompts.
- Typing in a local item triggers `persist_later` (a 500 ms debounced save, guarded by `save_generation`) and `schedule_auto_apply` (1 s, guarded by `edit_generation`, and skipped unless the item is effective, unapplied, and writable without a prompt). `flush_save` runs before every apply and on app quit (`on_app_quit`).
- `revision` is bumped on every store change. `applied: Cell<Option<bool>>` and `rules: Cell<Option<usize>>` cache values that render reads every frame. Call `invalidate()` / `.set(None)` whenever the store or the system hosts change.
- The system hosts is re-read when the window is activated, but only if its `(mtime, len)` `Stamp` changed. After every write, `after_write` force-reloads it. Both write paths (`apply`, whose `hosts::apply` returns the content before and after, and the history sheet's restore, which reads the hosts before `write_raw`) call `record_history(previous, next)` on success.
- `refresh_editor` uses `set_value` when the selection changes. `refresh_preview` updates read-only previews after store changes.
- History and its editor exist only while the history sheet is open.
- **List drag and drop** (`list_panel.rs`): item rows have `on_drag(DraggedItem)` (the constructor also resets `SwitchHostsApp::drag: Option<DragState>`) and `on_drag_move`, which sets `DragState::target` from the pointer's row and `DropPosition::at`, filtered by `Store::can_drop`. The System row and the empty area clear the target; the list itself tracks whether the pointer is inside it and owns the single `on_drop`, which applies the move through `mutate`. Rendering trusts `drag` only while `cx.has_active_drag()`. A 1 s uninterrupted hover over a collapsed folder with children expands it (`hover_generation`). Trash rows and the System row are not draggable.

### editor_ops.rs

These are pure functions `fn(&str, Range<usize>) -> Option<Edit>` on UTF-8 byte offsets: `toggle_comment`, `move_line_up`, `move_line_down`, `duplicate_line`, `delete_line`. They are well covered by tests; add a test with every change. `SwitchHostsApp::run_editor_op` converts the result to UTF-16 and applies it.

### highlight.rs

`HostsHighlighter` implements `InputHighlighter` and produces `Comment`, `Ip` (first field parses as `IpAddr`) and `Invalid` tokens. Tokens never span lines, so `apply_edit` only retokenizes the touched lines and shifts the rest; a test checks that the result equals a full retokenize. Colors come from the theme's `comment`/`number` highlight styles. Invalid lines use the theme's `danger` color, which `preferences::apply_theme` pushes in through `set_theme_colors`. The hard-coded fallbacks are used only when the syntax theme lacks a style.

### i18n.rs

- Texts live in `locales/app.yml` (rust-i18n's `_version: 2` single-file format): one flat snake_case key per message, each with `zh-CN`, `zh-TW` and `en`. Error keys start with `err_`. `rust_i18n::i18n!("locales", fallback = "en")` in `main.rs` embeds them at compile time; `build.rs` reruns on changes under `locales/`, because the macro doesn't tell cargo.
- Look texts up with `use rust_i18n::t;` and `t!("key")`, or `t!("key", name = value)` for `%{name}` placeholders. It returns `Cow<'static, str>`; Radio/Checkbox labels need `SharedString::from(t!(..))`. Always write the key as a literal (in `match` arms if it depends on a value) so the tests can see it.
- The UI language is rust-i18n's **global locale**, which gpui-component reads too (same `zh-CN`/`zh-TW`/`en` codes). `Lang::apply` sets it (from `preferences::apply_language`, then `cx.refresh_windows()`), and `Lang::current` reads it back. `rust-i18n` must stay at the version gpui-component uses (`cargo tree -i rust-i18n` shows one copy), or the two would have separate locales.
- `LocalizedError::new("err_..").arg("name", value).detail(..)` carries errors to the UI. Use `.localize("err_..")` on any `Result`, and render with `error_text(&err)` or `err.message()`. Its `Display` is English (`message_in(Lang::En)`).
- Tests replace the old compile-time checks: every key has all three languages, non-empty; the `%{name}` placeholders match across languages and no `{0}`-style ones remain; every `t!("..")` key and every `"err_.."` literal under `src/` exists; and no key in the file is unused. They read the embedded table through `crate::_rust_i18n_backend()`, and scan source with `#[cfg(test)] mod tests` stripped, so these checks cover every platform's code.
- `Lang::from_locale` maps `zh-Hant/TW/HK/MO` to zh-TW, other `zh*` to zh-CN, and everything else to English.
- Wording follows SwitchHosts' translations where the concept matches.

### settings.rs

`Settings` is a gpui `Global` persisted to `settings.json` with `#[serde(default)]`. Fields at their default values are skipped when written, and unknown fields are ignored. It holds the language, `ThemePref` (System/Light/Dark), the light/dark theme names (`None` means the default), `check_updates` and `last_update_check`. Change it only through `Settings::update(cx, |s| ..)`, which saves and re-sets the global.

### themes.rs and app/preferences.rs

- `themes::load` registers user themes from `<data dir>/themes/*.json` (sorted by file name) **before** the 21 bundled files in `assets/themes/` (11 light + 25 dark themes; a test pins those counts). The registry keeps the first theme of each name, so user themes override bundled ones. Themes load once, at startup.
- `preferences::apply_theme(appearance, cx)` picks the light and dark configs with `themes::pick`, which falls back to the default when the saved name is missing or has the wrong mode. When anything changed, it calls `reset_theme_file_fields` and then `Theme::change`. Afterwards it **always** re-applies `theme.sheet.margin_top = TOP_BAR_HEIGHT` and the caret idle timeout, and pushes the highlight colors.
- `apply_theme` runs at init, when the window appearance changes (System mode), and whenever a theme preference changes.

### updater.rs

- **Source**: `SHL_UPDATE_REPO` at build time, else `CARGO_PKG_REPOSITORY`. The `OWNER` placeholder means "not configured": the menu item is disabled and no checks run. The manifest is `releases/latest/download/latest.json` (`{version, notes, assets: {"<os>-<arch>": {url, sha256, size}}}`), and the platform key is `std::env::consts::{OS, ARCH}`.
- **Flow**: `init` waits 5 s, removes `.old`/`.new` leftovers and stale `switch-hosts-lite-update-*` directories in the temp dir, and then checks if `check_updates` is on and the last check was at least 24 h ago. A manual check reports every outcome; an automatic one only reports a found update.
- **Download**: `download` streams the archive, hashes it with SHA-256, and verifies the size and hash. The HTTP backend is the `mod http` inside `updater.rs`, which uses the system `curl`.
- **Install**:
  - macOS: `ditto -x -k`, check that the new `Info.plist` version matches, strip quarantine, then swap the `.app` bundle (asking for an admin password via osascript if the parent directory isn't writable). Refused for non-bundle (dev) runs and for translocated apps.
  - Windows: a `-setup.exe` asset is handed to a detached copy of this exe (`--install-update <setup> <exe> <pid>`), because the installed image cannot be overwritten while it runs and the helper must not be that image. The copy waits for the app's process to exit, runs the installer with `/S` elevated through `hosts::run_elevated_process` (UAC; a cancelled elevation keeps the old version), deletes the installer, and starts the app again unelevated. The work directory is left to `remove_stale_update_dirs` on a later startup. A `.zip` asset (no longer published; older Windows builds only understand this path) is extracted with `tar.exe`, else PowerShell, then `replace_binary` renames the running exe to `.old`; that path requires the exe's directory to be writable.
- **Restart**: spawn the relaunch command, then `dispatch_action(&Quit)` so the normal quit path flushes pending saves.

## Conventions

- **Warnings-free.** `cargo clippy --all-targets -- -D warnings` must pass. Don't add `#[allow(dead_code)]`; the only ones in the tree are `cfg_attr`-gated for platform-specific items (`updater.rs`). Use the same `cfg_attr` pattern if you add another one.
- **Tests for pure logic.** Store semantics, import/export, hosts building, editor ops, highlighting, version parsing, settings parsing and theme picking all have unit tests next to the code. Extend them when you change behavior. UI code is not unit-tested.
- **All user-visible strings go through `t!` with a key in `locales/app.yml`** with all three languages. Don't build user-facing English in code. Errors shown to users should be `LocalizedError`. Hosts markers, file names and log messages (`eprintln!`) are not translated.
- **Colors come only from `cx.theme()` tokens** (`foreground`, `muted_foreground`, `border`, `primary`, `danger`, `warning`, `list_active`, `sidebar`, …). Never hard-code colors in UI code. The only literal colors are the highlighter fallbacks.
- **Platform code stays behind `cfg`** (`#[cfg(windows)]`, `#[cfg(target_os = "macos")]`, `#[cfg(all(unix, not(target_os = "macos")))]`). Logic that can be pure should be `cfg(any(<platform>, test))` so it runs in tests everywhere.
- **Comments state only non-obvious constraints** (why a value is what it is, platform quirks). Don't write comments that narrate the code or the change. Doc comments are short and descriptive.
- Match the existing style: small helpers, `let-else`, let-chains, `anyhow::Result` with `.localize("err_..")` at the boundaries.
- **Don't add dependencies lightly.** Binary size matters. The release profile uses `lto = "fat"`, `codegen-units = 1`, `strip`, `panic = "abort"`, and `opt-level = "s"` for dependencies except the per-frame ones (gpui, gpui-base, gpui-component, taffy, ropey, rustybuzz), which stay at 3; add a crate there if it turns out to be hot. Don't use UPX: it breaks macOS code signing and trips Windows antivirus.
- Keep `README.md` and `README.en.md` in sync, and update this file when architecture or conventions change.

## gpui / gpui-component gotchas (learned here)

- **Controls inside the `TitleBar` on Windows.** The bar is one big drag area (`WindowControlArea::Drag`, hit-tested as `HTCAPTION`), so the system swallows clicks on its children and turns them into window drags. Wrap every control in a `div().occlude()` (see `no_drag` in `main_panel.rs`); `occlude` gives the wrapper a blocking hitbox, so the drag area is no longer under the pointer. Also stop the left mouse down there, or a one-pixel hand movement starts a window move and the click is lost.
- **Sheets vs. the title bar.** `Sheet` starts at `theme.sheet.margin_top`, whose default is the library's `TITLE_BAR_HEIGHT`, shorter than our 44 px `TOP_BAR_HEIGHT`. `apply_theme` overrides it after every `Theme::change`, because theme changes reset it. Any new code path that changes the theme must keep doing this.
- **Editor-only key bindings** use the context `"Input && mode == editor"` (`EDITOR_CONTEXT` in `app.rs`) so single-line inputs don't get them. Handlers call `cx.propagate()` when the editor is read-only or unfocused.
- **Undoable programmatic edits.** Use `EntityInputHandler::replace_text_in_range(Some(utf16_range), text, ..)`, which takes **UTF-16** ranges; convert from byte offsets as `run_editor_op` does. Then call `set_selected_range_with_direction`.
- **`EditorState::set_value` does not emit `InputEvent::Change`**, and it clears undo history and the selection. That is what we want when switching items, but it means programmatic `set_value` calls never reach the change subscription (no save, no auto-apply).
- **`Switch` inside a clickable row**: call `cx.stop_propagation()` in `on_change`, or the row click also selects or opens the item. The same applies to the chevron and edit buttons.
- **Drag and drop.** A drag starts after 2 px of movement with the button held, and then the row's click doesn't fire, so a plain click still selects. Pressing a child control also arms its row's drag: wrap controls (switch, edit button, folder chevron) in a div whose left `on_mouse_down` calls `cx.stop_propagation()` (`no_drag`). Children's mouse listeners run before their parent's in the bubble phase, so the control still gets the press; putting the stop on the control's own div could cancel its click instead. `on_drag_move` fires on **every** element that has one, wherever the pointer is, so check `event.bounds.contains(&event.event.position)`. Those bounds aren't clipped by a scroll container, which is why the list checks its own bounds first (capture listeners run parent first). `on_drop` only fires on the hovered element, and a drag released anywhere else just ends, with no callback. Hover and group-hover styles are off while a drag is active.
- **Caret blink** repaints the window twice a second even in the background. `apply_theme` sets `CursorBlink::default().with_idle_timeout(10s)`.
- **Don't use `ThemeRegistry::watch_dir`.** Its reload clears the registry down to the default themes plus that directory, wiping the bundled themes we added with `load_themes_from_str`.
- **Theme files only override the fields they name.** Before switching, `reset_theme_file_fields` restores font, radius, shadow and highlight theme to their defaults so that nothing leaks from the previous theme.
- **Missing icons.** gpui-kit's default icon bundle lacks some Lucide icons. Register them in `icon_assets!(ExtraIcons, [...])` in `main.rs`.
- **Focus.** Without a focused element, key bindings dispatch from the window root and never reach the view's actions. `render` focuses the app's `focus_handle` when nothing is focused.
- **System appearance at startup.** `SwitchHostsApp::new` re-applies the theme with `window.appearance()` when the preference is System, since the window knows the current appearance.

## Safety

- **Never write the real system hosts from tests or scripts.** `hosts::build`, `normalize` and the other helpers are pure; test those. `apply`, `write_raw` and `write_elevated` touch `/etc/hosts` or the Windows hosts file.
- Tests that need files use `std::env::temp_dir()` with a unique name and clean up after themselves. Don't write into `data_dir()` from tests.
- For manual experiments with app data, point the data directory at a temp location so the real `data.json` and history aren't touched. On macOS, build first and run the binary with a temp `HOME`, e.g. `HOME=$(mktemp -d) "<target_dir>/debug/switch-hosts-lite"`; running `cargo` itself with a fake `HOME` breaks rustup. Applying hosts still writes the real system file.
- Don't change the data file formats incompatibly. `data.json`, `settings.json` and `history.json` are read with `serde(default)`, so add new fields with defaults.
