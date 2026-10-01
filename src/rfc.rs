use crate::{Error, Patch, PatchOperation, apply_json_patch, delta, json_equal, pointer};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::io::{self, Write};

/// Options for producing RFC 6902 patches.
#[derive(Clone, Debug)]
pub struct JsonPatchOptions {
    /// Reuse existing values with move/copy when that produces a smaller patch.
    pub factorize: bool,
    /// Replace a changed parent when its serialized patch is smaller.
    pub rationalize: bool,
    /// Include tests for the values and containers affected by each operation.
    pub tests: bool,
}

impl Default for JsonPatchOptions {
    fn default() -> Self {
        Self {
            factorize: true,
            rationalize: true,
            tests: false,
        }
    }
}

/// Compare documents and return a standard JSON Patch.
pub fn diff_json_patch(
    left: &Value,
    right: &Value,
    options: &JsonPatchOptions,
) -> Result<Patch, Error> {
    crate::DiffPatcher::default().diff_json_patch(left, right, options)
}

pub(crate) fn optimize(
    left: &Value,
    right: &Value,
    mut patch: Patch,
    options: &JsonPatchOptions,
) -> Result<Patch, Error> {
    if options.factorize {
        patch = factorize(left, right, patch)?;
    }
    if options.rationalize {
        patch = rationalize(left, right, patch, options.tests)?;
    }
    if options.tests {
        patch = guard(left, &patch)?;
    }
    Ok(patch)
}

pub(crate) fn decode(value: Value) -> Result<PatchOperation, Error> {
    use json_patch::{
        AddOperation, CopyOperation, MoveOperation, RemoveOperation, ReplaceOperation,
        TestOperation, jsonptr::PointerBuf,
    };
    let Value::Object(mut fields) = value else {
        return Err(Error::new(
            "",
            "generated JSON Patch operation is not an object",
        ));
    };
    let Some(Value::String(operation)) = fields.remove("op") else {
        return Err(Error::new(
            "",
            "generated JSON Patch operation is missing op",
        ));
    };
    let take_pointer = |fields: &mut serde_json::Map<String, Value>, name: &str| {
        let Some(Value::String(value)) = fields.remove(name) else {
            return Err(Error::new(
                "",
                format!("generated JSON Patch operation is missing {name}"),
            ));
        };
        PointerBuf::parse(value).map_err(|error| Error::new("", error.to_string()))
    };
    let path = take_pointer(&mut fields, "path")?;
    let take_value = |fields: &mut serde_json::Map<String, Value>| {
        fields
            .remove("value")
            .ok_or_else(|| Error::new("", "generated JSON Patch operation is missing value"))
    };
    // Construct typed operations directly. Serde's internally tagged enum
    // buffer cannot consume Value's u128 representation for arbitrary-precision
    // integers; passing owned Value payloads avoids both that buffer and a JSON
    // serialization/parsing round trip.
    Ok(match operation.as_str() {
        "add" => PatchOperation::Add(AddOperation {
            path,
            value: take_value(&mut fields)?,
        }),
        "remove" => PatchOperation::Remove(RemoveOperation { path }),
        "replace" => PatchOperation::Replace(ReplaceOperation {
            path,
            value: take_value(&mut fields)?,
        }),
        "move" => PatchOperation::Move(MoveOperation {
            from: take_pointer(&mut fields, "from")?,
            path,
        }),
        "copy" => PatchOperation::Copy(CopyOperation {
            from: take_pointer(&mut fields, "from")?,
            path,
        }),
        "test" => PatchOperation::Test(TestOperation {
            path,
            value: take_value(&mut fields)?,
        }),
        _ => {
            return Err(Error::new(
                "",
                "unrecognized generated JSON Patch operation",
            ));
        }
    })
}

