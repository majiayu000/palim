use crate::{ArrayItemMatcher, DiffOptions, Error, pointer, text};
use imara_diff::{Algorithm, Diff, InternedInput, Interner, Token};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

#[derive(Hash, PartialEq, Eq)]
enum Key<'a> {
    Identity(String),
    Position(usize),
    Value(&'a Value),
}

pub(crate) fn node(
    left: &Value,
    right: &Value,
    options: &DiffOptions,
    path: &str,
) -> Result<Option<Value>, Error> {
    if options
        .node_filter
        .as_ref()
        .is_some_and(|filter| !filter(path, Some(left), Some(right)))
    {
        return Ok(None);
    }
    node_unfiltered(left, right, options, path)
}

// Array item roots have already passed the original-position projection filter.
// Nested properties and nested array positions still go through their filters.
fn node_unfiltered(
    left: &Value,
    right: &Value,
    options: &DiffOptions,
    path: &str,
) -> Result<Option<Value>, Error> {
    if left == right {
        return Ok(None);
    }
    let result = match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            let mut delta = Map::new();
            for (key, old) in a {
                if options
                    .property_filter
                    .as_ref()
                    .is_some_and(|f| !f(key, left, right, path))
                {
                    continue;
                }
                match b.get(key) {
                    Some(new) => {
                        // Equal primitive fields need no path allocation. A node
                        // filter still receives unchanged nodes in its usual order.
                        if options.node_filter.is_none()
                            && matches!(old, Value::Number(_) | Value::Bool(_) | Value::Null)
                            && old == new
                        {
                            continue;
                        }
                        if let Some(child) = node(old, new, options, &pointer(path, key))? {
                            delta.insert(key.clone(), child);
                        }
                    }
                    None => {
                        let child_path = pointer(path, key);
                        if options.node_filter.is_none() {
                            delta.insert(key.clone(), json!([old, 0, 0]));
                        } else if let Some(retained) =
                            filtered_child(Some(old), None, options, &child_path)
                        {
                            // The retained subtree has already been filtered. Diff it
                            // without calling the same node predicates a second time.
                            let mut projected_options = options.clone();
                            projected_options.node_filter = None;
                            if let Some(child) =
                                node_unfiltered(old, &retained, &projected_options, &child_path)?
                            {
                                delta.insert(key.clone(), child);
                            }
                        } else {
                            delta.insert(key.clone(), json!([old, 0, 0]));
                        }
                    }
                }
            }
            for (key, new) in b {
                if a.contains_key(key) {
                    continue;
                }
                if options
                    .property_filter
                    .as_ref()
                    .is_some_and(|f| !f(key, left, right, path))
                {
                    continue;
                }
                if options.node_filter.is_some() {
                    if let Some(projected) =
                        filtered_child(None, Some(new), options, &pointer(path, key))
                    {
                        delta.insert(key.clone(), json!([projected]));
                    }
                } else {
                    delta.insert(key.clone(), json!([new]));
                }
            }
            if delta.is_empty() {
                return Ok(None);
            }
            Value::Object(delta)
        }
        (Value::Array(a), Value::Array(b)) => return array(a, b, options, path),
        (Value::String(a), Value::String(b))
            if options.text_diff_min_length.is_some_and(|min| {
                a.encode_utf16().take(min).count() == min
                    && b.encode_utf16().take(min).count() == min
            }) =>
        {
            json!([text::diff(a, b, path)?, 0, 2])
        }
        _ if options.node_filter.is_some() && (right.is_object() || right.is_array()) => {
            json!([left, project_present(Some(left), right, options, path)])
        }
        _ => json!([left, right]),
    };
    Ok(Some(result))
}

// Missing values stay missing. In particular, excluding an added child never
// manufactures JSON null. A rejected node retains its entire source subtree.
fn filtered_child(
    left: Option<&Value>,
    right: Option<&Value>,
    options: &DiffOptions,
    path: &str,
) -> Option<Value> {
    if options
        .node_filter
        .as_ref()
        .is_some_and(|filter| !filter(path, left, right))
    {
        return left.cloned();
    }
    match right {
        Some(value) => Some(project_present(left, value, options, path)),
        None => left.and_then(|value| project_removed(value, options, path)),
    }
}

