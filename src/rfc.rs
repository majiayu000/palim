use crate::{Error, Patch, PatchOperation, apply_json_patch, delta, json_equal, pointer};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
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
        patch = guard(left.clone(), &patch)?;
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

struct LimitedByteCount {
    count: usize,
    limit: usize,
    exceeded: bool,
}

impl Write for LimitedByteCount {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let next = self
            .count
            .checked_add(data.len())
            .ok_or_else(|| io::Error::other("JSON size exceeds address space"))?;
        if next >= self.limit {
            self.exceeded = true;
            return Err(io::Error::other("candidate cannot fit budget"));
        }
        self.count = next;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bytes_below<T: Serialize + ?Sized>(value: &T, limit: usize) -> Result<Option<usize>, Error> {
    let mut output = LimitedByteCount {
        count: 0,
        limit,
        exceeded: false,
    };
    match serde_json::to_writer(&mut output, value) {
        Ok(()) => Ok((output.count < limit).then_some(output.count)),
        Err(_) if output.exceeded => Ok(None),
        Err(error) => Err(Error::new("", error.to_string())),
    }
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
    // A single operation has no earlier edits to replay. Search the initial
    // document directly instead of building a whole-tree scalar cache.
    if patch.0.len() == 1 {
        let op = &patch.0[0];
        let (value, destination, replace) = match op {
            PatchOperation::Add(op) => (&op.value, op.path.as_str(), false),
            PatchOperation::Replace(op) => (&op.value, op.path.as_str(), true),
            _ => return Ok(patch),
        };
        let eligible = !replace
            || parent(destination).is_none_or(|p| !left.pointer(p).is_some_and(Value::is_array));
        if eligible {
            let original_size = bytes(op)?;
            let minimum_size = bytes(&CopyOperation {
                op: "copy",
                from: "",
                path: destination,
            })?;
            if minimum_size < original_size {
                let mut best = None;
                find_copy(
                    left,
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
                        patch.0[0] = copied;
                    }
                }
            }
        }
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

// Index destinations once while the original patch is unchanged. A rejected
// candidate does not affect these lists or their original operation order.
fn selection_index(patch: &Patch) -> (HashMap<String, Vec<usize>>, HashSet<String>, Vec<usize>) {
    let mut selected: HashMap<String, Vec<usize>> = HashMap::new();
    let mut destinations = HashSet::new();
    let mut moves = Vec::new();
    for (index, operation) in patch.0.iter().enumerate() {
        destinations.insert(path(operation).to_owned());
        let mut cursor = path(operation);
        loop {
            selected.entry(cursor.to_owned()).or_default().push(index);
            let Some(ancestor) = parent(cursor) else {
                break;
            };
            cursor = ancestor;
        }
        if matches!(operation, PatchOperation::Move(_)) {
            moves.push(index);
        }
    }
    (selected, destinations, moves)
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
    if parents.is_empty() {
        return Ok(patch);
    }
    let mut parents: Vec<_> = parents.into_iter().collect();
    let broad_first = tests
        && patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Move(_)));
    parents.sort_by(|a, b| {
        let depth = a.matches('/').count().cmp(&b.matches('/').count());
        // Guarded array moves can snapshot the whole container for every
        // candidate. Try broad replacements first so an accepted parent removes
        // its descendants before they trigger those repeated simulations.
        // Other patches retain the finer-first compression heuristic.
        (if broad_first { depth } else { depth.reverse() }).then_with(|| a.cmp(b))
    });
    let (mut current_size, mut guard_costs) = guarded_bytes(left, &patch, tests, &[])?;
    let mut selections = None;
    // After an accepted replacement, use the original scan rather than
    // rebuilding an index for every independently compressible parent.
    let mut index_unchanged = true;
    for ancestor in parents {
        let (Some(old), Some(new)) = (left.pointer(&ancestor), right.pointer(&ancestor)) else {
            continue;
        };
        let (selected, boundary_crossed) = if ancestor.is_empty() {
            ((0..patch.0.len()).collect::<Vec<_>>(), false)
        } else if patch.0.len() == 1 {
            let operation = &patch.0[0];
            let destination_inside = contains(&ancestor, path(operation));
            let crossed = (contains(path(operation), &ancestor) && !destination_inside)
                || matches!(operation, PatchOperation::Move(_))
                    && from(operation)
                        .is_some_and(|source| contains(&ancestor, source) != destination_inside);
            (
                if destination_inside {
                    vec![0]
                } else {
                    Vec::new()
                },
                crossed,
            )
        } else if !index_unchanged {
            let mut selected = Vec::new();
            let mut crossed = false;
            for (index, operation) in patch.0.iter().enumerate() {
                let inside = contains(&ancestor, path(operation));
                if contains(path(operation), &ancestor) && !inside {
                    crossed = true;
                    break;
                }
                if matches!(operation, PatchOperation::Move(_))
                    && from(operation).is_some_and(|source| contains(&ancestor, source) != inside)
                {
                    crossed = true;
                    break;
                }
                if inside {
                    selected.push(index);
                }
            }
            (selected, crossed)
        } else {
            let (selected_by_parent, destinations, moves) =
                selections.get_or_insert_with(|| selection_index(&patch));
            let selected = selected_by_parent
                .get(&ancestor)
                .cloned()
                .unwrap_or_default();
            let mut crossed = false;
            let mut cursor = ancestor.as_str();
            while let Some(above) = parent(cursor) {
                if destinations.contains(above) {
                    crossed = true;
                    break;
                }
                cursor = above;
            }
            if !crossed {
                for &index in moves.iter() {
                    if let PatchOperation::Move(operation) = &patch.0[index] {
                        if contains(&ancestor, operation.from.as_str())
                            != contains(&ancestor, operation.path.as_str())
                        {
                            crossed = true;
                            break;
                        }
                    }
                }
            }
            (selected, crossed)
        };
        if boundary_crossed || selected.is_empty() {
            continue;
        }
        let removed_size = if tests {
            0
        } else {
            selected
                .iter()
                .try_fold(0, |size, index| bytes(&patch.0[*index]).map(|n| size + n))?
                + selected.len()
                - 1
        };
        let budget = if tests {
            current_size.saturating_sub(3)
        } else {
            removed_size
        };
        let Some(replacement_size) = bytes_below(
            &ValueOperation {
                op: "replace",
                path: &ancestor,
                value: new,
            },
            budget,
        )?
        else {
            continue;
        };
        let candidate_size = if tests {
            if bytes_below(
                &ValueOperation {
                    op: "test",
                    path: &ancestor,
                    value: old,
                },
                budget - replacement_size,
            )?
            .is_none()
            {
                continue;
            }
            None
        } else {
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
        if let Ok((candidate_size, candidate_costs)) = candidate_size.map_or_else(
            || guarded_bytes(left, &candidate, true, &guard_costs[..=first]),
            |size| Ok((size, Vec::new())),
        ) {
            if candidate_size < current_size
                && apply_json_patch(left, &candidate).is_ok_and(|value| json_equal(&value, right))
            {
                patch = candidate;
                selections = None;
                index_unchanged = false;
                current_size = candidate_size;
                guard_costs = candidate_costs;
            }
        }
    }
    Ok(patch)
}

#[derive(Clone, Copy)]
struct GuardCost {
    bytes: usize,
    operations: usize,
}

fn guarded_bytes(
    left: &Value,
    patch: &Patch,
    tests: bool,
    prefix: &[GuardCost],
) -> Result<(usize, Vec<GuardCost>), Error> {
    if !tests {
        return Ok((bytes(patch)?, Vec::new()));
    }
    let mut shadow = left.clone();
    let first = prefix.len().saturating_sub(1);
    // Only the operations before the candidate's first change are identical.
    // Execute them on the same shadow, reusing their exact serialization cost.
    // Recompute every later guard: copy sources and array indices may depend on
    // the replacement even when their operations lie outside its subtree.
    for (index, op) in patch.0[..first].iter().enumerate() {
        crate::apply_json_patch_step(&mut shadow, op, index)?;
    }
    let mut costs = Vec::with_capacity(patch.0.len() + 1);
    if prefix.is_empty() {
        costs.push(GuardCost {
            bytes: 2, // Array brackets.
            operations: 0,
        });
    } else {
        costs.extend_from_slice(prefix);
    }
    let mut total = costs[first];
    for (index, op) in patch.0.iter().enumerate().skip(first) {
        for (path, value) in guard_values(&shadow, op)?.into_iter().flatten() {
            total.bytes += bytes(&ValueOperation {
                op: "test",
                path,
                value,
            })? + usize::from(total.operations > 0);
            total.operations += 1;
        }
        total.bytes += bytes(op)? + usize::from(total.operations > 0);
        total.operations += 1;
        crate::apply_json_patch_step(&mut shadow, op, index)?;
        costs.push(total);
    }
    Ok((total.bytes, costs))
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

fn guard(mut shadow: Value, patch: &Patch) -> Result<Patch, Error> {
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
    invert_with_target(left, patch).map(|(inverse, _)| inverse)
}

/// Generate an inverse with tests for the values and containers it will touch.
/// Guards are computed from the successfully patched baseline. They do not
/// authenticate unrelated document fields, and container tests can be large.
pub fn invert_json_patch_guarded(left: &Value, patch: &Patch) -> Result<Patch, Error> {
    let (inverse, right) = invert_with_target(left, patch)?;
    guard(right, &inverse)
}

fn invert_with_target(left: &Value, patch: &Patch) -> Result<(Patch, Value), Error> {
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
    Ok((Patch(groups.into_iter().rev().flatten().collect()), shadow))
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

#[cfg(test)]
mod guard_cost_tests {
    use super::*;

    fn patch(value: Value) -> Patch {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn bounded_cost_handles_escaped_utf8_and_exact_budget_boundaries() {
        let value = json!({"a~/\"": ["line\n🦀\\", 1, null]});
        for op in ["replace", "test"] {
            let candidate = ValueOperation {
                op,
                path: "/a~1b~0/\"🦀",
                value: &value,
            };
            let actual = serde_json::to_vec(&candidate).unwrap().len();
            assert_eq!(bytes_below(&candidate, actual + 1).unwrap(), Some(actual));
            assert_eq!(bytes_below(&candidate, actual).unwrap(), None);
            assert_eq!(bytes_below(&candidate, actual - 1).unwrap(), None);
            assert_eq!(bytes_below(&candidate, 0).unwrap(), None);
        }
    }

    #[test]
    fn bounded_cost_stops_traversal_and_preserves_real_serializer_errors() {
        use serde::{Serializer, ser::SerializeSeq};
        use std::cell::Cell;

        struct Counted<'a>(&'a Cell<usize>);
        impl Serialize for Counted<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(Some(10_000))?;
                for _ in 0..10_000 {
                    self.0.set(self.0.get() + 1);
                    sequence.serialize_element("unused payload")?;
                }
                sequence.end()
            }
        }
        let visits = Cell::new(0);
        assert_eq!(bytes_below(&Counted(&visits), 64).unwrap(), None);
        assert!(visits.get() < 10);

        struct Fails;
        impl Serialize for Fails {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(None)?;
                sequence.serialize_element("written before failure")?;
                Err(serde::ser::Error::custom(
                    "deliberate serialization failure",
                ))
            }
        }
        let expected = bytes(&Fails).unwrap_err();
        assert_eq!(bytes_below(&Fails, usize::MAX).unwrap_err(), expected);
    }

