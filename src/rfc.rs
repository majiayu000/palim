use crate::{Error, Patch, PatchOperation, apply_json_patch, delta, json_equal, pointer};
use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};
use std::io::{self, Write};

/// Options for producing RFC 6902 patches.
#[derive(Clone, Debug)]
pub struct JsonPatchOptions {
    /// Reuse existing values with move/copy when that produces a smaller patch.
    pub factorize: bool,
    /// Replace a changed parent when its serialized patch is smaller.
    pub rationalize: bool,
    /// Include tests authenticating the values and containers touched by the patch.
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
    op.path().as_str()
}

// Ancestors derived from typed operation paths remain valid JSON pointers.
// Resolving through jsonptr borrows unescaped tokens instead of allocating
// two replacement strings for every segment as Value::pointer does.
fn resolve<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    json_patch::jsonptr::Pointer::parse(path)
        .ok()?
        .resolve(value)
        .ok()
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

pub(crate) fn bytes<T: Serialize + ?Sized>(value: &T) -> Result<usize, Error> {
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

// Disjoint object-member writes commute. Pairing a remove with an add at the
// add's original position leaves every other read/write unchanged, including
// when the add precedes the remove. Keep the generic candidate replay for
// arrays, repeated paths, ancestor writes and source-dependent operations.
// The caller has replayed the original patch and one accepted candidate, so
// errors, source depth and target equality have already been checked.
fn factorize_object_moves(
    left: &Value,
    patch: &mut Patch,
    removed: &mut HashMap<Value, VecDeque<(usize, String)>>,
) -> Result<bool, Error> {
    let mut destinations = HashSet::with_capacity(patch.0.len());
    for op in &patch.0 {
        if !matches!(
            op,
            PatchOperation::Add(_) | PatchOperation::Remove(_) | PatchOperation::Replace(_)
        ) || !parent(path(op)).is_some_and(|p| resolve(left, p).is_some_and(Value::is_object))
            || !destinations.insert(path(op))
        {
            return Ok(false);
        }
    }
    for destination in &destinations {
        let mut ancestor = parent(destination);
        while let Some(p) = ancestor {
            if destinations.contains(p) {
                return Ok(false);
            }
            ancestor = parent(p);
        }
    }
    let mut deleted = vec![false; patch.0.len()];
    let mut moves = Vec::new();
    for (add_index, op) in patch.0.iter().enumerate() {
        let PatchOperation::Add(add) = op else {
            continue;
        };
        let Some(sources) = removed.get_mut(&add.value) else {
            continue;
        };
        for (position, (remove_index, source)) in sources.iter().enumerate() {
            let moved = decode(json!({"op":"move", "from":source, "path":add.path}))?;
            if bytes(&moved)? > bytes(&patch.0[*remove_index])? + bytes(op)? {
                continue;
            }
            deleted[*remove_index] = true;
            moves.push((add_index, moved));
            sources.remove(position);
            break;
        }
    }
    if !moves.is_empty() {
        let mut moves = moves.into_iter().peekable();
        patch.0 = std::mem::take(&mut patch.0)
            .into_iter()
            .enumerate()
            .filter_map(|(index, op)| {
                if deleted[index] {
                    None
                } else if moves
                    .peek()
                    .is_some_and(|(add_index, _)| *add_index == index)
                {
                    moves.next().map(|(_, moved)| moved)
                } else {
                    Some(op)
                }
            })
            .collect();
    }
    Ok(true)
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
        let mut removed: HashMap<Value, VecDeque<(usize, String)>> = HashMap::new();
        for (index, op) in patch.0.iter().enumerate() {
            if let PatchOperation::Remove(op) = op {
                if let Ok(value) = op.path.resolve(&shadow) {
                    removed
                        .entry(value.clone())
                        .or_default()
                        .push_back((index, op.path.to_string()));
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
                let candidate = patch.0.iter().enumerate().filter_map(|(index, op)| {
                    if index == *remove_index {
                        None
                    } else if index == add_index {
                        Some(&moved)
                    } else {
                        Some(op)
                    }
                });
                if candidate_applies(left, right, candidate) {
                    replacement = Some((*remove_index, add_index, moved));
                    break 'additions;
                }
            }
        }
        match replacement {
            Some((remove_index, add_index, moved)) => {
                // Delay the independence proof until a real candidate succeeds:
                // patches with no matching values need no extra scan. Disjoint
                // writes make every later pairing equivalent to this replay.
                if removed.values().map(VecDeque::len).sum::<usize>() > 1
                    && factorize_object_moves(left, &mut patch, &mut removed)?
                {
                    break;
                }
                // Validation borrowed the operations, including their payloads.
                // Only an accepted candidate needs a new dense operation vector.
                let mut moved = Some(moved);
                patch.0 = std::mem::take(&mut patch.0)
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, op)| {
                        if index == remove_index {
                            None
                        } else if index == add_index {
                            moved.take()
                        } else {
                            Some(op)
                        }
                    })
                    .collect();
            }
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
            || parent(destination).is_none_or(|p| !resolve(left, p).is_some_and(Value::is_array));
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
                            !resolve(current, parent).is_some_and(Value::is_array)
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
        if resolve(left, above).is_some_and(Value::is_array) {
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

// Replace and object-member Add/Remove have no external shifting indices or
// source reads across a boundary without strict ancestor writes. Preflight
// excludes dependent external parent snapshots. Fine-first candidates can only
// contain an accepted boundary or be disjoint from it, so the original selection
// lists remain valid when removed slots are filtered out. Keep one exact byte
// weight per slot, including its group's leading comma, and compact once.
fn rationalize_replacements(
    left: &Value,
    right: &Value,
    patch: &mut Patch,
    parents: &[String],
    mut current_size: usize,
    guard_costs: (&[GuardCost], bool),
    baseline_verified: &mut Option<bool>,
) -> Result<bool, Error> {
    let (guard_costs, exact_guard_baseline) = guard_costs;
    let tests = !guard_costs.is_empty();
    // Leaf destinations that are never parent candidates need no selection
    // entry. Borrow the already owned candidate paths instead of cloning them.
    let mut selections: HashMap<&str, Vec<usize>> = parents
        .iter()
        .map(|ancestor| (ancestor.as_str(), Vec::new()))
        .collect();
    for (index, op) in patch.0.iter().enumerate() {
        let mut cursor = path(op);
        loop {
            if let Some(selected) = selections.get_mut(cursor) {
                selected.push(index);
            }
            let Some(above) = parent(cursor) else {
                break;
            };
            cursor = above;
        }
    }
    if tests {
        let mut additions: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, op) in patch.0.iter().enumerate() {
            if matches!(op, PatchOperation::Add(_)) {
                if let Some(parent) = parent(path(op)) {
                    additions.entry(parent).or_default().push(index);
                }
            }
        }
        // An outside Add can snapshot an ancestor containing this boundary.
        // Before its first edit the boundary is unchanged; after its last edit
        // it equals the exact target. An interleaved snapshot observes an
        // intermediate value, so retain the original whole-patch fallback.
        // Keep original intervals even after compression: they are conservative
        // for later ancestors. Decide every boundary before changing any slots.
        if !additions.is_empty() {
            for ancestor in parents {
                let Some(selected) = selections.get(ancestor.as_str()) else {
                    continue;
                };
                let (Some(&first), Some(&last)) = (selected.first(), selected.last()) else {
                    continue;
                };
                let mut cursor = ancestor.as_str();
                while let Some(above) = parent(cursor) {
                    if let Some(indices) = additions.get(above) {
                        let after_first = indices.partition_point(|index| *index <= first);
                        if !exact_guard_baseline
                            // An earlier ancestor test also suppresses interior
                            // tests. A replacement cannot reuse their old costs.
                            || indices.first().is_some_and(|index| *index <= first)
                            || indices.get(after_first).is_some_and(|index| *index < last)
                        {
                            return Ok(false);
                        }
                    }
                    cursor = above;
                }
            }
        }
    }
    // A strict ancestor write cannot be removed by an earlier fine-first
    // candidate. Freeze these flags while paths can still be borrowed, then
    // release the destination set before replacing any operation slots.
    let crossed: Vec<_> = {
        let destinations: HashSet<_> = patch.0.iter().map(path).collect();
        parents
            .iter()
            .map(|ancestor| {
                let mut cursor = ancestor.as_str();
                while let Some(above) = parent(cursor) {
                    if destinations.contains(above) {
                        return true;
                    }
                    cursor = above;
                }
                false
            })
            .collect()
    };
    let mut costs = if tests {
        guard_costs
            .windows(2)
            .map(|group| group[1].bytes - group[0].bytes + usize::from(group[0].operations == 0))
            .collect::<Vec<_>>()
    } else {
        patch
            .0
            .iter()
            .map(|op| bytes(op).map(|size| size + 1))
            .collect::<Result<Vec<_>, _>>()?
    };
    let original_length = patch.0.len();
    let mut length = original_length;
    for (ancestor, crossed) in parents.iter().zip(crossed) {
        let (Some(old), Some(new)) = (resolve(left, ancestor), resolve(right, ancestor)) else {
            continue;
        };
        if crossed {
            continue;
        }
        let selected: Vec<_> = selections
            .get(ancestor.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|index| costs[*index] != 0)
            .collect();
        if selected.is_empty() {
            continue;
        }
        let removed: usize = selected.iter().map(|index| costs[*index]).sum();
        let budget = if tests {
            current_size.saturating_sub(3)
        } else {
            removed - 1
        };
        let Some(replacement_size) = bytes_below(
            &ValueOperation {
                op: "replace",
                path: ancestor,
                value: new,
            },
            budget,
        )?
        else {
            continue;
        };
        let test_size = if tests {
            let Some(size) = bytes_below(
                &ValueOperation {
                    op: "test",
                    path: ancestor,
                    value: old,
                },
                budget - replacement_size,
            )?
            else {
                continue;
            };
            size + 1
        } else {
            0
        };
        let replaced = decode(json!({"op": "replace", "path": ancestor, "value": new}))?;
        let replacement_cost = replacement_size + test_size + 1;
        let candidate_size = current_size - removed + replacement_cost;
        if candidate_size >= current_size {
            continue;
        }
        if ancestor.is_empty() {
            // Root is the last fine-first candidate. No later selection needs
            // the stable slots or index, so release them before real validation
            // clones the complete document and root replacement payload.
            drop(selections);
            if length < original_length {
                let mut index = 0;
                patch.0.retain(|_| {
                    let keep = costs[index] != 0;
                    index += 1;
                    keep
                });
                patch.0.shrink_to_fit();
            }
            drop(costs);
            if candidate_applies(left, right, std::iter::once(&replaced)) {
                patch.0 = vec![replaced];
            }
            return Ok(true);
        }
        if length == 1 {
            if !candidate_applies(left, right, std::iter::once(&replaced)) {
                continue;
            }
        } else if !*baseline_verified.get_or_insert_with(|| {
            apply_json_patch(left, patch).is_ok_and(|value| json_equal(&value, right))
        }) {
            // No replacement has been accepted before this lazy check. Leave
            // the original patch intact for the full candidate-validation path.
            return Ok(false);
        }
        let first = selected[0];
        patch.0[first] = replaced;
        costs[first] = replacement_cost;
        for &index in &selected[1..] {
            costs[index] = 0;
            // Removed groups need no pointer or payload. Only their stable
            // vector slot remains until the final compaction.
            patch.0[index] = PatchOperation::Replace(json_patch::ReplaceOperation {
                path: Default::default(),
                value: Value::Null,
            });
        }
        length -= selected.len() - 1;
        current_size = candidate_size;
        *baseline_verified = Some(true);
    }
    if length < original_length {
        let mut index = 0;
        patch.0.retain(|_| {
            let keep = costs[index] != 0;
            index += 1;
            keep
        });
        patch.0.shrink_to_fit();
    }
    Ok(true)
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
    let (mut current_size, mut guard_costs, mut baseline_verified, exact_guard_baseline) = if tests
    {
        let (size, costs, shadow) =
            guarded_operation_bytes(left, patch.0.iter(), patch.0.len(), &[])?;
        // Only a multi-operation patch with non-root candidates can reuse
        // this proof. Root and single-operation candidates are applied directly.
        let verified = (patch.0.len() > 1 && parents.len() > 1).then(|| json_equal(&shadow, right));
        // A later outside parent snapshot can reuse its exact byte cost only
        // when the original final values preserve the target's representation.
        // Mathematical equality still governs the general optimizer's proof.
        let exact = patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Add(_)))
            && shadow == *right;
        (size, costs, verified, exact)
    } else {
        let (size, costs) = guarded_bytes(left, &patch, false, &[])?;
        (size, costs, None, false)
    };
    if patch.0.len() > 1
        && parents.len() > 1
        && patch.0.iter().all(|op| match op {
            PatchOperation::Replace(_) => true,
            PatchOperation::Add(_) | PatchOperation::Remove(_) => parent(path(op))
                .is_some_and(|parent| resolve(left, parent).is_some_and(Value::is_object)),
            _ => false,
        })
        && baseline_verified != Some(false)
        && rationalize_replacements(
            left,
            right,
            &mut patch,
            &parents,
            current_size,
            (&guard_costs, exact_guard_baseline),
            &mut baseline_verified,
        )?
    {
        return Ok(patch);
    }
    let mut selections = None;
    // After an accepted replacement, use the original scan rather than
    // rebuilding an index for every independently compressible parent.
    let mut index_unchanged = true;
    for ancestor in parents {
        let (Some(old), Some(new)) = (resolve(left, &ancestor), resolve(right, &ancestor)) else {
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
    let mut tested = BTreeSet::new();
    let first = prefix.len().saturating_sub(1);
    // Only the operations before the candidate's first change are identical.
    // Execute them on the same shadow, reusing their exact serialization cost.
    // Recompute every later guard: copy sources and array indices may depend on
    // the replacement even when their operations lie outside its subtree.
    for (index, op) in operations.clone().take(first).enumerate() {
        unique_guard_values(&shadow, op, &mut tested)?;
        invalidate_guards(&shadow, op, &mut tested);
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
        for (path, value) in unique_guard_values(&shadow, op, &mut tested)?
            .into_iter()
            .flatten()
        {
            let size = bytes(&ValueOperation {
                op: "test",
                path,
                value,
            })?;
            total.bytes += size + usize::from(total.operations > 0);
            total.operations += 1;
        }
        total.bytes += bytes(op)? + usize::from(total.operations > 0);
        total.operations += 1;
        invalidate_guards(&shadow, op, &mut tested);
        crate::apply_json_patch_step(&mut shadow, op, index)?;
        costs.push(total);
    }
    Ok((total.bytes, costs, shadow))
}

fn test_source<'a>(
    shadow: &'a Value,
    path: &json_patch::jsonptr::Pointer,
) -> Result<&'a Value, Error> {
    path.resolve(shadow)
        .map_err(|_| Error::new(path.as_str(), "test source is missing"))
}

