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

fn ancestor_arrays<'a>(left: &Value, ancestor: &'a str) -> Vec<&'a str> {
    let mut arrays = Vec::new();
    let mut cursor = ancestor;
    while let Some(above) = parent(cursor) {
        if left.pointer(above).is_some_and(Value::is_array) {
            arrays.push(above);
        }
        cursor = above;
    }
    arrays
}

#[inline(always)]
fn independent_operation(
    operation: &PatchOperation,
    ancestor: &str,
    destination_inside: bool,
    arrays: &[&str],
) -> bool {
    let destination = path(operation);
    // The caller has already rejected strict ancestor destination writes.
    if let Some(source) = from(operation) {
        let source_inside = contains(ancestor, source);
        if source_inside != destination_inside || contains(source, ancestor) && !source_inside {
            return false;
        }
    }
    // Structural writes directly into any array on the boundary's path
    // can shift that boundary. Deeper writes and Replace do not shift it.
    if matches!(
        operation,
        PatchOperation::Add(_)
            | PatchOperation::Remove(_)
            | PatchOperation::Move(_)
            | PatchOperation::Copy(_)
    ) && parent(destination).is_some_and(|p| arrays.contains(&p))
    {
        return false;
    }
    if let PatchOperation::Move(operation) = operation {
        if parent(operation.from.as_str()).is_some_and(|p| arrays.contains(&p)) {
            return false;
        }
    }
    true
}

// Replacing this complete subtree commutes with the other operations only
// when neither pointers nor reads outside it depend on its intermediate state.
fn independent_replacement(left: &Value, patch: &Patch, ancestor: &str) -> bool {
    let arrays = ancestor_arrays(left, ancestor);
    patch.0.iter().all(|operation| {
        let inside = contains(ancestor, path(operation));
        (inside || !contains(path(operation), ancestor))
            && independent_operation(operation, ancestor, inside, &arrays)
    })
}

// Add-like guards can snapshot their destination parent, even when the
// destination lies outside the replacement. Do not reuse such an ancestor
// snapshot: it may observe the subtree before all selected edits finish.
fn independent_guard_read(
    operation: &PatchOperation,
    ancestor: &str,
    destination_inside: bool,
) -> bool {
    !matches!(
        operation,
        PatchOperation::Add(_) | PatchOperation::Copy(_) | PatchOperation::Move(_)
    ) || destination_inside
        || parent(path(operation)).is_some_and(|parent| !contains(parent, ancestor))
}

fn independent_guard_reads(patch: &Patch, ancestor: &str) -> bool {
    patch.0.iter().all(|operation| {
        independent_guard_read(operation, ancestor, contains(ancestor, path(operation)))
    })
}