fn path(op: &PatchOperation) -> &str {
    match op {
        PatchOperation::Add(op) => op.path.as_str(),
        PatchOperation::Remove(op) => op.path.as_str(),
        PatchOperation::Replace(op) => op.path.as_str(),
        PatchOperation::Move(op) => op.path.as_str(),
        PatchOperation::Copy(op) => op.path.as_str(),
        PatchOperation::Test(op) => op.path.as_str(),
    }
}

fn from(op: &PatchOperation) -> Option<&str> {
    match op {
        PatchOperation::Move(op) => Some(op.from.as_str()),
        PatchOperation::Copy(op) => Some(op.from.as_str()),
        _ => None,
    }
}

fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

fn contains(parent: &str, path: &str) -> bool {
    parent == path || (path.starts_with(parent) && path.as_bytes().get(parent.len()) == Some(&b'/'))
}

struct ByteCount(usize);

impl Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("JSON size exceeds address space"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bytes<T: Serialize + ?Sized>(value: &T) -> Result<usize, Error> {
    let mut output = ByteCount(0);
    serde_json::to_writer(&mut output, value).map_err(|error| Error::new("", error.to_string()))?;
    Ok(output.0)
}

#[derive(Serialize)]
struct ValueOperation<'a> {
    op: &'a str,
    path: &'a str,
    value: &'a Value,
}

#[derive(Serialize)]
struct CopyOperation<'a> {
    op: &'a str,
    from: &'a str,
    path: &'a str,
}