fn guard_add_value<'a, 'b>(
    shadow: &'a Value,
    path: &'b json_patch::jsonptr::Pointer,
) -> Result<(&'b str, &'a Value), Error> {
    let Some(parent) = path.parent() else {
        return Ok(("", shadow));
    };
    let container = parent
        .resolve(shadow)
        .map_err(|_| Error::new(parent.as_str(), "addition parent is missing"))?;
    // RFC test has no "must be absent" operation. Testing the parent protects
    // missing object keys, append positions and the order of array insertions.
    if container.is_array() {
        return Ok((parent.as_str(), container));
    }
    Ok(path
        .resolve(shadow)
        .map_or((parent.as_str(), container), |value| (path.as_str(), value)))
}

fn guard_values<'a, 'b>(
    shadow: &'a Value,
    op: &'b PatchOperation,
) -> Result<[Option<(&'b str, &'a Value)>; 2], Error> {
    Ok(match op {
        PatchOperation::Add(op) => [Some(guard_add_value(shadow, &op.path)?), None],
        PatchOperation::Remove(op) => [
            Some((op.path.as_str(), test_source(shadow, &op.path)?)),
            None,
        ],
        PatchOperation::Replace(op) => [
            Some((op.path.as_str(), test_source(shadow, &op.path)?)),
            None,
        ],
        PatchOperation::Move(op) => [
            Some((op.from.as_str(), test_source(shadow, &op.from)?)),
            Some(guard_add_value(shadow, &op.path)?),
        ],
        PatchOperation::Copy(op) => [
            Some((op.from.as_str(), test_source(shadow, &op.from)?)),
            Some(guard_add_value(shadow, &op.path)?),
        ],
        PatchOperation::Test(_) => [None, None],
    })
}