fn candidate_applies<'a>(
    left: &Value,
    right: &Value,
    operations: impl Iterator<Item = &'a PatchOperation>,
) -> bool {
    if delta::check_depth(left, 128).is_err() {
        return false;
    }
    let mut shadow = left.clone();
    for (index, operation) in operations.enumerate() {
        if crate::apply_json_patch_step(&mut shadow, operation, index).is_err() {
            return false;
        }
    }
    json_equal(&shadow, right)
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
    let original_length = patch.0.len();
    let (mut current_size, mut guard_costs, mut baseline_verified) = if tests {
        let (size, costs, shadow) =
            guarded_operation_bytes(left, patch.0.iter(), patch.0.len(), &[])?;
        // Only a multi-operation patch with non-root candidates can reuse
        // this proof. Root and single-operation candidates are applied directly.
        let verified = (patch.0.len() > 1 && parents.len() > 1).then(|| json_equal(&shadow, right));
        (size, costs, verified)
    } else {
        let (size, costs) = guarded_bytes(left, &patch, false, &[])?;
        (size, costs, None)
    };
    let mut selections = None;
    // After an accepted replacement, use the original scan rather than
    // rebuilding an index for every independently compressible parent.
    let mut index_unchanged = true;
    for ancestor in parents {
        let (Some(old), Some(new)) = (left.pointer(&ancestor), right.pointer(&ancestor)) else {
            continue;
        };
        let mut selection_proof = None;
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
            let arrays = if tests {
                ancestor_arrays(left, &ancestor)
            } else {
                Vec::new()
            };
            let mut independent = true;
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
                // Guard costing needs this proof for viable candidates.
                // Reuse the scan's destination classification only in guarded
                // mode; unguarded size bounds can reject the candidate first.
                // Copy/read failures only disable the shortcut; they do not
                // change the original Move/ancestor boundary rejection above.
                if tests {
                    independent = independent
                        && independent_operation(operation, &ancestor, inside, &arrays)
                        && independent_guard_read(operation, &ancestor, inside);
                }
                if inside {
                    selected.push(index);
                }
            }
            selection_proof = tests.then_some(independent);
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
        let (candidate_size, replacement_test_size) = if tests {
            let Some(test_size) = bytes_below(
                &ValueOperation {
                    op: "test",
                    path: &ancestor,
                    value: old,
                },
                budget - replacement_size,
            )?
            else {
                continue;
            };
            (None, test_size)
        } else {
            (Some(current_size - removed_size + replacement_size), 0)
        };
        let replaced = decode(json!({"op": "replace", "path": ancestor, "value": new}))?;
        let first = selected[0];
        let selected: BTreeSet<_> = selected.into_iter().collect();
        let candidate = || {
            patch.0.iter().enumerate().filter_map(|(index, operation)| {
                if index == first {
                    Some(&replaced)
                } else if selected.contains(&index) {
                    None
                } else {
                    Some(operation)
                }
            })
        };
        // Independent boundaries preserve both external applications and
        // their guard reads. Their exact per-operation costs can be reused;
        // otherwise retain the complete guard simulation and its final shadow.
        let independent_guards = tests
            && !ancestor.is_empty()
            && baseline_verified == Some(true)
            && selection_proof.unwrap_or_else(|| {
                independent_replacement(left, &patch, &ancestor)
                    && independent_guard_reads(&patch, &ancestor)
            });
        let mut guarded_shadow = None;
        let cost = if let Some(size) = candidate_size {
            Ok((size, Vec::new()))
        } else if ancestor.is_empty() {
            // The one root replacement has no outside guard reads. Its exact
            // cost is known independently of the original patch's final value;
            // candidate_applies below still performs real validation.
            let size = replacement_size + replacement_test_size + 3;
            Ok((
                size,
                vec![
                    GuardCost {
                        bytes: 2,
                        operations: 0,
                    },
                    GuardCost {
                        bytes: size,
                        operations: 2,
                    },
                ],
            ))
        } else if independent_guards {
            Ok(replacement_guard_costs(
                &guard_costs,
                &selected,
                first,
                replacement_size + replacement_test_size + 1,
            ))
        } else {
            guarded_operation_bytes(
                left,
                candidate(),
                patch.0.len() - selected.len() + 1,
                &guard_costs[..=first],
            )
            .map(|(size, costs, shadow)| {
                guarded_shadow = Some(shadow);
                (size, costs)
            })
        };
        if let Ok((candidate_size, candidate_costs)) = cost {
            if candidate_size >= current_size {
                continue;
            }
            let valid = if let Some(shadow) = guarded_shadow {
                json_equal(&shadow, right)
            } else if ancestor.is_empty() || patch.0.len() == 1 {
                // Validating the one-step candidate is cheaper than replaying
                // a potentially large original sequence for a root replacement.
                candidate_applies(left, right, candidate())
            } else if independent_guards {
                true
            } else if selection_proof
                .unwrap_or_else(|| independent_replacement(left, &patch, &ancestor))
                && *baseline_verified.get_or_insert_with(|| {
                    apply_json_patch(left, &patch).is_ok_and(|value| json_equal(&value, right))
                })
            {
                // A verified original patch plus an independent boundary proves
                // that replacing all its interior edits with the known target
                // leaves every external operation and final value unchanged.
                true
            } else {
                candidate_applies(left, right, candidate())
            };
            if valid {
                let mut replacement = Some(replaced);
                patch.0 = std::mem::take(&mut patch.0)
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, operation)| {
                        if index == first {
                            replacement.take()
                        } else if selected.contains(&index) {
                            None
                        } else {
                            Some(operation)
                        }
                    })
                    .collect();
                selections = None;
                index_unchanged = false;
                baseline_verified = Some(true);
                current_size = candidate_size;
                guard_costs = candidate_costs;
            }
        }
    }
    if patch.0.len() < original_length {
        // In-place filtering can retain the original operation allocation.
        // Release it once before guard generation clones the final payloads.
        patch.0.shrink_to_fit();
    }
    Ok(patch)
}

