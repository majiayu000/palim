//! Comparison reports intentionally do not promise reconstruction of the target.
use crate::{ArrayItemMatcher, Error, ObjectHash, delta, json_equal, pointer};
use serde_json::{Number, Value};
use std::{
    collections::{HashMap, VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::Arc,
};

/// Return false to exclude a node, including root, array items and absent values.
pub type CompareNodeFilter = crate::NodeFilter;
/// Return Some to decide equality of a node, or None to use normal comparison.
pub type CustomEqual = Arc<dyn Fn(&str, &Value, &Value) -> Option<bool> + Send + Sync>;
/// Select array paths whose order is immaterial. Repeated values retain their counts.
pub type UnorderedArrays = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// Options for a comparison report, separate from reversible delta generation.
#[derive(Clone)]
pub struct CompareOptions {
    /// Excluded nodes contribute neither differences nor similarity units.
    pub node_filter: Option<CompareNodeFilter>,
    /// Runs before ordinary equality; can override even identical values.
    pub custom_equal: Option<CustomEqual>,
    /// Array order is significant unless this callback returns true.
    pub unordered: Option<UnorderedArrays>,
    /// Takes precedence over object_hash when both are supplied.
    pub array_item_matcher: Option<ArrayItemMatcher>,
    /// Stable identities for container items; repeated identities pair in FIFO order.
    pub object_hash: Option<ObjectHash>,
    /// Exact decimal tolerance: distance <= max(absolute, relative * maximum magnitude).
    /// Finite f64 thresholds are interpreted as their shortest JSON decimal values.
    /// Arbitrary-precision inputs are compared without conversion to floating point.
    pub absolute_tolerance: f64,
    /// Nonnegative finite relative numeric tolerance; zero keeps exact equality.
    pub relative_tolerance: f64,
    /// Maximum recorded differences plus index moves. Exceeding it returns Error.
    pub max_differences: Option<usize>,
    /// Maximum node visits, matching work and decimal digit work, including
    /// speculative unordered pairs. Exhaustion returns an error.
    pub max_comparisons: Option<usize>,
    /// Maximum input container nesting, in 0..=128.
    pub max_depth: usize,
}
impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            node_filter: None,
            custom_equal: None,
            unordered: None,
            array_item_matcher: None,
            object_hash: None,
            absolute_tolerance: 0.0,
            relative_tolerance: 0.0,
            max_differences: None,
            max_comparisons: None,
            max_depth: 128,
        }
    }
}

/// A changed, added or removed value. None denotes absence, not JSON null.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Difference {
    /// Target path for paired/addition values, source path for removals.
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub left: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub right: Option<Value>,
}
fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    <Value as serde::Deserialize>::deserialize(deserializer).map(Some)
}
/// An identity-matched item whose source and target indices differ.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArrayMove {
    pub from: String,
    pub path: String,
}
/// JSON comparison output. It is not an applicable or reversible patch.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompareReport {
    pub differences: Vec<Difference>,
    /// Empty for unordered arrays. Reports index changes, not a minimal edit script.
    pub moves: Vec<ArrayMove>,
    /// Matching terminal/absent/type-change units divided by all such units.
    /// Each reported move adds an unmatched unit; excluded nodes add no units.
    /// Empty/fully excluded comparisons have similarity 1.0.
    pub similarity: f64,
    /// Node visits, matching checks and decimal digit work, including discarded
    /// candidate comparisons.
    pub visited: usize,
}

/// Compare JSON with mathematical number equality by default (1 equals 1.0).
/// Inputs remain unchanged. Exhausted budgets return an error, never partial output.
pub fn compare(
    left: &Value,
    right: &Value,
    options: &CompareOptions,
) -> Result<CompareReport, Error> {
    if options.max_depth > 128 {
        return Err(Error::new("", "max_depth must not exceed 128"));
    }
    if !options.absolute_tolerance.is_finite()
        || options.absolute_tolerance < 0.0
        || !options.relative_tolerance.is_finite()
        || options.relative_tolerance < 0.0
    {
        return Err(Error::new(
            "",
            "numeric tolerances must be finite and nonnegative",
        ));
    }
    delta::check_depth(left, options.max_depth)?;
    delta::check_depth(right, options.max_depth)?;
    let mut context = Context {
        options,
        report: CompareReport {
            differences: Vec::new(),
            moves: Vec::new(),
            similarity: 1.0,
            visited: 0,
        },
        units: 0,
        equal_units: 0,
    };
    context.node("", Some(left), Some(right), true)?;
    if context.units != 0 {
        context.report.similarity = context.equal_units as f64 / context.units as f64;
    }
    Ok(context.report)
}

