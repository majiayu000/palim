use crate::{Error, delta, pointer, text};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(crate) fn apply(left: &Value, change: &Value, strict: bool) -> Result<Value, Error> {
    delta::check_depth(left, 128)?;
    apply_checked(left.clone(), change, strict, None)
}
pub(crate) fn apply_owned(left: Value, change: &Value, strict: bool) -> Result<Value, Error> {
    delta::check_depth(&left, 128)?;
    apply_checked(left, change, strict, None)
}
pub(crate) fn apply_fuzzy(
    left: &Value,
    change: &Value,
    options: &crate::TextPatchOptions,
) -> Result<Value, Error> {
    delta::check_depth(left, 128)?;
    apply_checked(left.clone(), change, true, Some(options))
}
fn apply_checked(
    left: Value,
    change: &Value,
    strict: bool,
    fuzzy: Option<&crate::TextPatchOptions>,
) -> Result<Value, Error> {
    apply_node_owned(Some(left), change, "", strict, fuzzy)?.ok_or_else(|| {
        Error::new(
            "",
            "root deletion has no JSON value; use a containing object",
        )
    })
}
fn apply_node(
    left: Option<&Value>,
    change: &Value,
    path: &str,
    strict: bool,
) -> Result<Option<Value>, Error> {
    apply_node_owned(left.cloned(), change, path, strict, None)
}
fn apply_node_owned(
    left: Option<Value>,
    change: &Value,
    path: &str,
    strict: bool,
    fuzzy: Option<&crate::TextPatchOptions>,
) -> Result<Option<Value>, Error> {
    match change {
        Value::Array(a) => match a.as_slice() {
            [new] => {
                if left.is_some() && !path.is_empty() {
                    return Err(Error::new(path, "addition targets an existing value"));
                }
                Ok(Some(new.clone()))
            }
            [old, new] => {
                let current =
                    left.ok_or_else(|| Error::new(path, "replacement targets a missing value"))?;
                if strict && current != *old {
                    return Err(Error::new(path, "replacement old value does not match"));
                }
                Ok(Some(new.clone()))
            }
            [old, zero, marker] if zero == &json!(0) && marker == &json!(0) => {
                let current =
                    left.ok_or_else(|| Error::new(path, "deletion targets a missing value"))?;
                if strict && current != *old {
                    return Err(Error::new(path, "deletion old value does not match"));
                }
                Ok(None)
            }
            [Value::String(patch), _, marker] if marker == &json!(2) => {
                let Some(Value::String(current)) = left else {
                    return Err(Error::new(path, "text patch requires a string"));
                };
                Ok(Some(Value::String(match fuzzy {
                    Some(options) => text::apply_fuzzy(&current, patch, path, options)?,
                    None => text::apply(&current, patch, path)?,
                })))
            }
            _ => Err(Error::new(path, "invalid leaf delta")),
        },
        Value::Object(map) if delta::is_array(map) => {
            let Some(Value::Array(source)) = left else {
                return Err(Error::new(path, "array delta requires an array"));
            };
            array(source, map, path, strict, fuzzy).map(|v| Some(Value::Array(v)))
        }
        Value::Object(map) => {
            let Some(Value::Object(mut result)) = left else {
                return Err(Error::new(path, "object delta requires an object"));
            };
            for (key, child) in map {
                let current = result.remove(key);
                if let Some(value) =
                    apply_node_owned(current, child, &pointer(path, key), strict, fuzzy)?
                {
                    result.insert(key.clone(), value);
                }
            }
            Ok(Some(Value::Object(result)))
        }
        _ => Err(Error::new(path, "invalid delta")),
    }
}
fn array(
    source: Vec<Value>,
    change: &Map<String, Value>,
    path: &str,
    strict: bool,
    fuzzy: Option<&crate::TextPatchOptions>,
) -> Result<Vec<Value>, Error> {
    let parts = delta::parts(change, path)?;
    for (&index, change) in &parts.removals {
        let current = source.get(index).ok_or_else(|| {
            Error::new(&pointer(path, index), "original array index out of bounds")
        })?;
        if strict && (change[2] == json!(0) || change[0] != json!("")) && *current != change[0] {
            return Err(Error::new(
                &pointer(path, index),
                "array operation old value does not match",
            ));
        }
    }
    let len = source
        .len()
        .checked_sub(parts.removals.len())
        .and_then(|n| n.checked_add(parts.moves.len()))
        .and_then(|n| n.checked_add(parts.additions.len()))
        .ok_or_else(|| Error::new(path, "array length overflow"))?;
    for &index in parts
        .additions
        .keys()
        .chain(parts.moves.keys())
        .chain(parts.changes.keys())
    {
        if index >= len {
            return Err(Error::new(
                &pointer(path, index),
                "target array index out of bounds",
            ));
        }
    }
    let mut slots: Vec<Option<Value>> = source.into_iter().map(Some).collect();
    let mut moved = BTreeMap::new();
    for (&new, &old) in &parts.moves {
        let value = slots[old]
            .take()
            .ok_or_else(|| Error::new(path, "duplicate move source"))?;
        moved.insert(new, value);
    }
    for &old in parts.removals.keys() {
        slots[old].take();
    }
    let mut survivors = slots.into_iter().flatten();
    let mut result = Vec::with_capacity(len);
    for index in 0..len {
        let base = if let Some(added) = parts.additions.get(&index) {
            (*added).clone()
        } else if let Some(value) = moved.remove(&index) {
            value
        } else {
            survivors
                .next()
                .ok_or_else(|| Error::new(path, "array delta leaves an unfilled position"))?
        };
        let value = if let Some(child) = parts.changes.get(&index) {
            apply_node_owned(Some(base), child, &pointer(path, index), strict, fuzzy)?.ok_or_else(
                || Error::new(&pointer(path, index), "target array deletion is invalid"),
            )?
        } else {
            base
        };
        result.push(value);
    }
    if survivors.next().is_some() {
        return Err(Error::new(path, "array delta leaves unconsumed values"));
    }
    Ok(result)
}