    #[test]
    fn single_replace_in_an_array_cannot_become_an_inserting_copy() {
        let text = "reusable payload".repeat(100);
        let left = json!([text, "old"]);
        let right = json!([text, text]);
        let standard = patch(json!([{"op":"replace","path":"/1","value":text}]));
        assert_eq!(
            factorize(&left, &right, standard.clone()).unwrap(),
            standard
        );
        assert_eq!(apply_json_patch(&left, &standard).unwrap(), right);
    }

    #[test]
    fn guard_costs_match_serialized_guards_for_empty_root_and_two_guard_operations() {
        let cases = [
            (Value::Null, Patch::default()),
            (
                json!(0),
                patch(json!([{"op":"replace","path":"","value":true}])),
            ),
            (
                json!(["first", "second"]),
                patch(json!([
                    {"op":"copy","from":"/0","path":"/1"}
                ])),
            ),
        ];
        for (left, patch) in cases {
            let actual = guard(left.clone(), &patch).unwrap();
            let expected = bytes(&actual).unwrap();
            let (size, costs) = guarded_bytes(&left, &patch, true, &[]).unwrap();
            assert_eq!(size, expected);
            assert_eq!(costs.len(), patch.0.len() + 1);
            assert_eq!(costs[0].bytes, 2);
            assert_eq!(costs[0].operations, 0);
            assert_eq!(costs.last().unwrap().operations, actual.0.len());
            for end in 0..=patch.0.len() {
                assert_eq!(
                    guarded_bytes(&left, &patch, true, &costs[..=end])
                        .unwrap()
                        .0,
                    expected
                );
            }
            let (size, costs) = guarded_bytes(&left, &patch, false, &[]).unwrap();
            assert_eq!(size, bytes(&patch).unwrap());
            assert!(costs.is_empty());
        }
    }

