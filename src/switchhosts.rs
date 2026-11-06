//! Compatibility with SwitchHosts backups.
//!
//! Import accepts the v3 (`{version:[3,..], list}`), v4
//! (`{version:[4,..], data:{list:{tree}, collection:{hosts:{data}}}}`) and v5
//! (`{format:"switchhosts-backup", manifest:{root}, entries}`) layouts,
//! following `src-tauri/src/import_export/preview.rs` of SwitchHosts.
//! Export writes the v4 layout, which SwitchHosts v4 loads with
//! `PotDb::loadJSON` and v5 still imports.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{Value, json};

use crate::i18n::LocalizedError;
use crate::store::{FolderMode, Item, ItemKind, Store, new_id};

const MAX_DEPTH: usize = 60;

/// Top-level key of exports written by this app. SwitchHosts ignores it; it
/// tells the import to keep the on/off state and the top-level choice mode.
const LITE_KEY: &str = "switchhosts_lite";

macro_rules! fail {
    ($key:literal $(, $name:ident = $arg:expr)?) => {
        return Err(LocalizedError::new($key)$(.arg(stringify!($name), $arg))?.into())
    };
}

/// A backup missing a required part, named by its JSON path.
fn incomplete(path: &str) -> anyhow::Error {
    LocalizedError::new("err_invalid_backup").detail(path).into()
}

pub fn is_backup(value: &Value) -> bool {
    value["format"] == "switchhosts-backup" || value["version"][0].is_u64()
}

/// Whether multiple top-level items may be on, when exported by this app.
pub fn multi_select(value: &Value) -> Option<bool> {
    value[LITE_KEY]["multi_select"].as_bool()
}

/// Convert a SwitchHosts backup into items with fresh ids. Like SwitchHosts
/// itself, everything (folders included) is imported switched off and system
/// nodes are skipped, except that exports of this app keep their on/off state.
pub fn parse(value: &Value) -> Result<Vec<Item>> {
    let mut contents = HashMap::new();
    let keep_on = value[LITE_KEY].is_object();
    let (nodes, version) = if value["format"] == "switchhosts-backup" {
        if value.get("schemaVersion").is_some_and(|v| v != 1)
            || value["manifest"]
                .get("schemaVersion")
                .is_some_and(|v| v != 1)
        {
            fail!("err_newer_backup");
        }
        let Some(root) = value["manifest"]["root"].as_array() else {
            return Err(incomplete("manifest.root"));
        };
        if let Some(entries) = value["entries"].as_object() {
            for (id, content) in entries {
                contents.insert(id.clone(), content.as_str().unwrap_or_default().to_string());
            }
        }
        (root.clone(), 5)
    } else {
        match value["version"][0].as_u64() {
            Some(3) => {
                let Some(list) = value["list"].as_array() else {
                    return Err(incomplete("list"));
                };
                (list.clone(), 3)
            }
            Some(4) => {
                let Some(tree) = value.pointer("/data/list/tree").and_then(Value::as_array) else {
                    return Err(incomplete("data.list.tree"));
                };
                match value
                    .pointer("/data/collection/hosts/data")
                    .and_then(Value::as_array)
                {
                    Some(rows) => {
                        for row in rows {
                            if let Some(id) = string_id(&row["id"]) {
                                let content = row["content"].as_str().unwrap_or_default();
                                contents.insert(id, content.to_string());
                            }
                        }
                    }
                    // A truncated backup must not silently become empty items.
                    None if has_content_nodes(tree) => {
                        return Err(incomplete("data.collection.hosts.data"));
                    }
                    None => {}
                }
                (tree.clone(), 4)
            }
            Some(n) if n > 4 => fail!("err_newer_backup"),
            _ => fail!("err_unknown_format"),
        }
    };

    let source = Source {
        version,
        contents: &contents,
        keep_on,
    };
    let mut ids = HashMap::new();
    let mut items = convert(&nodes, &source, &mut ids, 0)?;
    remap_includes(&mut items, &ids);
    if items.is_empty() {
        fail!("err_empty_backup");
    }
    Ok(items)
}

/// How the nodes of a backup are read.
struct Source<'a> {
    version: u8,
    contents: &'a HashMap<String, String>,
    keep_on: bool,
}