fn factorize(left: &Value, right: &Value, mut patch: Patch) -> Result<Patch, Error> {
    // Pair removes with additions first. A candidate is accepted only if its
    // sequential indices still produce the target; intervening array edits can
    // otherwise make a seemingly valid remove/add pair unsafe to turn into move.
    while patch
        .0
        .iter()
        .any(|op| matches!(op, PatchOperation::Remove(_)))
        && patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Add(_)))
    {
        let mut shadow = left.clone();
        let mut removed: HashMap<Value, Vec<(usize, String)>> = HashMap::new();
        for (index, op) in patch.0.iter().enumerate() {
            if let PatchOperation::Remove(op) = op {
                if let Some(value) = shadow.pointer(op.path.as_str()) {
                    removed
                        .entry(value.clone())
                        .or_default()
                        .push((index, op.path.to_string()));
                }
            }
            crate::apply_json_patch_step(&mut shadow, op, index)?;
        }
        let mut replacement = None;
        'additions: for (add_index, op) in patch.0.iter().enumerate() {
            let PatchOperation::Add(add) = op else {
                continue;
            };
            let Some(sources) = removed.get(&add.value) else {
                continue;
            };
            for (remove_index, source) in sources {
                if contains(source, add.path.as_str()) {
                    continue;
                }
                let moved = decode(json!({"op": "move", "from": source, "path": add.path}))?;
                // Replacing two operations with one also removes a comma.
                // Reject expensive pairs before cloning the entire patch.
                if bytes(&moved)? > bytes(&patch.0[*remove_index])? + bytes(op)? {
                    continue;
                }
                let candidate = Patch(
                    patch
                        .0
                        .iter()
                        .enumerate()
                        .filter_map(|(index, op)| {
                            if index == *remove_index {
                                None
                            } else if index == add_index {
                                Some(moved.clone())
                            } else {
                                Some(op.clone())
                            }
                        })
                        .collect(),
                );
                if apply_json_patch(left, &candidate).is_ok_and(|value| json_equal(&value, right)) {
                    replacement = Some(candidate);
                    break 'additions;
                }
            }
        }
        match replacement {
            Some(candidate) => patch = candidate,
            None => break,
        }
    }

    // A copy source must exist immediately before that operation. Do not use
    // values from the initial document after an earlier operation removed them.
    if !patch
        .0
        .iter()
        .any(|op| matches!(op, PatchOperation::Add(_) | PatchOperation::Replace(_)))
    {
        return Ok(patch);
    }
    // Retain removed scalars too: absence from this overapproximation proves
    // that searching the current document cannot find a matching copy source.
    // Neither this cache nor the sequential shadow is needed until a copy can
    // actually be smaller. Advance the shadow only when a search needs it.
    let mut scalars = HashMap::new();
    let mut loaded_scalars = 0;
    let mut source_reduction = 0;
    let mut shadow = None;
    let mut applied = 0;
    for index in 0..patch.0.len() {
        let op = &patch.0[index];
        let value = match op {
            PatchOperation::Add(op) => Some(&op.value),
            PatchOperation::Replace(op) => Some(&op.value),
            _ => None,
        };
        let mut replacement = None;
        if let Some(value) = value {
            let destination = path(op);
            let original_size = bytes(op)?;
            let minimum_size = bytes(&CopyOperation {
                op: "copy",
                from: "",
                path: destination,
            })?;
            if minimum_size < original_size {
                let possible = if let Some(key) = scalar_key(value) {
                    let kind = scalar_kind(value);
                    if loaded_scalars & kind == 0 {
                        if loaded_scalars == 0 {
                            for previous in &patch.0 {
                                if let Some(source) = from(previous) {
                                    source_reduction += path_minimum(source)?
                                        .saturating_sub(path_minimum(path(previous))?);
                                }
                            }
                        }
                        collect_scalars(left, 0, &mut scalars, kind)?;
                        for previous in &patch.0[..index] {
                            let prefix = path_minimum(path(previous))?;
                            match previous {
                                PatchOperation::Add(op) => {
                                    collect_scalars(&op.value, prefix, &mut scalars, kind)?;
                                }
                                PatchOperation::Replace(op) => {
                                    collect_scalars(&op.value, prefix, &mut scalars, kind)?;
                                }
                                _ => {}
                            }
                        }
                        loaded_scalars |= kind;
                    }
                    scalars.get(&key).is_some_and(|cost: &usize| {
                        cost.saturating_sub(source_reduction) < original_size - minimum_size
                    })
                } else {
                    true
                };
                if possible {
                    if index > 0 {
                        let shadow = shadow.get_or_insert_with(|| left.clone());
                        for (offset, previous) in patch.0[applied..index].iter().enumerate() {
                            crate::apply_json_patch_step(shadow, previous, applied + offset)?;
                        }
                        applied = index;
                    }
                    let current = shadow.as_ref().unwrap_or(left);
                    // Copy inserts into arrays; it cannot directly replace an
                    // array element. Check the parent in the current document.
                    let eligible = !matches!(op, PatchOperation::Replace(_))
                        || parent(destination).is_none_or(|parent| {
                            !current.pointer(parent).is_some_and(Value::is_array)
                        });
                    if eligible {
                        let mut best = None;
                        find_copy(
                            current,
                            "",
                            value,
                            destination,
                            original_size - minimum_size,
                            &mut best,
                        )?;
                        if let Some((source, _)) = best {
                            let copied =
                                decode(json!({"op": "copy", "from": source, "path": destination}))?;
                            if bytes(&copied)? < original_size {
                                replacement = Some(copied);
                            }
                        }
                    }
                }
            }
        }
        if let Some(replacement) = replacement {
            if loaded_scalars != 0 {
                if let Some(source) = from(&replacement) {
                    source_reduction +=
                        path_minimum(source)?.saturating_sub(path_minimum(path(&replacement))?);
                }
            }
            patch.0[index] = replacement;
        }
        // Include nested payloads for later operations. Moves and copies only
        // reuse values already present, so they introduce no new scalar keys.
        if loaded_scalars != 0 {
            let prefix = path_minimum(path(&patch.0[index]))?;
            match &patch.0[index] {
                PatchOperation::Add(op) => {
                    collect_scalars(&op.value, prefix, &mut scalars, loaded_scalars)?;
                }
                PatchOperation::Replace(op) => {
                    collect_scalars(&op.value, prefix, &mut scalars, loaded_scalars)?;
                }
                _ => {}
            }
        }
    }
    Ok(patch)
}

