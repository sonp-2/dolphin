//! 部品の値（マップ）から `Node` の木を作る

use std::collections::HashMap;

use super::*;

/// 部品の値（マップ）を `Node` にする。コールバックは番号をつけて `callbacks` に入れる
pub(crate) fn to_node(
    value: &Value,
    callbacks: &mut HashMap<u64, Value>,
    next_id: &mut u64,
) -> Result<Node, String> {
    let mut register = |callback: &Value| {
        let id = *next_id;
        *next_id += 1;
        callbacks.insert(id, callback.clone());
        id
    };
    match value {
        Value::Nil => Ok(Node::Column {
            children: Vec::new(),
            spacing: None,
        }),
        Value::Array(items) => {
            let children = nodes(&items.borrow(), callbacks, next_id)?;
            Ok(Node::Column {
                children,
                spacing: None,
            })
        }
        Value::Map(map) => {
            let map = map.borrow();
            let kind = match map.get("type") {
                Some(Value::Str(kind)) => kind.to_string(),
                _ => return Err("部品ではないマップです（`std:ui` の関数で作ります）".into()),
            };
            let str_of = |key: &str| match map.get(key) {
                Some(Value::Str(s)) => s.to_string(),
                _ => String::new(),
            };
            let num_of = |key: &str| match map.get(key) {
                Some(Value::Num(n)) => *n,
                _ => 0.0,
            };
            let bool_of = |key: &str| matches!(map.get(key), Some(Value::Bool(true)));
            let callback = |key: &str| map.get(key).cloned().unwrap_or(Value::Nil);
            let children = |key: &str| match map.get(key) {
                Some(Value::Array(items)) => Some(items.borrow().clone()),
                _ => None,
            };
            let spacing = || match map.get("spacing") {
                Some(Value::Num(n)) => Some(*n as f32),
                _ => None,
            };
            Ok(match kind.as_str() {
                "text" => Node::Text {
                    text: str_of("text"),
                    size: match map.get("size") {
                        Some(Value::Num(n)) => Some(*n as f32),
                        _ => None,
                    },
                    color: match map.get("color") {
                        Some(Value::Str(s)) => parse_color(s),
                        _ => None,
                    },
                    bold: bool_of("bold"),
                },
                "button" => Node::Button {
                    label: str_of("label"),
                    id: register(&callback("on_click")),
                    enabled: !matches!(map.get("enabled"), Some(Value::Bool(false))),
                },
                "text_input" => Node::TextInput {
                    value: str_of("value"),
                    id: register(&callback("on_change")),
                    hint: str_of("hint"),
                    multiline: bool_of("multiline"),
                    password: bool_of("password"),
                },
                "checkbox" => Node::Checkbox {
                    label: str_of("label"),
                    checked: bool_of("checked"),
                    id: register(&callback("on_change")),
                },
                "slider" => Node::Slider {
                    value: num_of("value"),
                    min: num_of("min"),
                    max: num_of("max"),
                    id: register(&callback("on_change")),
                },
                "select" => {
                    let labels = children("labels")
                        .unwrap_or_default()
                        .iter()
                        .map(|v| v.to_string())
                        .collect();
                    Node::Select {
                        labels,
                        index: num_of("index") as usize,
                        id: register(&callback("on_change")),
                    }
                }
                "progress" => Node::Progress(num_of("fraction") as f32),
                "column" | "row" | "group" => {
                    let items = children("children").unwrap_or_default();
                    let nodes = nodes(&items, callbacks, next_id)?;
                    match kind.as_str() {
                        "column" => Node::Column {
                            children: nodes,
                            spacing: spacing(),
                        },
                        "row" => Node::Row {
                            children: nodes,
                            spacing: spacing(),
                        },
                        _ => Node::Group(nodes),
                    }
                }
                "scroll" => {
                    let child = map.get("child").cloned().unwrap_or(Value::Nil);
                    Node::Scroll(Box::new(to_node(&child, callbacks, next_id)?))
                }
                "separator" => Node::Separator,
                "space" => Node::Space(num_of("size") as f32),
                other => return Err(format!("知らない部品 `{other}` です")),
            })
        }
        other => Err(format!(
            "部品は `std:ui` の関数が返す値です（{}でした）",
            other.type_name()
        )),
    }
}

/// 子の部品の並び。`nil` は、その場所に何も置かない
fn nodes(
    items: &[Value],
    callbacks: &mut HashMap<u64, Value>,
    next_id: &mut u64,
) -> Result<Vec<Node>, String> {
    items
        .iter()
        .filter(|item| !matches!(item, Value::Nil))
        .map(|item| to_node(item, callbacks, next_id))
        .collect()
}

/// `#rrggbb`
pub(super) fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}
