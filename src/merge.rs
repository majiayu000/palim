use crate::{Error, delta::check_depth, pointer};
use serde_json::{Map, Value};

/// Apply an RFC 7396 JSON Merge Patch without changing the source.
///
/// Object members with null patch values are removed. Other members are merged
/// recursively; an object patch treats a non-object source as an empty object.
/// A non-object patch replaces the entire source, including a root null patch.
/// Source and patch nesting must not exceed 128 containers.
pub fn merge_patch(source: &Value, patch: &Value) -> Result<Value, Error> {
    check_depth(source, 128)?;
    check_depth(patch, 128)?;
    let mut result = source.clone();
    apply(&mut result, patch);
    Ok(result)
}

/// Apply an RFC 7396 merge patch atomically to a mutable source.
///
/// On error the source is unchanged. A private result is committed only after
/// input validation and application have succeeded.
pub fn merge_patch_in_place(source: &mut Value, patch: &Value) -> Result<(), Error> {
    let result = merge_patch(source, patch)?;
    *source = result;
    Ok(())
}

/// Generate an RFC 7396 merge patch transforming left into right.
///
/// Null in an object patch means deletion, so a newly added null-valued object
/// member or an existing member changed to null cannot be represented. Those
/// cases return an error at the affected JSON Pointer. Unchanged null members,
/// root null replacements, and nulls inside replaced arrays are supported.
/// Unchanged object roots yield `{}`; unchanged non-object roots yield that
/// same value, because applying `{}` would turn them into an object.
pub fn diff_merge_patch(left: &Value, right: &Value) -> Result<Value, Error> {
    check_depth(left, 128)?;
    check_depth(right, 128)?;
    difference(left, right, "")
}

/// Compose two RFC 7396 merge patches for every possible source document.
///
/// The result has the same effect as applying first and then second. RFC 7396
/// cannot encode every composition: an object patch following a replacement
/// or deletion must discard unknown source members, while a single object
/// patch would preserve them. Such compositions return an error at the first
/// unrepresentable node rather than silently changing their meaning.
pub fn compose_merge_patches(first: &Value, second: &Value) -> Result<Value, Error> {
    check_depth(first, 128)?;
    check_depth(second, 128)?;
    compose(first, second, "")
}

fn apply(target: &mut Value, patch: &Value) {
    let Value::Object(changes) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    if let Value::Object(values) = target {
        for (key, change) in changes {
            if change.is_null() {
                values.remove(key);
            } else {
                apply(values.entry(key.clone()).or_insert(Value::Null), change);
            }
        }
    }
}

fn difference(left: &Value, right: &Value, path: &str) -> Result<Value, Error> {
    let Value::Object(target) = right else {
        return Ok(right.clone());
    };
    let source = left.as_object();
    let mut changes = Map::new();
    if let Some(source) = source {
        for key in source.keys() {
            if !target.contains_key(key) {
                changes.insert(key.clone(), Value::Null);
            }
        }
    }
    for (key, value) in target {
        let previous = source.and_then(|values| values.get(key));
        if previous == Some(value) {
            continue;
        }
        let child_path = pointer(path, key);
        if value.is_null() {
            return Err(Error::new(
                &child_path,
                "RFC 7396 cannot assign null to an object member; null means deletion",
            ));
        }
        changes.insert(
            key.clone(),
            difference(previous.unwrap_or(&Value::Null), value, &child_path)?,
        );
    }
    Ok(Value::Object(changes))
}

fn compose(first: &Value, second: &Value, path: &str) -> Result<Value, Error> {
    let Value::Object(next) = second else {
        return Ok(second.clone());
    };
    let Value::Object(previous) = first else {
        return Err(Error::new(
            path,
            "RFC 7396 cannot compose an object patch after a replacement or deletion for every source",
        ));
    };
    let mut combined = previous.clone();
    for (key, value) in next {
        let merged = match previous.get(key) {
            Some(old) if value.is_object() => compose(old, value, &pointer(path, key))?,
            _ => value.clone(),
        };
        // Null must remain a deletion instruction, not be applied to first.
        combined.insert(key.clone(), merged);
    }
    Ok(Value::Object(combined))
}