// The parent has been allowed already. Target containers recursively project
// children; incompatible source types provide no source child. Scalar targets
// are atomic replacements, so their former descendants are not consulted.
fn project_present(
    left: Option<&Value>,
    right: &Value,
    options: &DiffOptions,
    path: &str,
) -> Value {
    match right {
        Value::Object(target) => {
            let source = left.and_then(Value::as_object);
            let mut result = Map::new();
            for (key, new) in target {
                if let Some(value) = filtered_child(
                    source.and_then(|map| map.get(key)),
                    Some(new),
                    options,
                    &pointer(path, key),
                ) {
                    result.insert(key.clone(), value);
                }
            }
            if let Some(source) = source {
                for (key, old) in source {
                    if !target.contains_key(key) {
                        if let Some(value) =
                            filtered_child(Some(old), None, options, &pointer(path, key))
                        {
                            result.insert(key.clone(), value);
                        }
                    }
                }
            }
            Value::Object(result)
        }
        Value::Array(target) => {
            let source = left
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            Value::Array(
                (0..source.len().max(target.len()))
                    .filter_map(|index| {
                        filtered_child(
                            source.get(index),
                            target.get(index),
                            options,
                            &pointer(path, index),
                        )
                    })
                    .collect(),
            )
        }
        _ => right.clone(),
    }
}

// An allowed deletion can still retain excluded descendants. Keep their ancestor
// containers only when at least one descendant survives; empty allowed containers
// disappear. Arrays compact retained children instead of inserting null padding.
fn project_removed(left: &Value, options: &DiffOptions, path: &str) -> Option<Value> {
    match left {
        Value::Object(source) => {
            let result: Map<_, _> = source
                .iter()
                .filter_map(|(key, old)| {
                    filtered_child(Some(old), None, options, &pointer(path, key))
                        .map(|value| (key.clone(), value))
                })
                .collect();
            (!result.is_empty()).then_some(Value::Object(result))
        }
        Value::Array(source) => {
            let result: Vec<_> = source
                .iter()
                .enumerate()
                .filter_map(|(index, old)| {
                    filtered_child(Some(old), None, options, &pointer(path, index))
                })
                .collect();
            (!result.is_empty()).then_some(Value::Array(result))
        }
        _ => None,
    }
}

fn key<'a>(value: &'a Value, index: usize, options: &DiffOptions) -> Key<'a> {
    if value.is_object() || value.is_array() {
        if let Some(hash) = &options.object_hash {
            if let Some(id) = hash(value, index) {
                return Key::Identity(id);
            }
        } else if options.match_by_position {
            return Key::Position(index);
        }
    }
    Key::Value(value)
}

// Unique tokens reduce LCS to LIS over source indices without expanding matches.
fn unique_matches<T>(input: &InternedInput<T>) -> Option<Vec<(usize, usize)>> {
    let token_count = input.interner.num_tokens() as usize;
    let mut source = vec![None; token_count];
    for (old, token) in input.before.iter().enumerate() {
        if source[token_index(*token)].replace(old).is_some() {
            return None;
        }
    }
    let mut seen = vec![false; token_count];
    let mut pairs = Vec::new();
    for (new, token) in input.after.iter().enumerate() {
        let index = token_index(*token);
        if std::mem::replace(&mut seen[index], true) {
            return None;
        }
        if let Some(old) = source[index] {
            pairs.push((old, new));
        }
    }
    Some(increasing_matches(&pairs))
}

// Repeated tokens use the exact Hunt/McIlroy candidate-chain construction when
// the remaining match pairs fit a fixed budget. Descending source positions in
// each target group prevent the LIS from matching one target item twice. Dense
// ambiguous inputs exceed the budget and retain Histogram's bounded fallback.
fn bounded_matches<T>(input: &InternedInput<T>) -> Option<Vec<(usize, usize)>> {
    const MAX_MATCH_PAIRS: usize = 16_384;
    let before = &input.before;
    let after = &input.after;
    let shared_len = before.len().min(after.len());
    let prefix = before
        .iter()
        .zip(after)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(shared_len - prefix)
        .take_while(|(old, new)| old == new)
        .count();
    let mut occurrences = vec![Vec::new(); input.interner.num_tokens() as usize];
    for (offset, token) in before[prefix..before.len() - suffix].iter().enumerate() {
        occurrences[token_index(*token)].push(prefix + offset);
    }
    let mut pair_count = 0;
    for token in &after[prefix..after.len() - suffix] {
        pair_count += occurrences[token_index(*token)].len();
        if pair_count > MAX_MATCH_PAIRS {
            return None;
        }
    }
    let mut pairs = Vec::with_capacity(pair_count);
    for (offset, token) in after[prefix..after.len() - suffix].iter().enumerate() {
        for &old in occurrences[token_index(*token)].iter().rev() {
            pairs.push((old, prefix + offset));
        }
    }
    let mut matches = Vec::with_capacity(prefix + suffix + pairs.len().min(shared_len));
    matches.extend((0..prefix).map(|index| (index, index)));
    matches.extend(increasing_matches(&pairs));
    matches.extend((0..suffix).map(|offset| {
        (
            before.len() - suffix + offset,
            after.len() - suffix + offset,
        )
    }));
    Some(matches)
}

