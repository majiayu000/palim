use crate::{Error, pointer, text};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

// serde_json's default reader rejects its 128th nested container. Keep produced
// and imported deltas within the default wire reader's round-trip boundary.
pub(crate) const MAX_DELTA_DEPTH: usize = 127;

pub(crate) fn check_depth(value: &Value, limit: usize) -> Result<(), Error> {
    measure_depth(value, limit).map(|_| ())
}

// Return the actual container depth during the existing boundary check.
pub(crate) fn measure_depth(value: &Value, limit: usize) -> Result<usize, Error> {
    let mut pending = vec![(value, 0)];
    let mut maximum = 0;
    while let Some((value, depth)) = pending.pop() {
        match value {
            Value::Array(values) => {
                if depth >= limit {
                    return Err(Error::new("", "JSON nesting exceeds max_depth"));
                }
                maximum = maximum.max(depth + 1);
                pending.extend(
                    values
                        .iter()
                        .filter(|v| v.is_array() || v.is_object())
                        .map(|v| (v, depth + 1)),
                );
            }
            Value::Object(values) => {
                if depth >= limit {
                    return Err(Error::new("", "JSON nesting exceeds max_depth"));
                }
                maximum = maximum.max(depth + 1);
                pending.extend(
                    values
                        .values()
                        .filter(|v| v.is_array() || v.is_object())
                        .map(|v| (v, depth + 1)),
                );
            }
            _ => {}
        }
    }
    Ok(maximum)
}

pub(crate) fn index(key: &str, path: &str) -> Result<usize, Error> {
    let value = key
        .parse::<usize>()
        .map_err(|_| Error::new(path, "invalid array index"))?;
    if value.to_string() != key {
        return Err(Error::new(path, "array index must be canonical decimal"));
    }
    Ok(value)
}
pub(crate) fn target(value: &Value, path: &str) -> Result<usize, Error> {
    value
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(|| Error::new(path, "move destination must be a nonnegative integer"))
}
pub(crate) fn is_array(map: &Map<String, Value>) -> bool {
    map.get("_t") == Some(&json!("a"))
}

pub(crate) fn validate(value: &Value, path: &str, depth: usize) -> Result<(), Error> {
    if depth >= 128 {
        return Err(Error::new(path, "delta nesting exceeds 128"));
    }
    match value {
        Value::Array(fields) => match fields.as_slice() {
            [_] | [_, _] => Ok(()),
            [_, zero, marker] if zero == &json!(0) && marker == &json!(0) => Ok(()),
            [Value::String(patch), zero, marker] if zero == &json!(0) && marker == &json!(2) => {
                text::parse(patch, path).map(|_| ())
            }
            _ => Err(Error::new(path, "invalid leaf delta")),
        },
        Value::Object(map) if is_array(map) => {
            let mut destinations = BTreeSet::new();
            for (key, child) in map {
                if key == "_t" {
                    continue;
                }
                let p = pointer(path, key);
                if let Some(old) = key.strip_prefix('_') {
                    index(old, &p)?;
                    match child.as_array().map(Vec::as_slice) {
                        Some([_, zero, marker]) if zero == &json!(0) && marker == &json!(0) => {}
                        Some([_, to, marker]) if marker == &json!(3) => {
                            let to = target(to, &p)?;
                            if !destinations.insert(to) {
                                return Err(Error::new(&p, "duplicate insertion destination"));
                            }
                        }
                        _ => {
                            return Err(Error::new(
                                &p,
                                "original indices permit only deletion or move",
                            ));
                        }
                    }
                } else {
                    let i = index(key, &p)?;
                    validate(child, &p, depth + 1)?;
                    if let Some(fields) = child.as_array() {
                        if fields.len() == 1 {
                            if !destinations.insert(i) {
                                return Err(Error::new(&p, "duplicate insertion destination"));
                            }
                        } else if fields.len() == 3 && fields[2] == json!(0) {
                            return Err(Error::new(
                                &p,
                                "array deletion requires an original index",
                            ));
                        }
                    }
                }
            }
            // A moved value may also have a modification, but a new value cannot.
            Ok(())
        }
        Value::Object(map) => {
            for (key, child) in map {
                validate(child, &pointer(path, key), depth + 1)?;
            }
            Ok(())
        }
        _ => Err(Error::new(path, "delta must be an object or a leaf tuple")),
    }
}

pub(crate) struct ArrayParts<'a> {
    pub removals: BTreeMap<usize, &'a Value>,
    pub additions: BTreeMap<usize, &'a Value>,
    pub moves: BTreeMap<usize, usize>, // target -> original
    pub changes: BTreeMap<usize, &'a Value>,
}
pub(crate) fn parts<'a>(map: &'a Map<String, Value>, path: &str) -> Result<ArrayParts<'a>, Error> {
    let mut result = ArrayParts {
        removals: BTreeMap::new(),
        additions: BTreeMap::new(),
        moves: BTreeMap::new(),
        changes: BTreeMap::new(),
    };
    for (key, value) in map {
        if key == "_t" {
            continue;
        }
        let p = pointer(path, key);
        if let Some(old) = key.strip_prefix('_') {
            let old = index(old, &p)?;
            result.removals.insert(old, value);
            if value.get(2) == Some(&json!(3)) {
                result.moves.insert(target(&value[1], &p)?, old);
            }
        } else {
            let new = index(key, &p)?;
            if value.as_array().is_some_and(|a| a.len() == 1) {
                result.additions.insert(new, &value[0]);
            } else {
                result.changes.insert(new, value);
            }
        }
    }
    Ok(result)
}

/// Recover the old index of a surviving item using ranks, without knowing array size.
pub(crate) fn old_index(
    new: usize,
    removed: &[usize],
    inserted: &[usize],
    path: &str,
) -> Result<usize, Error> {
    let rank = new
        .checked_sub(inserted.partition_point(|i| *i < new))
        .ok_or_else(|| Error::new(path, "inconsistent array index mapping"))?;
    let mut lo = rank;
    let mut hi = rank
        .checked_add(removed.len())
        .ok_or_else(|| Error::new(path, "array index overflow"))?;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let count = mid + 1 - removed.partition_point(|i| *i <= mid);
        if count <= rank {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

pub(crate) fn strip_old(value: &Value) -> Value {
    match value {
        Value::Array(a) if a.len() == 2 => json!([0, a[1]]),
        Value::Array(a) if a.len() == 3 && a[2] == json!(0) => json!([0, 0, 0]),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if k == "_t" && v == "a" {
                            v.clone()
                        } else {
                            strip_old(v)
                        },
                    )
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}