    #[test]
    fn guard_cost_prefixes_replay_dependent_copy_and_array_suffixes() {
        let left = json!({"a":[1,2,3],"source":{"x":0,"y":"old"},"dest":[]});
        let body = "new 中文🦀 \" body".repeat(20);
        let original = patch(json!([
            {"op":"move","from":"/a/2","path":"/a/0"},
            {"op":"copy","from":"/a/0","path":"/dest/0"},
            {"op":"replace","path":"/source/x","value":1},
            {"op":"copy","from":"/source/y","path":"/dest/1"},
            {"op":"replace","path":"/source/y","value":body},
            {"op":"remove","path":"/a/1"},
            {"op":"move","from":"/a/1","path":"/dest/2"}
        ]));
        let (_, costs) = guarded_bytes(&left, &original, true, &[]).unwrap();
        let candidate = patch(json!([
            {"op":"move","from":"/a/2","path":"/a/0"},
            {"op":"copy","from":"/a/0","path":"/dest/0"},
            {"op":"replace","path":"/source","value":{"x":1,"y":body}},
            {"op":"copy","from":"/source/y","path":"/dest/1"},
            {"op":"remove","path":"/a/1"},
            {"op":"move","from":"/a/1","path":"/dest/2"}
        ]));
        let actual = guard(left.clone(), &candidate).unwrap();
        let (size, candidate_costs) = guarded_bytes(&left, &candidate, true, &costs[..=2]).unwrap();
        assert_eq!(size, bytes(&actual).unwrap());
        assert_eq!(candidate_costs.last().unwrap().operations, actual.0.len());
        assert!(
            actual
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Test(op)
            if op.path.as_str() == "/source/y" && op.value == json!(body)))
        );
        // The copy now reads the replacement's new y, not the original y.
        // Correct pricing does not replace the complete candidate validation.
        assert_ne!(
            apply_json_patch(&left, &original).unwrap(),
            apply_json_patch(&left, &candidate).unwrap()
        );

        // After dropping an earlier operation, the next candidate's first
        // changed index uses the new patch's costs, not the original indices.
        let next = patch(json!([
            {"op":"move","from":"/a/2","path":"/a/0"},
            {"op":"copy","from":"/a/0","path":"/dest/0"},
            {"op":"replace","path":"/source","value":{"x":1,"y":body}},
            {"op":"copy","from":"/source/y","path":"/dest/1"},
            {"op":"replace","path":"/a","value":[3]},
            {"op":"add","path":"/dest/2","value":2}
        ]));
        let actual = guard(left.clone(), &next).unwrap();
        let (size, costs) = guarded_bytes(&left, &next, true, &candidate_costs[..=4]).unwrap();
        assert_eq!(size, bytes(&actual).unwrap());
        assert_eq!(costs.last().unwrap().operations, actual.0.len());
        assert_eq!(
            apply_json_patch(&left, &next).unwrap(),
            apply_json_patch(&left, &candidate).unwrap()
        );
    }
}