// Container tests certify their whole subtree. Deterministic interior edits do
// not require another snapshot, but replacing that boundary or shifting its
// array ancestors invalidates certificates below the changed path.
fn unique_guard_values<'a, 'b>(
    shadow: &'a Value,
    op: &'b PatchOperation,
    tested: &mut BTreeSet<String>,
) -> Result<[Option<(&'b str, &'a Value)>; 2], Error> {
    let mut guards = guard_values(shadow, op)?;
    if let PatchOperation::Test(test) = op {
        if (test.value.is_array() || test.value.is_object())
            && !guarded_path(tested, test.path.as_str())
        {
            // The caller applies this operation before continuing; a failing
            // explicit test returns its original error and no generated patch.
            forget_guards(tested, test.path.as_str(), false);
            tested.insert(test.path.to_string());
        }
    }
    for guard in &mut guards {
        let Some((path, value)) = *guard else {
            continue;
        };
        if guarded_path(tested, path) {
            *guard = None;
        } else if value.is_array() || value.is_object() {
            forget_guards(tested, path, false);
            tested.insert(path.to_owned());
        }
    }
    Ok(guards)
}

fn guarded_path(tested: &BTreeSet<String>, mut path: &str) -> bool {
    loop {
        if tested.contains(path) {
            return true;
        }
        let Some(above) = parent(path) else {
            return false;
        };
        path = above;
    }
}

fn forget_guards(tested: &mut BTreeSet<String>, path: &str, inclusive: bool) {
    if tested.is_empty() {
        return;
    }
    if tested.len() == 1 {
        let only = tested.first().unwrap();
        if contains(path, only) && (inclusive || only != path) {
            tested.clear();
        }
        return;
    }
    if inclusive {
        tested.remove(path);
    }
    let prefix = format!("{path}/");
    let descendants: Vec<_> = tested
        .range(prefix.clone()..)
        .take_while(|candidate| candidate.starts_with(&prefix))
        .cloned()
        .collect();
    for descendant in descendants {
        tested.remove(&descendant);
    }
}

fn invalidate_guards(shadow: &Value, op: &PatchOperation, tested: &mut BTreeSet<String>) {
    if matches!(op, PatchOperation::Test(_)) || tested.is_empty() {
        return;
    }
    forget_guards(tested, path(op), true);
    if !matches!(op, PatchOperation::Replace(_)) {
        if let Some(parent) = op.path().parent() {
            if parent.resolve(shadow).is_ok_and(Value::is_array) {
                forget_guards(tested, parent.as_str(), false);
            }
        }
    }
    if let PatchOperation::Move(op) = op {
        forget_guards(tested, op.from.as_str(), true);
        if let Some(parent) = op.from.parent() {
            if parent.resolve(shadow).is_ok_and(Value::is_array) {
                forget_guards(tested, parent.as_str(), false);
            }
        }
    }
}