fn increasing_matches(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; pairs.len()];
    for (index, &(old, _)) in pairs.iter().enumerate() {
        let length = tails.partition_point(|&tail| pairs[tail].0 < old);
        if length != 0 {
            previous[index] = Some(tails[length - 1]);
        }
        if length == tails.len() {
            tails.push(index);
        } else {
            tails[length] = index;
        }
    }
    let mut matches = Vec::with_capacity(tails.len());
    let mut current = tails.last().copied();
    while let Some(index) = current {
        matches.push(pairs[index]);
        current = previous[index];
    }
    matches.reverse();
    matches
}

fn token_index(token: Token) -> usize {
    u32::from(token) as usize
}

// Evaluate each candidate once, then find a deterministic maximum one-to-one
// matching. An iterative augmenting-path search avoids recursion for large arrays.
fn custom_matches(
    a: &[Value],
    b: &[Value],
    matcher: &ArrayItemMatcher,
    path: &str,
) -> Vec<(usize, usize)> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let candidates: Vec<Vec<_>> = b
        .iter()
        .map(|new| {
            a.iter()
                .enumerate()
                .filter_map(|(old, value)| matcher(path, value, new).then_some(old))
                .collect()
        })
        .collect();
    let mut old_for_new = vec![None; b.len()];
    let mut new_for_old = vec![None; a.len()];
    let mut visited_old = vec![0; a.len()];
    let mut visited_new = vec![0; b.len()];
    let mut previous = vec![None; b.len()];
    let mut queue = VecDeque::new();
    for new in 0..b.len() {
        // Array capacity was checked before matching, so new + 1 cannot
        // overflow. Reuse scratch without clearing both arrays for each target.
        let generation = new + 1;
        queue.clear();
        queue.push_back(new);
        visited_new[new] = generation;
        previous[new] = None;
        let mut available = None;
        'search: while let Some(current) = queue.pop_front() {
            for &old in &candidates[current] {
                if visited_old[old] == generation {
                    continue;
                }
                visited_old[old] = generation;
                match new_for_old[old] {
                    Some(next) => {
                        if visited_new[next] != generation {
                            visited_new[next] = generation;
                            previous[next] = Some((current, old));
                            queue.push_back(next);
                        }
                    }
                    None => {
                        available = Some((current, old));
                        break 'search;
                    }
                }
            }
        }
        while let Some((current, old)) = available {
            old_for_new[current] = Some(old);
            new_for_old[old] = Some(current);
            available = previous[current];
        }
    }
    old_for_new
        .into_iter()
        .enumerate()
        .filter_map(|(new, old)| old.map(|old| (old, new)))
        .collect()
}

fn array(
    a: &[Value],
    b: &[Value],
    options: &DiffOptions,
    path: &str,
) -> Result<Option<Value>, Error> {
    if let Some(filter) = &options.node_filter {
        let (projected, original_positions): (Vec<_>, Vec<_>) = (0..a.len().max(b.len()))
            .filter_map(|index| {
                let child_path = pointer(path, index);
                if !filter(&child_path, a.get(index), b.get(index)) {
                    a.get(index).cloned().map(|value| (value, None))
                } else if let Some(value) = b.get(index) {
                    // Nested filtering waits for identity matching, so moved
                    // objects retain fields from their corresponding source.
                    Some((value.clone(), Some(index)))
                } else {
                    a.get(index)
                        .and_then(|value| project_removed(value, options, &child_path))
                        .map(|value| (value, None))
                }
            })
            .unzip();
        if a == projected {
            return Ok(None);
        }
        return array_diff(a, &projected, options, path, Some(&original_positions));
    }
    array_diff(a, b, options, path, None)
}

pub(crate) fn check_array_capacity(a: &[Value], b: &[Value], path: &str) -> Result<(), Error> {
    if a.len().max(b.len()) >= i32::MAX as usize
        || a.len()
            .checked_add(b.len())
            .is_none_or(|n| n >= i32::MAX as usize)
    {
        return Err(Error::new(
            path,
            "array exceeds sequence algorithm index capacity",
        ));
    }
    Ok(())
}