pub(crate) fn invert(change: &Value, path: &str) -> Result<Value, Error> {
    match change {
        Value::Array(a) => match a.as_slice() {
            [new] => Ok(json!([new, 0, 0])),
            [old, new] => Ok(json!([new, old])),
            [old, _, marker] if marker == &json!(0) => Ok(json!([old])),
            [Value::String(patch), _, marker] if marker == &json!(2) => {
                Ok(json!([text::reverse(patch, path)?, 0, 2]))
            }
            _ => Err(Error::new(path, "invalid leaf delta")),
        },
        Value::Object(map) if delta::is_array(map) => {
            let parts = delta::parts(map, path)?;
            let removed: Vec<_> = parts.removals.keys().copied().collect();
            let mut inserted: Vec<_> = parts
                .additions
                .keys()
                .chain(parts.moves.keys())
                .copied()
                .collect();
            inserted.sort_unstable();
            let mut result = Map::new();
            result.insert("_t".into(), json!("a"));
            for (&old, child) in &parts.removals {
                if child[2] == json!(3) {
                    let new = delta::target(&child[1], path)?;
                    let moved_value = if child[0] != json!("") {
                        if let Some(edit) = parts.changes.get(&new) {
                            apply_node(Some(&child[0]), edit, &pointer(path, new), true)?
                                .ok_or_else(|| Error::new(path, "moved value cannot be deleted"))?
                        } else {
                            child[0].clone()
                        }
                    } else {
                        json!("")
                    };
                    result.insert(format!("_{new}"), json!([moved_value, old, 3]));
                } else {
                    result.insert(old.to_string(), json!([child[0]]));
                }
            }
            for (&new, child) in &parts.additions {
                result.insert(format!("_{new}"), json!([child, 0, 0]));
            }
            for (&new, child) in &parts.changes {
                let old = match parts.moves.get(&new) {
                    Some(old) => *old,
                    None => delta::old_index(new, &removed, &inserted, path)?,
                };
                result.insert(old.to_string(), invert(child, &pointer(path, new))?);
            }
            Ok(Value::Object(result))
        }
        Value::Object(map) => {
            let result = map
                .iter()
                .map(|(key, child)| invert(child, &pointer(path, key)).map(|v| (key.clone(), v)))
                .collect::<Result<_, _>>()?;
            Ok(Value::Object(result))
        }
        _ => Err(Error::new(path, "invalid delta")),
    }
}