fn string_id(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn is_system(node: &Value, id: &str) -> bool {
    id == "0" || node["is_sys"] == true || node["isSys"] == true
}

fn has_content_nodes(nodes: &[Value]) -> bool {
    nodes.iter().any(|node| {
        let id = string_id(&node["id"]).unwrap_or_default();
        let kind = node["type"].as_str().unwrap_or("local");
        (matches!(kind, "local" | "remote") && !is_system(node, &id))
            || node["children"]
                .as_array()
                .is_some_and(|c| has_content_nodes(c))
    })
}

fn convert(
    nodes: &[Value],
    source: &Source,
    ids: &mut HashMap<String, String>,
    depth: usize,
) -> Result<Vec<Item>> {
    if depth > MAX_DEPTH {
        fail!("err_too_deep");
    }
    let version = source.version;
    let mut items = Vec::new();
    for node in nodes {
        let Some(old_id) = string_id(&node["id"]) else {
            fail!("err_invalid_id");
        };
        if is_system(node, &old_id) {
            continue;
        }
        let kind_field = if version == 3 {
            node.get("where").or(node.get("type"))
        } else {
            node.get("type")
        };
        let kind = match kind_field.and_then(Value::as_str).unwrap_or("local") {
            "local" | "remote" => ItemKind::Local,
            "folder" => ItemKind::Folder,
            "group" => ItemKind::Group,
            other => fail!("err_unknown_type", kind = other),
        };

        let id = new_id();
        if ids.insert(old_id.clone(), id.clone()).is_some() {
            fail!("err_duplicate_id", id = old_id);
        }

        let content = match kind {
            ItemKind::Local if version == 3 => node["content"].as_str().unwrap_or_default(),
            ItemKind::Local => source.contents.get(&old_id).map(String::as_str).unwrap_or_default(),
            _ => "",
        }
        .replace("\r\n", "\n");

        // v5 nests folder/group fields; older versions keep them flat.
        let folder_mode = node["folder"]["mode"]
            .as_u64()
            .or(node["folder_mode"].as_u64());
        let include = node["group"]["include"]
            .as_array()
            .or(node["include"].as_array())
            .map(|refs| refs.iter().filter_map(string_id).collect())
            .unwrap_or_default();
        let children = match (kind, node["children"].as_array()) {
            (ItemKind::Folder, Some(children)) => convert(children, source, ids, depth + 1)?,
            _ => Vec::new(),
        };

        items.push(Item {
            id,
            title: node["title"].as_str().unwrap_or_default().to_string(),
            kind,
            on: source.keep_on && node["on"] == true,
            content,
            children,
            folder_mode: match folder_mode {
                Some(1) => FolderMode::Single,
                Some(2) => FolderMode::Multi,
                _ => FolderMode::Default,
            },
            collapsed: node["is_collapsed"] == true || node["folder_open"] == false,
            include,
        });
    }
    Ok(items)
}

pub(crate) fn remap_includes(items: &mut [Item], ids: &HashMap<String, String>) {
    for item in items {
        item.include = item
            .include
            .iter()
            .filter_map(|old| ids.get(old).cloned())
            .collect();
        remap_includes(&mut item.children, ids);
    }
}

/// Serialize the store as a SwitchHosts v4 backup, with every switch as is.
pub fn export_v4(store: &Store) -> Value {
    let mut hosts = Vec::new();
    let tree: Vec<Value> = store
        .items
        .iter()
        .map(|item| node_v4(item, &mut hosts))
        .collect();
    let trashcan: Vec<Value> = store
        .trash
        .iter()
        .map(|trash| {
            json!({
                "data": node_v4(&trash.item, &mut hosts),
                "add_time_ms": trash.deleted_at * 1000,
                "parent_id": trash.parent_id,
            })
        })
        .collect();
    let mut backup = json!({
        "version": [4, 3, 0, 0],
        "data": {
            "list": { "tree": tree, "trashcan": trashcan },
            "collection": { "hosts": { "data": hosts } },
        },
    });
    backup[LITE_KEY] = json!({ "multi_select": store.multi_select });
    backup
}

fn node_v4(item: &Item, hosts: &mut Vec<Value>) -> Value {
    let mut node = json!({ "id": item.id, "title": item.title, "on": item.on });
    match item.kind {
        ItemKind::Local => {
            node["type"] = json!("local");
            // PotDb keys collection documents by `_id`.
            hosts.push(json!({ "_id": new_id(), "id": item.id, "content": item.content }));
        }
        ItemKind::Group => {
            node["type"] = json!("group");
            node["include"] = json!(item.include);
        }
        ItemKind::Folder => {
            node["type"] = json!("folder");
            node["folder_mode"] = json!(match item.folder_mode {
                FolderMode::Default => 0,
                FolderMode::Single => 1,
                FolderMode::Multi => 2,
            });
            node["folder_open"] = json!(!item.collapsed);
            node["children"] = item
                .children
                .iter()
                .map(|child| node_v4(child, hosts))
                .collect();
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.title.as_str()).collect()
    }

    #[test]
    fn imports_v4_with_numeric_ids_groups_and_system_node() {
        let backup = json!({"version":[4,3,0,6140],"data":{
            "list":{"tree":[
                {"id":"0","title":"System","is_sys":true},
                {"id":"a","title":"Dev","type":"local","on":true},
                {"id":7,"title":"Team","type":"folder","on":true,"folder_mode":1,"folder_open":false,"children":[
                    {"id":"r","title":"Remote","type":"remote","on":true,"url":"https://x"}]},
                {"id":"g","title":"Both","type":"group","include":["a","r","missing"]}
            ],"trashcan":[]},
            "collection":{"hosts":{"data":[
                {"_id":"x1","id":"a","content":"127.0.0.1 dev\r\n"},
                {"_id":"x2","id":"r","content":"1.1.1.1 remote"}]}}}});
        let items = parse(&backup).unwrap();
        assert_eq!(titles(&items), ["Dev", "Team", "Both"]);
        assert!(!items[0].on && !items[1].on && !items[1].children[0].on);
        assert_eq!(items[0].content, "127.0.0.1 dev\n");
        assert_eq!(items[1].folder_mode, FolderMode::Single);
        assert!(items[1].collapsed);
        assert_eq!(items[1].children[0].content, "1.1.1.1 remote");
        assert_eq!(items[2].include, [items[0].id.clone(), items[1].children[0].id.clone()]);
    }

    #[test]
    fn rejects_v4_without_contents_and_newer_versions() {
        let missing = json!({"version":[4],"data":{"list":{"tree":[{"id":"a","type":"local"}]}}});
        assert!(parse(&missing).is_err());
        let folders_only = json!({"version":[4],"data":{"list":{"tree":[{"id":"f","type":"folder"}]}}});
        assert!(parse(&folders_only).is_ok());
        assert!(parse(&json!({"version":[6]})).is_err());
    }

    #[test]
    fn imports_v3_and_v5() {
        let v3 = json!({"version":[3,3],"list":[
            {"id":0,"content":"sys"},
            {"id":12,"where":"folder","children":[{"id":13,"where":"local","on":true,"content":"x"}]}]});
        let items = parse(&v3).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children[0].content, "x");

        let v5 = json!({"format":"switchhosts-backup","schemaVersion":1,
            "manifest":{"root":[
                {"id":"f","type":"folder","title":"F","on":true,"folder":{"mode":1},"children":[
                    {"id":"l","type":"local","title":"L","on":true}]},
                {"id":"g","type":"group","title":"G","group":{"include":["l"]}},
                {"id":"d","type":"folder","title":"D","folder":{"mode":0}},
                {"id":"m","type":"folder","title":"M","folder_mode":2}]},
            "entries":{"l":"2.2.2.2 l"}});
        let items = parse(&v5).unwrap();
        assert_eq!(items[0].folder_mode, FolderMode::Single);
        assert!(!items[0].on && !items[0].children[0].on);
        assert_eq!(items[0].children[0].content, "2.2.2.2 l");
        assert_eq!(items[1].include, [items[0].children[0].id.clone()]);
        assert_eq!(items[2].folder_mode, FolderMode::Default);
        assert_eq!(items[3].folder_mode, FolderMode::Multi);
    }

    #[test]
    fn v4_export_writes_switches_as_is_and_round_trips() {
        let mut local = Item::new(ItemKind::Local, "L");
        local.content = "1.1.1.1 a".into();
        local.on = true;
        let mut folder = Item::new(ItemKind::Folder, "F");
        folder.folder_mode = FolderMode::Single;
        folder.children.push(local.clone());
        let mut group = Item::new(ItemKind::Group, "G");
        group.include = vec![local.id.clone()];
        group.on = true;
        let store = Store {
            items: vec![folder, group],
            trash: Vec::new(),
            multi_select: true,
        };

        let exported = export_v4(&store);
        let tree = &exported["data"]["list"]["tree"];
        assert_eq!(tree[0]["on"], false);
        assert_eq!(tree[0]["folder_mode"], 1);
        assert_eq!(tree[0]["children"][0]["on"], true);
        assert_eq!(tree[1]["on"], true);
        assert!(exported["data"]["collection"]["hosts"]["data"][0]["_id"].is_string());
        assert_eq!(multi_select(&exported), Some(true));

        let items = parse(&exported).unwrap();
        assert!(!items[0].on && items[0].children[0].on && items[1].on);
        assert_eq!(items[0].children[0].content, "1.1.1.1 a");
        assert_eq!(items[1].include, [items[0].children[0].id.clone()]);

        let mut foreign = exported;
        foreign.as_object_mut().unwrap().remove(LITE_KEY);
        let items = parse(&foreign).unwrap();
        assert!(!items[0].children[0].on && !items[1].on);
    }
}