fn array_diff(
    a: &[Value],
    b: &[Value],
    options: &DiffOptions,
    path: &str,
    original_positions: Option<&[Option<usize>]>,
) -> Result<Option<Value>, Error> {
    check_array_capacity(a, b, path)?;
    let custom = options
        .array_item_matcher
        .as_ref()
        .map(|matcher| custom_matches(a, b, matcher, path));
    // Custom matching uses its candidate graph for both stable items and
    // moves. Token interning would allocate and hash values it never uses.
    let input = custom.is_none().then(|| {
        let mut interner = Interner::new(a.len() + b.len());
        let before = a
            .iter()
            .enumerate()
            .map(|(i, v)| interner.intern(key(v, i, options)))
            .collect();
        let after = b
            .iter()
            .enumerate()
            .map(|(i, v)| interner.intern(key(v, i, options)))
            .collect();
        InternedInput {
            before,
            after,
            interner,
        }
    });
    let mut old_for_new = vec![None; b.len()];
    let mut new_for_old = vec![None; a.len()];
    let stable = custom
        .as_ref()
        .map(|matches| increasing_matches(matches))
        .or_else(|| input.as_ref().and_then(unique_matches))
        .or_else(|| input.as_ref().and_then(bounded_matches));
    if let Some(matches) = stable {
        for (old, new) in matches {
            old_for_new[new] = Some(old);
            new_for_old[old] = Some(new);
        }
    } else if let Some(input) = &input {
        let differences = Diff::compute(Algorithm::Histogram, input);
        let (mut old, mut new) = (0, 0);
        for hunk in differences.hunks() {
            while old < hunk.before.start as usize {
                old_for_new[new] = Some(old);
                new_for_old[old] = Some(new);
                old += 1;
                new += 1;
            }
            old = hunk.before.end as usize;
            new = hunk.after.end as usize;
        }
        while old < a.len() {
            old_for_new[new] = Some(old);
            new_for_old[old] = Some(new);
            old += 1;
            new += 1;
        }
    }
    let survivors = new_for_old.iter().map(Option::is_some).collect::<Vec<_>>();
    if options.detect_moves {
        if let Some(matches) = custom {
            for (old, new) in matches {
                old_for_new[new] = Some(old);
                new_for_old[old] = Some(new);
            }
        } else if let Some(input) = &input {
            let mut candidates: HashMap<_, VecDeque<usize>> = HashMap::new();
            for (old, token) in input.before.iter().enumerate() {
                if new_for_old[old].is_none() {
                    candidates.entry(*token).or_default().push_back(old);
                }
            }
            for (new, token) in input.after.iter().enumerate() {
                if old_for_new[new].is_none() {
                    if let Some(old) = candidates.get_mut(token).and_then(VecDeque::pop_front) {
                        old_for_new[new] = Some(old);
                        new_for_old[old] = Some(new);
                    }
                }
            }
        }
    }
    let mut result = Map::new();
    result.insert("_t".into(), json!("a"));
    for (old, paired) in new_for_old.iter().enumerate() {
        if let Some(new) = paired {
            if !survivors[old] {
                let value = if options.include_value_on_move {
                    a[old].clone()
                } else {
                    json!("")
                };
                result.insert(format!("_{old}"), json!([value, new, 3]));
            }
        } else {
            result.insert(format!("_{old}"), json!([a[old], 0, 0]));
        }
    }
    for (new, paired) in old_for_new.iter().enumerate() {
        let mut projected_options;
        let child_options = if let Some(positions) = original_positions {
            if positions[new] == Some(new) {
                options
            } else {
                projected_options = options.clone();
                projected_options.node_filter = positions[new].and_then(|original| {
                    options.node_filter.as_ref().map(|filter| {
                        let filter = filter.clone();
                        let original_path = pointer(path, original);
                        let target_path = pointer(path, new);
                        // Filtering sees original input paths even when omitted
                        // items compact the output. Delta/error paths stay final.
                        Arc::new(
                            move |current: &str, left: Option<&Value>, right: Option<&Value>| {
                                if let Some(suffix) = current.strip_prefix(&target_path) {
                                    filter(&format!("{original_path}{suffix}"), left, right)
                                } else {
                                    filter(current, left, right)
                                }
                            },
                        ) as crate::NodeFilter
                    })
                });
                &projected_options
            }
        } else {
            options
        };
        if let Some(old) = paired {
            if let Some(child) =
                node_unfiltered(&a[*old], &b[new], child_options, &pointer(path, new))?
            {
                result.insert(new.to_string(), child);
            }
        } else {
            let added = if child_options.node_filter.is_some() {
                project_present(None, &b[new], child_options, &pointer(path, new))
            } else {
                b[new].clone()
            };
            result.insert(new.to_string(), json!([added]));
        }
    }
    if result.len() == 1 {
        Ok(None)
    } else {
        Ok(Some(Value::Object(result)))
    }
}