#[derive(Clone, Copy)]
struct GuardCost {
    bytes: usize,
    operations: usize,
}

// Each prefix difference contains one complete operation group: generated
// tests followed by the original operation. Strip its old leading comma and
// rebuild every prefix after replacing the selected groups, including counts.
fn replacement_guard_costs(
    costs: &[GuardCost],
    selected: &BTreeSet<usize>,
    first: usize,
    replacement_bytes: usize,
) -> (usize, Vec<GuardCost>) {
    let mut total = GuardCost {
        bytes: 2,
        operations: 0,
    };
    let mut result = Vec::with_capacity(costs.len() - selected.len() + 1);
    result.push(total);
    for (index, group) in costs.windows(2).enumerate() {
        let (group_bytes, operations) = if index == first {
            (replacement_bytes, 2)
        } else if selected.contains(&index) {
            continue;
        } else {
            (
                group[1].bytes - group[0].bytes - usize::from(group[0].operations > 0),
                group[1].operations - group[0].operations,
            )
        };
        total.bytes += group_bytes + usize::from(total.operations > 0);
        total.operations += operations;
        result.push(total);
    }
    (total.bytes, result)
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
    guarded_operation_bytes(left, patch.0.iter(), patch.0.len(), prefix)
        .map(|(size, costs, _)| (size, costs))
}