struct Context<'a> {
    options: &'a CompareOptions,
    report: CompareReport,
    units: usize,
    equal_units: usize,
}
impl Context<'_> {
    fn visit(&mut self, path: &str) -> Result<(), Error> {
        self.spend(path, 1)
    }
    fn spend(&mut self, path: &str, work: usize) -> Result<(), Error> {
        if self
            .options
            .max_comparisons
            .is_some_and(|limit| work > limit.saturating_sub(self.report.visited))
        {
            return Err(Error::new(path, "comparison exceeds max_comparisons"));
        }
        self.report.visited = self
            .report
            .visited
            .checked_add(work)
            .ok_or_else(|| Error::new(path, "comparison count overflow"))?;
        Ok(())
    }
    fn change_allowed(&self, path: &str) -> Result<(), Error> {
        if self.options.max_differences.is_some_and(|limit| {
            self.report
                .differences
                .len()
                .saturating_add(self.report.moves.len())
                >= limit
        }) {
            return Err(Error::new(path, "comparison exceeds max_differences"));
        }
        Ok(())
    }
    fn leaf(
        &mut self,
        path: &str,
        left: Option<&Value>,
        right: Option<&Value>,
        equal: bool,
        record: bool,
    ) -> Result<bool, Error> {
        if record {
            self.units += 1;
            if equal {
                self.equal_units += 1;
            } else {
                self.change_allowed(path)?;
                self.report.differences.push(Difference {
                    path: path.to_owned(),
                    left: left.cloned(),
                    right: right.cloned(),
                });
            }
        }
        Ok(equal)
    }
    fn node(
        &mut self,
        path: &str,
        left: Option<&Value>,
        right: Option<&Value>,
        record: bool,
    ) -> Result<bool, Error> {
        self.node_moving(path, left, right, record, None)
    }
    fn node_moving(
        &mut self,
        path: &str,
        left: Option<&Value>,
        right: Option<&Value>,
        record: bool,
        moved_from: Option<&str>,
    ) -> Result<bool, Error> {
        self.visit(path)?;
        if self
            .options
            .node_filter
            .as_ref()
            .is_some_and(|filter| !filter(path, left, right))
        {
            return Ok(true);
        }
        if let Some(from) = moved_from {
            if !record {
                return Ok(false);
            }
            self.change_allowed(path)?;
            self.report.moves.push(ArrayMove {
                from: from.to_owned(),
                path: path.to_owned(),
            });
            self.units += 1;
        }
        Ok(self.node_body(path, left, right, record)? && moved_from.is_none())
    }
    fn node_body(
        &mut self,
        path: &str,
        left: Option<&Value>,
        right: Option<&Value>,
        record: bool,
    ) -> Result<bool, Error> {
        let (Some(left_value), Some(right_value)) = (left, right) else {
            return self.leaf(path, left, right, left.is_none() && right.is_none(), record);
        };
        if let Some(equal) = self
            .options
            .custom_equal
            .as_ref()
            .and_then(|callback| callback(path, left_value, right_value))
        {
            return self.leaf(path, left, right, equal, record);
        }
        match (left_value, right_value) {
            (Value::Object(a), Value::Object(b)) => {
                if a.is_empty() && b.is_empty() {
                    return self.leaf(path, left, right, true, record);
                }
                let mut equal = true;
                for (key, value) in a {
                    equal &= self.node(&pointer(path, key), Some(value), b.get(key), record)?;
                    if !record && !equal {
                        return Ok(false);
                    }
                }
                for (key, value) in b {
                    if !a.contains_key(key) {
                        equal &= self.node(&pointer(path, key), None, Some(value), record)?;
                        if !record && !equal {
                            return Ok(false);
                        }
                    }
                }
                Ok(equal)
            }
            (Value::Array(a), Value::Array(b)) => self.array(path, a, b, record),
            (Value::Number(a), Value::Number(b)) => {
                let equal = self.close_numbers(path, a, b)?;
                self.leaf(path, left, right, equal, record)
            }
            _ => self.leaf(
                path,
                left,
                right,
                json_equal(left_value, right_value),
                record,
            ),
        }
    }
    fn close_numbers(&mut self, path: &str, a: &Number, b: &Number) -> Result<bool, Error> {
        if self.options.absolute_tolerance == 0.0 && self.options.relative_tolerance == 0.0 {
            if a == b {
                return Ok(true);
            }
            self.spend(path, crate::numbers::number_text(a).len())?;
            self.spend(path, crate::numbers::number_text(b).len())?;
            return Ok(crate::numbers::numbers_equal(a, b));
        }
        crate::numbers::within_tolerance(
            a,
            b,
            self.options.absolute_tolerance,
            self.options.relative_tolerance,
            |work| self.spend(path, work),
        )
    }
    fn array(&mut self, path: &str, a: &[Value], b: &[Value], record: bool) -> Result<bool, Error> {
        if a.is_empty() && b.is_empty() {
            if record {
                self.units += 1;
                self.equal_units += 1;
            }
            return Ok(true);
        }
        let unordered = self
            .options
            .unordered
            .as_ref()
            .is_some_and(|callback| callback(path));
        let identity =
            self.options.array_item_matcher.is_some() || self.options.object_hash.is_some();
        if !unordered && !identity {
            let mut equal = true;
            for i in 0..a.len().max(b.len()) {
                equal &= self.node(&pointer(path, i), a.get(i), b.get(i), record)?;
                if !record && !equal {
                    return Ok(false);
                }
            }
            return Ok(equal);
        }
        let pairs = self.pairs(path, a, b, identity)?;
        let mut used_left = vec![false; a.len()];
        let mut equal = true;
        for (j, old) in pairs.into_iter().enumerate() {
            let target_path = pointer(path, j);
            if let Some(i) = old {
                used_left[i] = true;
                let source_path = (!unordered && i != j).then(|| pointer(path, i));
                equal &= self.node_moving(
                    &target_path,
                    Some(&a[i]),
                    Some(&b[j]),
                    record,
                    source_path.as_deref(),
                )?;
            } else {
                equal &= self.node(&target_path, None, Some(&b[j]), record)?;
            }
            if !record && !equal {
                return Ok(false);
            }
        }
        for (i, value) in a.iter().enumerate() {
            if !used_left[i] {
                equal &= self.node(&pointer(path, i), Some(value), None, record)?;
                if !record && !equal {
                    return Ok(false);
                }
            }
        }
        Ok(equal)
    }
    fn pairs(
        &mut self,
        path: &str,
        a: &[Value],
        b: &[Value],
        identity: bool,
    ) -> Result<Vec<Option<usize>>, Error> {
        if let Some(matcher) = &self.options.array_item_matcher {
            let matcher = Arc::clone(matcher);
            let mut edges = vec![Vec::new(); a.len()];
            for (i, left) in a.iter().enumerate() {
                for (j, right) in b.iter().enumerate() {
                    self.visit(&pointer(path, j))?;
                    if matcher(path, left, right) {
                        edges[i].push(j);
                    }
                }
            }
            return self.maximum_matching(path, &edges, b.len());
        }
        if let Some(hash) = &self.options.object_hash {
            let hash = Arc::clone(hash);
            let mut keys = HashMap::<String, VecDeque<usize>>::new();
            let mut unkeyed_left = Vec::new();
            for (i, value) in a.iter().enumerate() {
                self.visit(&pointer(path, i))?;
                if let Some(key) = (value.is_object() || value.is_array())
                    .then(|| hash(value, i))
                    .flatten()
                {
                    keys.entry(key).or_default().push_back(i);
                } else {
                    unkeyed_left.push(i);
                }
            }
            let mut pairs = vec![None; b.len()];
            let mut unkeyed_right = Vec::new();
            for (j, value) in b.iter().enumerate() {
                self.visit(&pointer(path, j))?;
                if let Some(key) = (value.is_object() || value.is_array())
                    .then(|| hash(value, j))
                    .flatten()
                {
                    pairs[j] = keys.get_mut(&key).and_then(VecDeque::pop_front);
                } else {
                    unkeyed_right.push(j);
                }
            }
            let mut edges = vec![Vec::new(); unkeyed_left.len()];
            for (ii, &i) in unkeyed_left.iter().enumerate() {
                for (jj, &j) in unkeyed_right.iter().enumerate() {
                    if self.node(&pointer(path, j), Some(&a[i]), Some(&b[j]), false)? {
                        edges[ii].push(jj);
                    }
                }
            }
            for (jj, ii) in self
                .maximum_matching(path, &edges, unkeyed_right.len())?
                .into_iter()
                .enumerate()
            {
                if let Some(ii) = ii {
                    pairs[unkeyed_right[jj]] = Some(unkeyed_left[ii]);
                }
            }
            return Ok(pairs);
        }
        let hashable = !identity
            && self.options.node_filter.is_none()
            && self.options.custom_equal.is_none()
            && self.options.absolute_tolerance == 0.0
            && self.options.relative_tolerance == 0.0;
        if hashable && a.iter().chain(b).all(|v| !v.is_object() && !v.is_array()) {
            let mut keys = HashMap::<ScalarKey<'_>, VecDeque<usize>>::new();
            for (i, value) in a.iter().enumerate() {
                let item_path = pointer(path, i);
                self.visit(&item_path)?;
                if let Value::Number(number) = value {
                    self.spend(&item_path, crate::numbers::number_text(number).len())?;
                }
                keys.entry(scalar_key(value)).or_default().push_back(i);
            }
            let mut result = Vec::with_capacity(b.len());
            for (j, value) in b.iter().enumerate() {
                let item_path = pointer(path, j);
                self.visit(&item_path)?;
                if let Value::Number(number) = value {
                    self.spend(&item_path, crate::numbers::number_text(number).len())?;
                }
                result.push(
                    keys.get_mut(&scalar_key(value))
                        .and_then(VecDeque::pop_front),
                );
            }
            return Ok(result);
        }
        if hashable {
            // Omit array order from fingerprints, then check actual path-specific
            // comparison again. Hash collisions create candidates, never equality.
            let mut targets = HashMap::<u64, Vec<usize>>::new();
            for (j, value) in b.iter().enumerate() {
                let hash = self.fingerprint(&pointer(path, j), value)?;
                targets.entry(hash).or_default().push(j);
            }
            let mut edges = vec![Vec::new(); a.len()];
            for (i, value) in a.iter().enumerate() {
                let hash = self.fingerprint(&pointer(path, i), value)?;
                if let Some(candidates) = targets.get(&hash) {
                    for &j in candidates {
                        if self.node(&pointer(path, j), Some(value), Some(&b[j]), false)? {
                            edges[i].push(j);
                        }
                    }
                }
            }
            return self.maximum_matching(path, &edges, b.len());
        }
        let mut edges = vec![Vec::new(); a.len()];
        for (i, left) in a.iter().enumerate() {
            for (j, right) in b.iter().enumerate() {
                if self.node(&pointer(path, j), Some(left), Some(right), false)? {
                    edges[i].push(j);
                }
            }
        }
        self.maximum_matching(path, &edges, b.len())
    }
    fn fingerprint(&mut self, path: &str, value: &Value) -> Result<u64, Error> {
        self.visit(path)?;
        let mut hasher = DefaultHasher::new();
        match value {
            Value::Array(values) => {
                0_u8.hash(&mut hasher);
                let mut children = Vec::with_capacity(values.len());
                for (i, value) in values.iter().enumerate() {
                    children.push(self.fingerprint(&pointer(path, i), value)?);
                }
                children.sort_unstable();
                children.hash(&mut hasher);
            }
            Value::Object(values) => {
                1_u8.hash(&mut hasher);
                let mut children = Vec::with_capacity(values.len());
                for (key, value) in values {
                    children.push((key, self.fingerprint(&pointer(path, key), value)?));
                }
                children.sort_unstable_by(|a, b| a.0.cmp(b.0));
                children.hash(&mut hasher);
            }
            _ => {
                2_u8.hash(&mut hasher);
                if let Value::Number(number) = value {
                    self.spend(path, crate::numbers::number_text(number).len())?;
                }
                scalar_key(value).hash(&mut hasher);
            }
        }
        Ok(hasher.finish())
    }
    // Iterative augmenting paths prevent greedy mismatches for non-transitive tolerances.
    fn maximum_matching(
        &mut self,
        path: &str,
        edges: &[Vec<usize>],
        right_len: usize,
    ) -> Result<Vec<Option<usize>>, Error> {
        let mut right_to_left: Vec<Option<usize>> = vec![None; right_len];
        let mut left_to_right: Vec<Option<usize>> = vec![None; edges.len()];
        let mut seen_left = vec![usize::MAX; edges.len()];
        let mut seen_right = vec![usize::MAX; right_len];
        let mut previous = vec![None; right_len];
        let mut queue = VecDeque::new();
        for start in 0..edges.len() {
            queue.clear();
            queue.push_back(start);
            seen_left[start] = start;
            let mut destination = None;
            'search: while let Some(i) = queue.pop_front() {
                for &j in &edges[i] {
                    self.visit(&pointer(path, j))?;
                    if seen_right[j] == start {
                        continue;
                    }
                    seen_right[j] = start;
                    previous[j] = Some(i);
                    if let Some(old) = right_to_left[j] {
                        if seen_left[old] != start {
                            seen_left[old] = start;
                            queue.push_back(old);
                        }
                    } else {
                        destination = Some(j);
                        break 'search;
                    }
                }
            }
            while let Some(j) = destination {
                let Some(i) = previous[j] else {
                    break;
                };
                destination = left_to_right[i];
                left_to_right[i] = Some(j);
                right_to_left[j] = Some(i);
            }
        }
        Ok(right_to_left)
    }
}

#[derive(Hash, PartialEq, Eq)]
enum ScalarKey<'a> {
    Null,
    Bool(bool),
    String(&'a str),
    Number(crate::numbers::NumberKey<'a>),
}
fn scalar_key(value: &Value) -> ScalarKey<'_> {
    match value {
        Value::Null => ScalarKey::Null,
        Value::Bool(value) => ScalarKey::Bool(*value),
        Value::String(value) => ScalarKey::String(value),
        Value::Number(value) => ScalarKey::Number(crate::numbers::number_key(value)),
        // The primitive fast path establishes this before calling scalar_key.
        Value::Array(_) | Value::Object(_) => unreachable!("scalar_key requires a scalar"),
    }
}