#[derive(Hash, PartialEq, Eq)]
enum ScalarKey {
    Null,
    Bool(bool),
    String(String),
    Number(crate::numbers::NumberKey<'static>),
}

fn scalar_key(value: &Value) -> Option<ScalarKey> {
    Some(match value {
        Value::Null => ScalarKey::Null,
        Value::Bool(value) => ScalarKey::Bool(*value),
        Value::String(value) => ScalarKey::String(value.clone()),
        Value::Number(value) => ScalarKey::Number(crate::numbers::number_key(value).into_owned()),
        Value::Array(_) | Value::Object(_) => return None,
    })
}

fn scalar_kind(value: &Value) -> u8 {
    match value {
        Value::Null => 1,
        Value::Bool(_) => 2,
        Value::String(_) => 4,
        Value::Number(_) => 8,
        Value::Array(_) | Value::Object(_) => 0,
    }
}

fn numeric_token(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_digit())
}

fn path_minimum(path: &str) -> Result<usize, Error> {
    // Array insertions/removals can shorten decimal indices. Treat every
    // numeric token as one digit, even numeric object keys, for a safe bound.
    let mut shortest = String::with_capacity(path.len());
    for token in path.split('/').skip(1) {
        shortest.push('/');
        shortest.push_str(if numeric_token(token) { "0" } else { token });
    }
    bytes(&shortest).map(|size| size - 2)
}