fn guard(mut shadow: Value, patch: &Patch) -> Result<Patch, Error> {
    let mut output = Vec::new();
    let mut tested = BTreeSet::new();
    for (index, op) in patch.0.iter().enumerate() {
        for (path, value) in unique_guard_values(&shadow, op, &mut tested)?
            .into_iter()
            .flatten()
        {
            // Keep the existing payload serialization, including Number's
            // normalization, without building and dismantling an outer map.
            let value = crate::export::copy_addition(value);
            let path = json_patch::jsonptr::PointerBuf::parse(path)
                .map_err(|error| Error::new("", error.to_string()))?;
            output.push(PatchOperation::Test(json_patch::TestOperation {
                path,
                value,
            }));
        }
        output.push(op.clone());
        invalidate_guards(&shadow, op, &mut tested);
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

    #[test]
    fn borrowed_resolution_keeps_value_pointer_results() {
        let value = json!({
            "": {"": 0}, "a/b~": {"🦀": [null, {"": 2}]},
            "array": [1, 2], "01": 3, "~2": 4,
        });
        for path in [
            "",
            "/",
            "//",
            "/a~1b~0",
            "/a~1b~0/🦀/1/",
            "/a~1b~0/🦀/0/x",
            "/array/0",
            "/array/1",
            "/array/01",
            "/array/+1",
            "/array/-",
            "/array/184467440737095516160",
            "/01",
            "/~02",
            "/missing",
        ] {
            assert_eq!(resolve(&value, path), value.pointer(path), "{path}");
        }
    }

    fn patch(value: Value) -> Patch {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn container_guards_have_linear_output_for_interleaved_insertions() {
        for count in [32, 512, 4096] {
            let left = json!({"a/~":[], "a-":[]});
            let mut original = Patch::default();
            for index in 0..count {
                for container in ["/a~1~0", "/a-"] {
                    original.0.push(
                        decode(json!({
                            "op":"add", "path":format!("{container}/-"),
                            "value":{"index":index,"text":"🦀"}
                        }))
                        .unwrap(),
                    );
                }
            }
            let guarded = guard(left.clone(), &original).unwrap();
            assert_eq!(guarded.0.len(), original.0.len() + 2);
            assert!(bytes(&guarded).unwrap() < count * 200 + 200);
            let mut actual = left.clone();
            json_patch::patch(&mut actual, &guarded).unwrap();
            assert_eq!(actual, apply_json_patch(&left, &original).unwrap());
            for key in ["a/~", "a-"] {
                let mut drift = left.clone();
                drift[key] = json!(["unexpected"]);
                assert!(json_patch::patch(&mut drift, &guarded).is_err());
            }
        }
    }

    #[test]
    fn container_certificates_do_not_follow_shifted_array_indices() {
        let left = json!({"a":[{"x":1},{"x":2},{"x":3}],"sink":0});
        let original = patch(json!([
            {"op":"copy","from":"/a/0","path":"/sink"},
            {"op":"remove","path":"/a/0"},
            {"op":"replace","path":"/a/0/x","value":4}
        ]));
        let guarded = guard(left.clone(), &original).unwrap();
        assert!(guarded.0.iter().any(|operation| matches!(operation,
            PatchOperation::Test(test) if test.path.as_str() == "/a/0/x" && test.value == json!(2)
        )));
        let mut drift = left.clone();
        drift["a"][1]["x"] = json!(99);
        assert!(json_patch::patch(&mut drift, &guarded).is_err());
        let (size, _) = guarded_bytes(&left, &original, true, &[]).unwrap();
        assert_eq!(size, bytes(&guarded).unwrap());
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases:256, rng_seed:proptest::test_runner::RngSeed::Fixed(20261006), ..Default::default() })]
        #[test]
        fn deduplicated_guards_keep_reference_drift_rejections(choices in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..32)) {
            let left = json!({"a":[{"x":1},{"x":2},{"x":3}],"sink":0,"a-":{"x":4},"other":[]});
            let mut shadow = left.clone();
            let mut original = Patch::default();
            let mut reference = Patch::default();
            for (step, choice) in choices.into_iter().enumerate() {
                let length = shadow["a"].as_array().unwrap().len();
                let index = usize::from(choice) % length.max(1);
                let value = json!({"x":step});
                let operation = match choice % 7 {
                    0 => json!({"op":"add","path":format!("/a/{index}"),"value":value}),
                    1 if length > 0 => json!({"op":"remove","path":format!("/a/{index}")}),
                    2 if length > 0 => json!({"op":"replace","path":format!("/a/{index}/x"),"value":step}),
                    3 if length > 0 => json!({"op":"copy","from":format!("/a/{index}"),"path":"/sink"}),
                    4 if length > 0 => json!({"op":"move","from":format!("/a/{index}"),"path":"/a/-"}),
                    5 => json!({"op":"add","path":"/other/-","value":value}),
                    _ => json!({"op":"add","path":format!("/a-/key{step}"),"value":value}),
                };
                let operation = decode(operation).unwrap();
                for (path, value) in guard_values(&shadow, &operation).unwrap().into_iter().flatten() {
                    reference.0.push(decode(json!({"op":"test","path":path,"value":value})).unwrap());
                }
                reference.0.push(operation.clone());
                crate::apply_json_patch_step(&mut shadow, &operation, step).unwrap();
                original.0.push(operation);
            }
            let guarded = guard(left.clone(), &original).unwrap();
            proptest::prop_assert_eq!(apply_json_patch(&left, &guarded).unwrap(), shadow);
            let (size, _) = guarded_bytes(&left, &original, true, &[]).unwrap();
            proptest::prop_assert_eq!(size, bytes(&guarded).unwrap());
            for path in ["/a/0/x", "/a/1/x", "/a/2/x", "/a-/x", "/sink", "/other"] {
                let mut drift = left.clone();
                *drift.pointer_mut(path).unwrap() = json!(99);
                if apply_json_patch(&drift, &reference).is_err() {
                    proptest::prop_assert!(apply_json_patch(&drift, &guarded).is_err(), "lost guard at {}", path);
                }
            }
        }
    }

    #[test]
    fn array_insertion_costs_match_every_serialized_prefix_and_resumed_replay() {
        for container in ["", "/a~1~0🦀", "/items/0"] {
            for initial in [json!([]), json!([null, "🦀"])] {
                let left = match container {
                    "" => initial,
                    "/items/0" => json!({"items":[initial]}),
                    _ => json!({"a/~🦀":initial}),
                };
                let values: Value = serde_json::from_str(if cfg!(feature = "exact-numbers") {
                    r#"[1.0,1e9999,123456789012345678901234567890,{"escaped":"\n\"🦀"},[true,null]]"#
                } else {
                    r#"[1.0,1e20,1234567890,{"escaped":"\n\"🦀"},[true,null]]"#
                }).unwrap();
                let original = Patch(values.as_array().unwrap().iter().enumerate().map(|(index,value)| {
                    decode(json!({"op":"add","path":pointer(container,if index%2==0 {"0"} else {"-"}),"value":value})).unwrap()
                }).collect());
                let actual = guard(left.clone(), &original).unwrap();
                let (size, costs, shadow) =
                    guarded_operation_bytes(&left, original.0.iter(), original.0.len(), &[])
                        .unwrap();
                assert_eq!(size, bytes(&actual).unwrap());
                assert_eq!(shadow, apply_json_patch(&left, &original).unwrap());
                for end in 0..=original.0.len() {
                    let prefix = guard(left.clone(), &Patch(original.0[..end].to_vec())).unwrap();
                    assert_eq!(costs[end].bytes, bytes(&prefix).unwrap());
                    assert_eq!(costs[end].operations, prefix.0.len());
                    assert_eq!(
                        guarded_bytes(&left, &original, true, &costs[..=end])
                            .unwrap()
                            .0,
                        size
                    );
                }
            }
        }
    }

    #[test]
    fn array_insertion_costs_reset_on_other_writes_and_keep_errors() {
        let left = json!({"a":[],"b":[1],"o":{"x":true,"y":false},"source":{"nested":"🦀"}});
        let original = patch(json!([
            {"op":"add","path":"/a/-","value":1},
            {"op":"add","path":"/a/0","value":2},
            {"op":"add","path":"/b/-","value":3},
            {"op":"add","path":"/a/-","value":4},
            {"op":"replace","path":"/a/0","value":"changed"},
            {"op":"add","path":"/a/-","value":5},
            {"op":"remove","path":"/a/1"},
            {"op":"add","path":"/a/-","value":6},
            {"op":"copy","from":"/source","path":"/a/-"},
            {"op":"add","path":"/a/-","value":7},
            {"op":"move","from":"/a/0","path":"/a/-"},
            {"op":"add","path":"/a/-","value":8},
            {"op":"move","from":"/o/x","path":"/o/longer-name"},
            {"op":"add","path":"/a/-","value":9},
            {"op":"test","path":"/b","value":[1,3]},
            {"op":"add","path":"/a/-","value":10}
        ]));
        let (size, costs) = guarded_bytes(&left, &original, true, &[]).unwrap();
        assert_eq!(
            size,
            bytes(&guard(left.clone(), &original).unwrap()).unwrap()
        );
        for end in 0..=original.0.len() {
            assert_eq!(
                guarded_bytes(&left, &original, true, &costs[..=end])
                    .unwrap()
                    .0,
                size
            );
        }
        for invalid in [
            json!({"op":"add","path":"/a/999","value":0}),
            json!({"op":"add","path":"/missing/-","value":0}),
            json!({"op":"add","path":"/a/00","value":0}),
        ] {
            let mut invalid_patch = original.clone();
            invalid_patch.0.push(decode(invalid).unwrap());
            assert_eq!(
                guarded_bytes(&left, &invalid_patch, true, &[])
                    .err()
                    .unwrap(),
                guard(left.clone(), &invalid_patch).unwrap_err()
            );
        }
        // Adding at the root replaces the document, including its type. It
        // must not reuse the old array's insertion cost for subsequent edits.
        let left = json!([1, 2, 3]);
        let root_writes = patch(json!([
            {"op":"add","path":"","value":["replacement"]},
            {"op":"add","path":"/-","value":"next"},
            {"op":"add","path":"/0","value":null},
            {"op":"add","path":"","value":{"a":1,"b":2}},
            {"op":"move","from":"/a","path":"/longer-a"},
            {"op":"move","from":"/b","path":"/longer-b"}
        ]));
        let (size, costs) = guarded_bytes(&left, &root_writes, true, &[]).unwrap();
        assert_eq!(
            size,
            bytes(&guard(left.clone(), &root_writes).unwrap()).unwrap()
        );
        for end in 0..=root_writes.0.len() {
            assert_eq!(
                guarded_bytes(&left, &root_writes, true, &costs[..=end])
                    .unwrap()
                    .0,
                size
            );
        }
    }

    #[test]
    #[cfg(feature = "exact-numbers")]
    fn guard_payloads_keep_existing_number_normalization_and_escaped_paths() {
        for (raw, expected) in [
            ("-0", "0"),
            ("1E+0003", "1e+0003"),
            ("1.00e00", "1.00e+00"),
            ("1.0", "1.0"),
            ("1e9999", "1e+9999"),
            (
                "1234567890123456789012345678901234567890",
                "1234567890123456789012345678901234567890",
            ),
        ] {
            let mut fields = serde_json::Map::new();
            fields.insert(
                "a/~🦀".into(),
                Value::Number(serde_json::Number::from_string_unchecked(raw.into())),
            );
            let left = Value::Object(fields);
            let original = patch(json!([{"op":"remove","path":"/a~1~0🦀"}]));
            let actual = guard(left.clone(), &original).unwrap();
            let expected = format!(
                r#"[{{"op":"test","path":"/a~1~0🦀","value":{expected}}},{{"op":"remove","path":"/a~1~0🦀"}}]"#
            );
            assert_eq!(serde_json::to_string(&actual).unwrap(), expected);
            assert_eq!(apply_json_patch(&left, &actual).unwrap(), json!({}));
        }
    }

    #[test]
    fn guard_construction_keeps_missing_sources_and_application_error_order() {
        let left = json!({"o":{"a":1},"array":[0]});
        for (operations, path, message) in [
            (
                json!([
                    {"op":"replace","path":"/o/a","value":2},
                    {"op":"move","from":"/missing","path":"/also-missing/x"}
                ]),
                "/missing",
                "test source is missing",
            ),
            (
                json!([
                    {"op":"replace","path":"/o/a","value":2},
                    {"op":"add","path":"/missing/x","value":1}
                ]),
                "/missing",
                "addition parent is missing",
            ),
        ] {
            let error = guard(left.clone(), &patch(operations)).unwrap_err();
            assert_eq!(error, Error::new(path, message));
        }
        for operation in [
            json!({"op":"add","path":"/array/9","value":1}),
            json!({"op":"test","path":"/o/a","value":99}),
        ] {
            let operations = patch(json!([
                {"op":"replace","path":"/o/a","value":2},
                operation
            ]));
            assert_eq!(
                guard(left.clone(), &operations).unwrap_err(),
                apply_json_patch(&left, &operations).unwrap_err()
            );
        }
        assert_eq!(left, json!({"o":{"a":1},"array":[0]}));
    }

    #[test]
    fn mixed_slots_parent_snapshots_preserve_chronology_and_number_bytes() {
        let left = json!({
            "0accepted":{"leaf":{"old_a_long_property_name":0,"old_b_long_property_name":0}},
            "a":{"old_a_long_property_name":0,"old_b_long_property_name":0},
            "keep":"unchanged 🦀".repeat(500),
        });
        let changes = json!([
            {"op":"remove","path":"/0accepted/leaf/old_a_long_property_name"},
            {"op":"add","path":"/0accepted/leaf/new_a_long_property_name","value":1},
            {"op":"remove","path":"/0accepted/leaf/old_b_long_property_name"},
            {"op":"add","path":"/0accepted/leaf/new_b_long_property_name","value":2},
            {"op":"remove","path":"/a/old_a_long_property_name"},
            {"op":"add","path":"/a/new_a_long_property_name","value":11111},
            {"op":"remove","path":"/a/old_b_long_property_name"},
            {"op":"add","path":"/a/new_b_long_property_name","value":22222},
        ]);
        // Baseline 0.1.2 wire: root Add before, between, and after interior edits.
        for insertion in [0, 6, 8] {
            let mut original = Patch(
                changes
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned()
                    .map(|v| decode(v).unwrap())
                    .collect(),
            );
            let tail = decode(json!({"op":"add","path":"/tail","value":0})).unwrap();
            original.0.insert(insertion, tail.clone());
            let right = apply_json_patch(&left, &original).unwrap();
            let mut expected = Patch(vec![
                decode(json!({"op":"replace","path":"/0accepted/leaf","value":{"new_a_long_property_name":1,"new_b_long_property_name":2}})).unwrap(),
                decode(json!({"op":"replace","path":"/a","value":{"new_a_long_property_name":11111,"new_b_long_property_name":22222}})).unwrap(),
            ]);
            expected.0.insert(if insertion == 0 { 0 } else { 2 }, tail);
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
                assert!(json_equal(
                    &apply_json_patch(&left, &output).unwrap(),
                    &right
                ));
                let inverse = crate::invert_json_patch(&left, &output).unwrap();
                assert!(json_equal(
                    &apply_json_patch(&right, &inverse).unwrap(),
                    &left
                ));
            }
        }

        // A one-byte acceptance margin: the external Add sees 1.0 originally,
        // but the replacement comes from target 1. Math equality cannot price it.
        let left = json!({"a":{"old":0,"u":"x".repeat(52)},"keep":"unchanged 🦀".repeat(500)});
        let original = Patch(vec![
            decode(json!({"op":"remove","path":"/a/old"})).unwrap(),
            PatchOperation::Add(json_patch::AddOperation {
                path: json_patch::jsonptr::PointerBuf::parse("/a/new").unwrap(),
                value: serde_json::from_str("1.0").unwrap(),
            }),
            decode(json!({"op":"add","path":"/tail","value":0})).unwrap(),
        ]);
        let original_final = apply_json_patch(&left, &original).unwrap();
        let mut right = original_final.clone();
        right["a"]["new"] = json!(1);
        assert!(json_equal(&original_final, &right));
        assert_ne!(original_final, right);
        let expected = Patch(vec![
            decode(json!({"op":"replace","path":"/a","value":right["a"]})).unwrap(),
            original.0[2].clone(),
        ]);
        assert_eq!(
            serde_json::to_vec(&guard(left.clone(), &original).unwrap())
                .unwrap()
                .len(),
            7366
        );
        assert_eq!(
            serde_json::to_vec(&guard(left.clone(), &expected).unwrap())
                .unwrap()
                .len(),
            7365
        );
        let optimized = rationalize(&left, &right, original, true).unwrap();
        assert_eq!(
            serde_json::to_vec(&optimized).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        let output = guard(left.clone(), &optimized).unwrap();
        assert!(json_equal(
            &apply_json_patch(&left, &output).unwrap(),
            &right
        ));
        let inverse = crate::invert_json_patch(&left, &output).unwrap();
        assert!(json_equal(
            &apply_json_patch(&right, &inverse).unwrap(),
            &left
        ));
    }

    #[test]
    fn independent_object_moves_keep_duplicate_sources_and_original_add_order() {
        let payload = json!({"body":"repeated Unicode 🦀 ".repeat(12)});
        let left = json!({
            "group/~":{"0":payload,"old":payload},
            "other":{"old":{"unique":true}},
            "destination":{},"keep":0,
        });
        let original = Patch(vec![
            decode(json!({"op":"add","path":"/destination/first~1~0","value":payload})).unwrap(),
            decode(json!({"op":"remove","path":"/group~1~0/old"})).unwrap(),
            decode(json!({"op":"replace","path":"/keep","value":1})).unwrap(),
            decode(json!({"op":"add","path":"/destination/second","value":payload})).unwrap(),
            decode(json!({"op":"remove","path":"/group~1~0/0"})).unwrap(),
            decode(json!({"op":"remove","path":"/other/old"})).unwrap(),
            decode(json!({"op":"add","path":"/other/new","value":{"unique":true}})).unwrap(),
        ]);
        let right = apply_json_patch(&left, &original).unwrap();
        // These are the sequential factorizer's original source/operation order.
        let expected = json!([
            {"op":"move","from":"/group~1~0/old","path":"/destination/first~1~0"},
            {"op":"replace","path":"/keep","value":1},
            {"op":"move","from":"/group~1~0/0","path":"/destination/second"},
            {"op":"move","from":"/other/old","path":"/other/new"},
        ]);
        let optimized = factorize(&left, &right, original).unwrap();
        assert_eq!(serde_json::to_value(&optimized).unwrap(), expected);
        for tests in [false, true] {
            let output = if tests {
                guard(left.clone(), &optimized).unwrap()
            } else {
                optimized.clone()
            };
            let mut applied = left.clone();
            json_patch::patch(&mut applied, &output).unwrap();
            assert_eq!(applied, right);
            let inverse = crate::invert_json_patch(&left, &output).unwrap();
            json_patch::patch(&mut applied, &inverse).unwrap();
            assert_eq!(applied, left);
        }
    }

    #[test]
    fn ancestor_removal_keeps_late_child_moves_on_sequential_replay() {
        let child = "removed child 🦀 ".repeat(20);
        let sibling = "other child ".repeat(20);
        let third = "independent value ".repeat(20);
        let left = json!({"a":{"x":child,"y":sibling},"a-":{"z":third},"dest":{}});
        let original = Patch(vec![
            decode(json!({"op":"remove","path":"/a/x"})).unwrap(),
            decode(json!({"op":"remove","path":"/a"})).unwrap(),
            decode(json!({"op":"add","path":"/dest/parent","value":{"y":sibling}})).unwrap(),
            decode(json!({"op":"add","path":"/dest/child","value":child})).unwrap(),
            decode(json!({"op":"remove","path":"/a-/z"})).unwrap(),
            decode(json!({"op":"add","path":"/dest/third","value":third})).unwrap(),
        ]);
        let right = apply_json_patch(&left, &original).unwrap();
        let optimized = factorize(&left, &right, original).unwrap();
        // Moving /a/x at its later Add would read a source already removed by
        // the ancestor operation. /a- must not hide that dependency.
        let expected = json!([
            {"op":"remove","path":"/a/x"},
            {"op":"move","from":"/a","path":"/dest/parent"},
            {"op":"add","path":"/dest/child","value":child},
            {"op":"move","from":"/a-/z","path":"/dest/third"},
        ]);
        assert_eq!(serde_json::to_value(&optimized).unwrap(), expected);
        for tests in [false, true] {
            let output = if tests {
                guard(left.clone(), &optimized).unwrap()
            } else {
                optimized.clone()
            };
            let mut applied = left.clone();
            json_patch::patch(&mut applied, &output).unwrap();
            assert_eq!(applied, right);
            let inverse = crate::invert_json_patch(&left, &output).unwrap();
            json_patch::patch(&mut applied, &inverse).unwrap();
            assert_eq!(applied, left);
        }
    }

    #[test]
    fn object_move_preflight_preserves_initial_error_after_valid_pairs() {
        let left = json!({"a":{"x":1,"y":2},"b":{}});
        let original = Patch(vec![
            decode(json!({"op":"remove","path":"/a/x"})).unwrap(),
            decode(json!({"op":"add","path":"/b/x","value":1})).unwrap(),
            decode(json!({"op":"remove","path":"/a/y"})).unwrap(),
            decode(json!({"op":"add","path":"/b/y","value":2})).unwrap(),
            decode(json!({"op":"remove","path":"/missing"})).unwrap(),
        ]);
        let error = factorize(&left, &json!({"a":{},"b":{"x":1,"y":2}}), original).unwrap_err();
        assert_eq!(error.path, "/missing");
        assert_eq!(
            error.message,
            "operation '/4' failed at path '/missing': path is invalid"
        );
    }

    #[test]
    fn factorized_move_candidates_preserve_dense_indices_and_initial_errors() {
        for accepted in [false, true] {
            let left = if accepted {
                json!({"a":["a","b","b"]})
            } else {
                json!({"a":["a","b","c"]})
            };
            let operations = if accepted {
                json!([{"op":"add","path":"/a/0","value":"b"},{"op":"remove","path":"/a/2"}])
            } else {
                json!([{"op":"remove","path":"/a/0"},{"op":"remove","path":"/a/0"},{"op":"add","path":"/a/1","value":"a"}])
            };
            let original = Patch(
                operations
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned()
                    .map(|v| decode(v).unwrap())
                    .collect(),
            );
            let right = apply_json_patch(&left, &original).unwrap();
            let expected = if accepted {
                Patch(vec![
                    decode(json!({"op":"move","from":"/a/2","path":"/a/0"})).unwrap(),
                ])
            } else {
                original.clone()
            };
            let optimized = factorize(&left, &right, original).unwrap();
            assert_eq!(
                serde_json::to_vec(&optimized).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            for tests in [false, true] {
                let output = if tests {
                    guard(left.clone(), &optimized).unwrap()
                } else {
                    optimized.clone()
                };
                assert!(json_equal(
                    &apply_json_patch(&left, &output).unwrap(),
                    &right
                ));
                let inverse = crate::invert_json_patch(&left, &output).unwrap();
                assert!(json_equal(
                    &apply_json_patch(&right, &inverse).unwrap(),
                    &left
                ));
            }
        }
        let left = json!({"a":0});
        let original = Patch(vec![
            decode(json!({"op":"remove","path":"/missing"})).unwrap(),
            decode(json!({"op":"add","path":"/new","value":1})).unwrap(),
        ]);
        let error = factorize(&left, &json!({"a":0,"new":1}), original).unwrap_err();
        assert_eq!(error.path, "/missing");
        assert_eq!(
            error.message,
            "operation '/0' failed at path '/missing': path is invalid"
        );
    }

    #[test]
    fn stable_replacement_slots_preserve_interleaved_children_and_repeated_paths() {
        for nested in [false, true] {
            let left = if nested {
                json!({
                    "a": {"leaf": {"a_long_property_name": 0, "b_long_property_name": 0}},
                    "keep": "unchanged 🦀".repeat(500),
                })
            } else {
                json!({
                    "a": {
                        "left": {"a_long_property_name": 0, "b_long_property_name": 0},
                        "right": {"a_long_property_name": 0, "b_long_property_name": 0},
                    },
                    "keep": "unchanged 🦀".repeat(500),
                })
            };
            let original = if nested {
                patch(json!([
                    {"op":"replace","path":"/a","value":{"leaf":{"a_long_property_name":1,"b_long_property_name":1}}},
                    {"op":"replace","path":"/a/leaf/a_long_property_name","value":2},
                    {"op":"replace","path":"/a/leaf/a_long_property_name","value":3},
                    {"op":"replace","path":"/a/leaf/b_long_property_name","value":4},
                ]))
            } else {
                patch(json!([
                    {"op":"replace","path":"/a/left/a_long_property_name","value":1},
                    {"op":"replace","path":"/a/right/a_long_property_name","value":3},
                    {"op":"replace","path":"/a/left/b_long_property_name","value":2},
                    {"op":"replace","path":"/a/right/b_long_property_name","value":4},
                ]))
            };
            let right = apply_json_patch(&left, &original).unwrap();
            // Complete wire frozen from the published 0.1.1 implementation.
            // Interleaved siblings first compress, then their common ancestor;
            // nested original writes retain their intermediate guard costs.
            let expected = patch(json!([
                {"op":"replace","path":"/a","value":right["a"]},
            ]));
            for tests in [false, true] {
                let optimized = rationalize(&left, &right, original.clone(), tests).unwrap();
                assert_eq!(
                    serde_json::to_vec(&optimized).unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
                let guarded = if tests {
                    guard(left.clone(), &optimized).unwrap()
                } else {
                    optimized
                };
                assert_eq!(apply_json_patch(&left, &guarded).unwrap(), right);
                let inverse = invert_json_patch(&left, &guarded).unwrap();
                assert_eq!(apply_json_patch(&right, &inverse).unwrap(), left);
            }
        }
    }

    #[test]
    fn invalid_original_replacements_keep_plain_replay_and_guard_errors() {
        let left = json!({"a":{"x":0},"keep":"unchanged 🦀".repeat(500)});
        let right = json!({"a":{"x":1,"missing":2},"keep":"unchanged 🦀".repeat(500)});
        let original = patch(json!([
            {"op":"replace","path":"/a/x","value":1},
            {"op":"replace","path":"/a/missing","value":2},
        ]));
        // Frozen 0.1.1 behavior: the invalid original is not an error gate for
        // plain compression, since the full parent candidate repairs it.
        let expected = patch(json!([
            {"op":"replace","path":"/a","value":{"missing":2,"x":1}},
        ]));
        let optimized = rationalize(&left, &right, original.clone(), false).unwrap();
        assert_eq!(
            serde_json::to_vec(&optimized).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        assert_eq!(apply_json_patch(&left, &optimized).unwrap(), right);
        let inverse = invert_json_patch(&left, &optimized).unwrap();
        assert_eq!(apply_json_patch(&right, &inverse).unwrap(), left);
        let error = rationalize(&left, &right, original, true).unwrap_err();
        assert_eq!(error.path, "/a/missing");
        assert_eq!(error.message, "test source is missing");
    }

    #[test]
    fn pointer_tokens_preserve_parent_escapes_and_unicode() {
        for (parent, token, expected) in [
            ("", "", "/"),
            ("", "a/b~c", "/a~1b~0c"),
            ("/~0~1🦀", "~/🦀", "/~0~1🦀/~0~1🦀"),
            ("/parent", "~1", "/parent/~01"),
            ("/parent", "a~0b", "/parent/a~00b"),
        ] {
            assert_eq!(pointer(parent, token), expected);
        }
        assert_eq!(pointer("/🦀", 12), "/🦀/12");
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
                let expected = if tests && kind == 1 {
                    // The root Add now certifies the second leaf too, making
                    // the original interleaved operations smaller than /a.
                    let mut expected = Patch(vec![expected.0[0].clone()]);
                    expected.0.extend(original.0[2..].iter().cloned());
                    expected
                } else {
                    expected.clone()
                };
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
    fn object_rename_snapshot_costs_match_serialized_prefixes_and_resumed_replay() {
        let payload: Value = serde_json::from_str(if cfg!(feature = "exact-numbers") {
            r#"{"float":1.0,"integer":1234567890123456789012345678901234567890,"exponent":1e9999}"#
        } else {
            r#"{"float":1.0,"integer":1234567890,"exponent":1e20}"#
        })
        .unwrap();
        let object = json!({"":payload,"a/~\"\\\n":payload,"~1":[payload,"🦀"]});
        for container in ["", "/outer~1~0", "/items/0"] {
            let left = match container {
                "" => object.clone(),
                "/items/0" => json!({"items":[object]}),
                _ => json!({"outer/~":object}),
            };
            let original = Patch(
                [("a/~\"\\\n", "短"), ("", "目标~/🤖\0"), ("~1", "~01")]
                    .into_iter()
                    .map(|(source, destination)| {
                        decode(json!({"op":"move","from":pointer(container,source),
                            "path":pointer(container,destination)}))
                        .unwrap()
                    })
                    .collect(),
            );
            let actual = guard(left.clone(), &original).unwrap();
            let (size, costs, shadow) =
                guarded_operation_bytes(&left, original.0.iter(), original.0.len(), &[]).unwrap();
            assert_eq!(size, bytes(&actual).unwrap());
            let mut applied = left.clone();
            json_patch::patch(&mut applied, &actual).unwrap();
            assert_eq!(shadow, applied);
            for end in 0..=original.0.len() {
                let prefix = guard(left.clone(), &Patch(original.0[..end].to_vec())).unwrap();
                assert_eq!(costs[end].bytes, bytes(&prefix).unwrap());
                assert_eq!(costs[end].operations, prefix.0.len());
                assert_eq!(
                    guarded_bytes(&left, &original, true, &costs[..=end])
                        .unwrap()
                        .0,
                    size
                );
            }
            let inverse = crate::invert_json_patch(&left, &actual).unwrap();
            json_patch::patch(&mut applied, &inverse).unwrap();
            assert_eq!(applied, left);
        }
    }

    #[test]
    fn object_rename_snapshot_costs_reset_on_other_writes_and_preserve_errors() {
        let left = json!({"o":{"a":{"body":"old"},"b":2,"c":3,"d":4,
            "e":5,"f":6,"g":7,"overwrite":{"large":"🦀".repeat(50)}},
            "arr":[1,2],"source":"copy"});
        let original = patch(json!([
            {"op":"move","from":"/o/a","path":"/o/new-a"},
            {"op":"move","from":"/o/b","path":"/o/overwrite"},
            {"op":"move","from":"/o/c","path":"/o/new-c"},
            {"op":"replace","path":"/o/new-a/body","value":"changed 🦀"},
            {"op":"move","from":"/o/d","path":"/o/new-d"},
            {"op":"copy","from":"/source","path":"/o/copied"},
            {"op":"move","from":"/o/e","path":"/o/new-e"},
            {"op":"move","from":"/arr/0","path":"/arr/1"},
            {"op":"move","from":"/o/f","path":"/o/new-f"},
            {"op":"add","path":"/o/added","value":true},
            {"op":"remove","path":"/o/copied"},
            {"op":"test","path":"/source","value":"copy"},
            {"op":"move","from":"/o/g","path":"/o/new-g"},
            {"op":"move","from":"/o/new-g","path":"/outside"},
            {"op":"move","from":"/o/new-f","path":"/o/final-f"}
        ]));
        let (size, costs) = guarded_bytes(&left, &original, true, &[]).unwrap();
        assert_eq!(
            size,
            bytes(&guard(left.clone(), &original).unwrap()).unwrap()
        );
        for end in 0..=original.0.len() {
            let prefix = guard(left.clone(), &Patch(original.0[..end].to_vec())).unwrap();
            assert_eq!(costs[end].bytes, bytes(&prefix).unwrap());
            assert_eq!(costs[end].operations, prefix.0.len());
            assert_eq!(
                guarded_bytes(&left, &original, true, &costs[..=end])
                    .unwrap()
                    .0,
                size
            );
        }
        let mut invalid = original;
        invalid
            .0
            .push(decode(json!({"op":"move","from":"/o/missing","path":"/o/end"})).unwrap());
        assert_eq!(
            guarded_bytes(&left, &invalid, true, &[]).err(),
            guard(left, &invalid).err()
        );
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