fn guarded_operation_bytes<'a>(
    left: &Value,
    operations: impl Iterator<Item = &'a PatchOperation> + Clone,
    length: usize,
    prefix: &[GuardCost],
) -> Result<(usize, Vec<GuardCost>, Value), Error> {
    let mut shadow = left.clone();
    let first = prefix.len().saturating_sub(1);
    // Only the operations before the candidate's first change are identical.
    // Execute them on the same shadow, reusing their exact serialization cost.
    // Recompute every later guard: copy sources and array indices may depend on
    // the replacement even when their operations lie outside its subtree.
    for (index, op) in operations.clone().take(first).enumerate() {
        crate::apply_json_patch_step(&mut shadow, op, index)?;
    }
    let mut costs = Vec::with_capacity(length + 1);
    if prefix.is_empty() {
        costs.push(GuardCost {
            bytes: 2, // Array brackets.
            operations: 0,
        });
    } else {
        costs.extend_from_slice(prefix);
    }
    let mut total = costs[first];
    for (index, op) in operations.enumerate().skip(first) {
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
    Ok((total.bytes, costs, shadow))
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
    fn selection_scan_keeps_copy_guard_and_array_fallbacks_after_parent_acceptance() {
        for kind in 0..3 {
            let mut left = json!({
                "0accepted": {"leaf": {"a_long_property_name": 0, "b_long_property_name": 0}},
                "a": {"a_long_property_name": 0, "b_long_property_name": 0},
                "sink": 0,
                "source": 7,
                "keep": "unchanged 🦀".repeat(500),
            });
            let prefix = json!([
                {"op":"replace","path":"/0accepted/leaf/a_long_property_name","value":1},
                {"op":"replace","path":"/0accepted/leaf/b_long_property_name","value":2},
            ]);
            let suffix = match kind {
                0 => json!([
                    {"op":"replace","path":"/a/a_long_property_name","value":11111},
                    {"op":"copy","from":"/a/b_long_property_name","path":"/sink"},
                    {"op":"replace","path":"/a/b_long_property_name","value":22222},
                    {"op":"replace","path":"/sink","value":9},
                ]),
                1 => json!([
                    {"op":"replace","path":"/a/a_long_property_name","value":11111},
                    {"op":"add","path":"/new","value":7},
                    {"op":"replace","path":"/a/b_long_property_name","value":22222},
                ]),
                _ => {
                    left["a"] = json!([
                        {"tag":"shifted"},
                        {"a_long_property_name":0,"b_long_property_name":0},
                        {"a_long_property_name":9,"b_long_property_name":9},
                    ]);
                    json!([
                        {"op":"replace","path":"/a/1/a_long_property_name","value":11111},
                        {"op":"remove","path":"/a/0"},
                        {"op":"replace","path":"/a/1/b_long_property_name","value":22222},
                    ])
                }
            };
            let mut original = patch(prefix);
            original.0.extend(patch(suffix).0);
            let right = apply_json_patch(&left, &original).unwrap();
            // Frozen from 08f3735: /0accepted/leaf is accepted first, then
            // unsafe intermediate reads/shifts still use full candidate replay.
            let target = if kind == 2 {
                json!([
                    {"a_long_property_name":11111,"b_long_property_name":0},
                    {"a_long_property_name":9,"b_long_property_name":22222},
                ])
            } else {
                json!({"a_long_property_name":11111,"b_long_property_name":22222})
            };
            let mut expected = patch(json!([
                {"op":"replace","path":"/0accepted/leaf","value":{"a_long_property_name":1,"b_long_property_name":2}},
                {"op":"replace","path":"/a","value":target},
            ]));
            if kind == 0 {
                expected.0.extend(
                    patch(json!([
                        {"op":"copy","from":"/a/b_long_property_name","path":"/sink"},
                        {"op":"replace","path":"/sink","value":9},
                    ]))
                    .0,
                );
            } else if kind == 1 {
                expected
                    .0
                    .push(decode(json!({"op":"add","path":"/new","value":7})).unwrap());
            }
            for tests in [false, true] {
                let optimized = rationalize(&left, &right, original.clone(), tests).unwrap();
                assert_eq!(
                    serde_json::to_vec(&optimized).unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
                let output = if tests {
                    guard(left.clone(), &optimized).unwrap()
                } else {
                    optimized
                };
                assert_eq!(apply_json_patch(&left, &output).unwrap(), right);
                let inverse = invert_json_patch(&left, &output).unwrap();
                assert_eq!(apply_json_patch(&right, &inverse).unwrap(), left);
            }
        }
    }

    #[test]
    fn independent_guard_costs_match_full_replay_after_multiple_parent_replacements() {
        let left = json!({
            "a/b~": {"a_long_property_name": 0, "b_long_property_name": "old 🦀\\\""},
            "z~q": {"a_long_property_name": 0, "b_long_property_name": 0},
            "unrelated": {"source": "copy 🦀\n", "old": false},
            "before": 0,
            "after": 0,
            "flag": true,
        });
        for prefix in [false, true] {
            let mut original = patch(json!([
                {"op":"replace","path":"/a~1b~0/a_long_property_name","value":11111},
                {"op":"copy","from":"/unrelated/source","path":"/unrelated/new"},
                {"op":"replace","path":"/a~1b~0/b_long_property_name","value":"new 🦀\n\\\""},
                {"op":"test","path":"/flag","value":true},
                {"op":"replace","path":"/z~0q/a_long_property_name","value":33333},
                {"op":"remove","path":"/unrelated/old"},
                {"op":"replace","path":"/z~0q/b_long_property_name","value":44444},
                {"op":"replace","path":"/after","value":5},
            ]));
            if prefix {
                original.0.insert(
                    0,
                    decode(json!({"op":"replace","path":"/before","value":1})).unwrap(),
                );
            }
            let right = apply_json_patch(&left, &original).unwrap();
            let (_, mut costs) = guarded_bytes(&left, &original, true, &[]).unwrap();
            for ancestor in ["/a~1b~0", "/z~0q"] {
                assert!(independent_replacement(&left, &original, ancestor));
                assert!(independent_guard_reads(&original, ancestor));
                let selected: BTreeSet<_> = original
                    .0
                    .iter()
                    .enumerate()
                    .filter_map(|(index, operation)| {
                        contains(ancestor, path(operation)).then_some(index)
                    })
                    .collect();
                let first = *selected.first().unwrap();
                let replacement = decode(json!({"op":"replace","path":ancestor,"value":right.pointer(ancestor).unwrap()})).unwrap();
                let test_size = bytes(&ValueOperation {
                    op: "test",
                    path: ancestor,
                    value: left.pointer(ancestor).unwrap(),
                })
                .unwrap();
                let (size, next_costs) = replacement_guard_costs(
                    &costs,
                    &selected,
                    first,
                    test_size + bytes(&replacement).unwrap() + 1,
                );
                let candidate = Patch(
                    original
                        .0
                        .iter()
                        .enumerate()
                        .filter_map(|(index, operation)| {
                            if index == first {
                                Some(replacement.clone())
                            } else if selected.contains(&index) {
                                None
                            } else {
                                Some(operation.clone())
                            }
                        })
                        .collect(),
                );
                let actual_guards = guard(left.clone(), &candidate).unwrap();
                let (expected_size, expected_costs) =
                    guarded_bytes(&left, &candidate, true, &[]).unwrap();
                assert_eq!(size, bytes(&actual_guards).unwrap());
                assert_eq!(size, expected_size);
                assert_eq!(next_costs.len(), expected_costs.len());
                for (actual, expected) in next_costs.iter().zip(&expected_costs) {
                    assert_eq!(
                        (actual.bytes, actual.operations),
                        (expected.bytes, expected.operations)
                    );
                }
                assert_eq!(apply_json_patch(&left, &actual_guards).unwrap(), right);
                original = candidate;
                costs = next_costs;
            }
        }
    }

    #[test]
    fn external_add_copy_and_move_can_change_ancestor_guard_snapshot_costs() {
        let left = json!({
            "a/b~": {"a_long_property_name": 0, "b_long_property_name": 0},
            "source": 4,
        });
        for outside in [
            json!({"op":"add","path":"/new","value":4}),
            json!({"op":"copy","from":"/source","path":"/new"}),
            json!({"op":"move","from":"/source","path":"/new"}),
        ] {
            let original = patch(json!([
                {"op":"replace","path":"/a~1b~0/a_long_property_name","value":11111},
                outside,
                {"op":"replace","path":"/a~1b~0/b_long_property_name","value":22222},
            ]));
            let right = apply_json_patch(&left, &original).unwrap();
            let replacement =
                decode(json!({"op":"replace","path":"/a~1b~0","value":right["a/b~"]})).unwrap();
            let candidate = Patch(vec![replacement.clone(), original.0[1].clone()]);
            assert!(independent_replacement(&left, &original, "/a~1b~0"));
            assert!(!independent_guard_reads(&original, "/a~1b~0"));
            assert_eq!(apply_json_patch(&left, &candidate).unwrap(), right);
            let (_, costs) = guarded_bytes(&left, &original, true, &[]).unwrap();
            let test_size = bytes(&ValueOperation {
                op: "test",
                path: "/a~1b~0",
                value: &left["a/b~"],
            })
            .unwrap();
            let (invalid_reused_size, _) = replacement_guard_costs(
                &costs,
                &BTreeSet::from([0, 2]),
                0,
                test_size + bytes(&replacement).unwrap() + 1,
            );
            let full_size = bytes(&guard(left.clone(), &candidate).unwrap()).unwrap();
            assert_ne!(invalid_reused_size, full_size);
        }
    }

    #[test]
    fn guarded_reuse_requires_the_original_patch_to_reach_the_requested_target() {
        let left = json!({
            "a": {"a_long_property_name": 0, "b_long_property_name": 0},
            "omitted": 0,
            "keep": "unchanged 🦀".repeat(1000),
        });
        let original = patch(json!([
            {"op":"replace","path":"/a/a_long_property_name","value":11111},
            {"op":"replace","path":"/a/b_long_property_name","value":22222},
        ]));
        let mut right = apply_json_patch(&left, &original).unwrap();
        right["omitted"] = json!(1);
        assert!(independent_replacement(&left, &original, "/a"));
        assert!(independent_guard_reads(&original, "/a"));
        for tests in [false, true] {
            // A projected/filtered original patch is valid but cannot prove
            // this candidate reaches the complete requested target.
            assert_eq!(
                rationalize(&left, &right, original.clone(), tests).unwrap(),
                original
            );
        }

        let invalid = patch(json!([
            {"op":"replace","path":"/a/missing","value":1},
        ]));
        let expected_error = guarded_bytes(&left, &invalid, true, &[]).err().unwrap();
        assert_eq!(
            rationalize(&left, &right, invalid, true).unwrap_err(),
            expected_error
        );
    }

    #[test]
    fn rationalization_preserves_interleaved_copy_reads_of_a_subtree_and_its_ancestor() {
        let left = json!({
            "a/b~": {"a_long_property_name": 0, "b_long_property_name": 0},
            "copied": null,
            "keep": "unchanged 🦀 ".repeat(1000),
        });
        for source in ["/a~1b~0", ""] {
            let original = patch(json!([
                {"op": "replace", "path": "/a~1b~0/a_long_property_name", "value": 1},
                {"op": "copy", "from": source, "path": "/copied"},
                {"op": "replace", "path": "/a~1b~0/b_long_property_name", "value": 2},
            ]));
            let right = apply_json_patch(&left, &original).unwrap();
            // A parent replacement would expose the final second field to the
            // interleaved copy, which must instead preserve its earlier value.
            let copied = if source.is_empty() {
                &right["copied"]["a/b~"]
            } else {
                &right["copied"]
            };
            assert_eq!(copied["b_long_property_name"], json!(0));
            for tests in [false, true] {
                let optimized = rationalize(&left, &right, original.clone(), tests).unwrap();
                assert_eq!(optimized, original);
                assert_eq!(apply_json_patch(&left, &optimized).unwrap(), right);
            }
        }
    }

    #[test]
    fn rationalization_preserves_interleaved_array_removal_and_move_indices() {
        let left = json!([
            {"tag": "shifted"},
            {"a_long_property_name": 0, "b_long_property_name": 0},
            {"a_long_property_name": 9, "b_long_property_name": 9},
            {"keep": "unchanged 🦀 ".repeat(1000)},
        ]);
        for structural in [
            json!({"op": "remove", "path": "/0"}),
            json!({"op": "move", "from": "/0", "path": "/2"}),
        ] {
            let original = patch(json!([
                {"op": "replace", "path": "/1/a_long_property_name", "value": 1},
                structural,
                {"op": "replace", "path": "/1/b_long_property_name", "value": 2},
            ]));
            let right = apply_json_patch(&left, &original).unwrap();
            // The two operations at /1 act on different original elements.
            // Combining them must not move the replacement across the shift.
            assert_eq!(right[0]["a_long_property_name"], json!(1));
            assert_eq!(right[1]["b_long_property_name"], json!(2));
            for tests in [false, true] {
                let optimized = rationalize(&left, &right, original.clone(), tests).unwrap();
                assert_eq!(optimized, original);
                assert_eq!(apply_json_patch(&left, &optimized).unwrap(), right);
            }
        }
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