fn collect_scalars(
    value: &Value,
    prefix: usize,
    output: &mut HashMap<ScalarKey, usize>,
    kinds: u8,
) -> Result<(), Error> {
    let kind = scalar_kind(value);
    if kind != 0 {
        if kind & kinds != 0 {
            if let Some(key) = scalar_key(value) {
                output
                    .entry(key)
                    .and_modify(|cost| *cost = (*cost).min(prefix))
                    .or_insert(prefix);
            }
        }
        return Ok(());
    }
    match value {
        Value::Array(values) => {
            for child in values {
                collect_scalars(child, prefix + 2, output, kinds)?;
            }
        }
        Value::Object(values) => {
            for (key, child) in values {
                let suffix = if numeric_token(key) {
                    2
                } else {
                    bytes(&pointer("", key))? - 2
                };
                collect_scalars(child, prefix + suffix, output, kinds)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn find_copy(
    value: &Value,
    path: &str,
    wanted: &Value,
    destination: &str,
    budget: usize,
    best: &mut Option<(String, usize)>,
) -> Result<(), Error> {
    // A source pointer adds its serialized string contents to the empty-from
    // copy's size. Its raw UTF-8 length is a safe lower bound on that cost.
    // Descendant pointers only grow, so unprofitable branches need no search.
    let limit = best.as_ref().map_or(budget, |(_, cost)| budget.min(*cost));
    if path.len() >= limit {
        return Ok(());
    }
    if path != destination && json_equal(value, wanted) {
        let cost = bytes(path)? - 2;
        if cost < limit {
            *best = Some((path.to_owned(), cost));
        }
    }
    match value {
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                let limit = best.as_ref().map_or(budget, |(_, cost)| budget.min(*cost));
                let digits = index.checked_ilog10().map_or(1, |power| power as usize + 1);
                if path.len() + 1 + digits >= limit {
                    // Later decimal array indices cannot have shorter paths.
                    break;
                }
                find_copy(
                    child,
                    &pointer(path, index),
                    wanted,
                    destination,
                    budget,
                    best,
                )?;
            }
        }
        Value::Object(values) => {
            for (key, child) in values {
                let limit = best.as_ref().map_or(budget, |(_, cost)| budget.min(*cost));
                let escaped = key
                    .bytes()
                    .filter(|byte| matches!(byte, b'~' | b'/'))
                    .count();
                if path.len() + 1 + key.len() + escaped >= limit {
                    continue;
                }
                find_copy(
                    child,
                    &pointer(path, key),
                    wanted,
                    destination,
                    budget,
                    best,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn rationalize(left: &Value, right: &Value, mut patch: Patch, tests: bool) -> Result<Patch, Error> {
    let mut parents = BTreeSet::new();
    for op in &patch.0 {
        let mut cursor = path(op);
        while let Some(ancestor) = parent(cursor) {
            parents.insert(ancestor.to_owned());
            cursor = ancestor;
        }
    }
    let mut parents: Vec<_> = parents.into_iter().collect();
    parents.sort_by(|a, b| {
        b.matches('/')
            .count()
            .cmp(&a.matches('/').count())
            .then_with(|| a.cmp(b))
    });
    let mut current_size = guarded_bytes(left, &patch, tests)?;
    for ancestor in parents {
        let (Some(old), Some(new)) = (left.pointer(&ancestor), right.pointer(&ancestor)) else {
            continue;
        };
        let mut selected = Vec::new();
        let mut boundary_crossed = false;
        for (index, op) in patch.0.iter().enumerate() {
            let destination_inside = contains(&ancestor, path(op));
            // Replacing only a child cannot replace an operation on its parent.
            if contains(path(op), &ancestor) && !destination_inside {
                boundary_crossed = true;
                break;
            }
            if let Some(source) = from(op) {
                let source_inside = contains(&ancestor, source);
                if matches!(op, PatchOperation::Move(_)) && source_inside != destination_inside {
                    boundary_crossed = true;
                    break;
                }
            }
            if destination_inside {
                selected.push(index);
            }
        }
        if boundary_crossed || selected.is_empty() {
            continue;
        }
        let replacement_size = bytes(&ValueOperation {
            op: "replace",
            path: &ancestor,
            value: new,
        })?;
        let candidate_size = if tests {
            // Every candidate needs its replacement and the old-value guard.
            // Count borrowed payloads first, before cloning a large subtree or
            // simulating all of the candidate's other guards.
            let minimum_size = replacement_size
                + bytes(&ValueOperation {
                    op: "test",
                    path: &ancestor,
                    value: old,
                })?
                + 3; // The surrounding brackets and the test/replace comma.
            if minimum_size >= current_size {
                continue;
            }
            None
        } else {
            let removed_size = selected
                .iter()
                .try_fold(0, |size, index| bytes(&patch.0[*index]).map(|n| size + n))?
                + selected.len()
                - 1;
            if replacement_size >= removed_size {
                continue;
            }
            Some(current_size - removed_size + replacement_size)
        };
        let replaced = decode(json!({"op": "replace", "path": ancestor, "value": new}))?;
        let first = selected[0];
        let selected: BTreeSet<_> = selected.into_iter().collect();
        let candidate = Patch(
            patch
                .0
                .iter()
                .enumerate()
                .filter_map(|(index, op)| {
                    if index == first {
                        Some(replaced.clone())
                    } else if selected.contains(&index) {
                        None
                    } else {
                        Some(op.clone())
                    }
                })
                .collect(),
        );
        // The exact serialized size includes UTF-8, escaping, paths, commas and,
        // when requested, the tests this candidate would need.
        if let Ok(candidate_size) =
            candidate_size.map_or_else(|| guarded_bytes(left, &candidate, true), Ok)
        {
            if candidate_size < current_size
                && apply_json_patch(left, &candidate).is_ok_and(|value| json_equal(&value, right))
            {
                patch = candidate;
                current_size = candidate_size;
            }
        }
    }
    Ok(patch)
}

fn guarded_bytes(left: &Value, patch: &Patch, tests: bool) -> Result<usize, Error> {
    if !tests {
        return bytes(patch);
    }
    let mut shadow = left.clone();
    let mut size = 2; // Array brackets.
    let mut count = 0;
    for (index, op) in patch.0.iter().enumerate() {
        for (path, value) in guard_values(&shadow, op)?.into_iter().flatten() {
            size += bytes(&ValueOperation {
                op: "test",
                path,
                value,
            })? + usize::from(count > 0);
            count += 1;
        }
        size += bytes(op)? + usize::from(count > 0);
        count += 1;
        crate::apply_json_patch_step(&mut shadow, op, index)?;
    }
    Ok(size)
}

fn test_source<'a>(shadow: &'a Value, path: &str) -> Result<&'a Value, Error> {
    shadow
        .pointer(path)
        .ok_or_else(|| Error::new(path, "test source is missing"))
}

fn guard_add_value<'a, 'b>(
    shadow: &'a Value,
    path: &'b str,
) -> Result<(&'b str, &'a Value), Error> {
    let Some(parent) = parent(path) else {
        return Ok(("", shadow));
    };
    let container = shadow
        .pointer(parent)
        .ok_or_else(|| Error::new(parent, "addition parent is missing"))?;
    // RFC test has no "must be absent" operation. Testing the parent protects
    // missing object keys, append positions and the order of array insertions.
    if container.is_array() {
        return Ok((parent, container));
    }
    Ok(shadow
        .pointer(path)
        .map_or((parent, container), |value| (path, value)))
}

fn guard_values<'a, 'b>(
    shadow: &'a Value,
    op: &'b PatchOperation,
) -> Result<[Option<(&'b str, &'a Value)>; 2], Error> {
    Ok(match op {
        PatchOperation::Add(op) => [Some(guard_add_value(shadow, op.path.as_str())?), None],
        PatchOperation::Remove(op) => [
            Some((op.path.as_str(), test_source(shadow, op.path.as_str())?)),
            None,
        ],
        PatchOperation::Replace(op) => [
            Some((op.path.as_str(), test_source(shadow, op.path.as_str())?)),
            None,
        ],
        PatchOperation::Move(op) => [
            Some((op.from.as_str(), test_source(shadow, op.from.as_str())?)),
            Some(guard_add_value(shadow, op.path.as_str())?),
        ],
        PatchOperation::Copy(op) => [
            Some((op.from.as_str(), test_source(shadow, op.from.as_str())?)),
            Some(guard_add_value(shadow, op.path.as_str())?),
        ],
        PatchOperation::Test(_) => [None, None],
    })
}

fn guard(left: &Value, patch: &Patch) -> Result<Patch, Error> {
    let mut shadow = left.clone();
    let mut output = Vec::new();
    for (index, op) in patch.0.iter().enumerate() {
        for (path, value) in guard_values(&shadow, op)?.into_iter().flatten() {
            output.push(decode(json!({"op": "test", "path": path, "value": value}))?);
        }
        output.push(op.clone());
        crate::apply_json_patch_step(&mut shadow, op, index)?;
    }
    Ok(Patch(output))
}

/// Produce an inverse from the original operations and their complete baseline.
/// Tests make no changes and therefore produce no inverse operation. A move may
/// need remove/add/replace operations to restore a destination it overwrote.
pub fn invert_json_patch(left: &Value, patch: &Patch) -> Result<Patch, Error> {
    delta::check_depth(left, 128)?;
    let mut shadow = left.clone();
    let mut groups = Vec::new();
    for (index, op) in patch.0.iter().enumerate() {
        let inverse = match op {
            PatchOperation::Add(op) => undo_add(&shadow, op.path.as_str(), None),
            PatchOperation::Copy(op) => undo_add(&shadow, op.path.as_str(), None),
            PatchOperation::Remove(op) => old_operation(&shadow, "add", op.path.as_str()),
            PatchOperation::Replace(op) => old_operation(&shadow, "replace", op.path.as_str()),
            PatchOperation::Test(_) => Ok(Vec::new()),
            PatchOperation::Move(op) => undo_move(&shadow, op.from.as_str(), op.path.as_str()),
        };
        // Execute the original operation first so malformed patches preserve
        // its real error, operation index and path rather than an inverse error.
        crate::apply_json_patch_step(&mut shadow, op, index)?;
        groups.push(inverse?);
    }
    Ok(Patch(groups.into_iter().rev().flatten().collect()))
}

fn old_operation(before: &Value, op: &str, path: &str) -> Result<Vec<PatchOperation>, Error> {
    let value = before
        .pointer(path)
        .ok_or_else(|| Error::new(path, "inverse source is missing"))?;
    Ok(vec![decode(
        json!({"op": op, "path": path, "value": value}),
    )?])
}

// Translate a lookup in the document after a removal to the original document.
// Only removing an array element shifts a later sibling and its descendants.
fn before_removal_path(before: &Value, path: &str, removed: Option<&str>) -> Result<String, Error> {
    let Some(removed) = removed else {
        return Ok(path.to_owned());
    };
    let Some((container, removed_index)) = removed.rsplit_once('/') else {
        return Ok(path.to_owned());
    };
    if !before.pointer(container).is_some_and(Value::is_array)
        || path == container
        || !contains(container, path)
    {
        return Ok(path.to_owned());
    }
    let removed_index = removed_index
        .parse::<usize>()
        .map_err(|_| Error::new(removed, "invalid removal index"))?;
    let suffix = &path[container.len() + 1..];
    let (index, remainder) = suffix
        .split_once('/')
        .map_or((suffix, ""), |(index, tail)| (index, tail));
    let index = index
        .parse::<usize>()
        .map_err(|_| Error::new(path, "invalid array index"))?;
    if index < removed_index {
        return Ok(path.to_owned());
    }
    let index = index
        .checked_add(1)
        .ok_or_else(|| Error::new(path, "array index overflow"))?;
    let translated = pointer(container, index);
    Ok(if remainder.is_empty() {
        translated
    } else {
        format!("{translated}/{remainder}")
    })
}

fn undo_add(
    before: &Value,
    path: &str,
    removed: Option<&str>,
) -> Result<Vec<PatchOperation>, Error> {
    let Some(container_path) = parent(path) else {
        return old_operation(before, "replace", "");
    };
    let lookup_parent = before_removal_path(before, container_path, removed)?;
    let container = before
        .pointer(&lookup_parent)
        .ok_or_else(|| Error::new(path, "inverse destination parent is missing"))?;
    if let Value::Array(values) = container {
        let token = path.rsplit_once('/').map(|(_, token)| token).unwrap_or("");
        let actual_path = if token == "-" {
            let same_array_removal =
                removed.is_some_and(|removed| parent(removed) == Some(container_path));
            let index = values
                .len()
                .checked_sub(usize::from(same_array_removal))
                .ok_or_else(|| Error::new(path, "invalid append index"))?;
            pointer(container_path, index)
        } else {
            path.to_owned()
        };
        return Ok(vec![decode(json!({"op": "remove", "path": actual_path}))?]);
    }
    if !container.is_object() {
        return Err(Error::new(
            path,
            "inverse destination parent is not a container",
        ));
    }
    let lookup = before_removal_path(before, path, removed)?;
    match before.pointer(&lookup) {
        Some(value) => Ok(vec![decode(
            json!({"op": "replace", "path": path, "value": value}),
        )?]),
        None => Ok(vec![decode(json!({"op": "remove", "path": path}))?]),
    }
}

fn undo_move(
    before: &Value,
    source: &str,
    destination: &str,
) -> Result<Vec<PatchOperation>, Error> {
    if source == destination {
        return Ok(Vec::new());
    }
    // An object/root destination can overwrite an ancestor containing the
    // source. Restoring that whole value also restores the removed descendant.
    if contains(destination, source)
        && parent(destination)
            .is_none_or(|parent| before.pointer(parent).is_some_and(Value::is_object))
    {
        return old_operation(before, "replace", destination);
    }
    let mut result = undo_add(before, destination, Some(source))?;
    result.extend(old_operation(before, "add", source)?);
    Ok(result)
}
