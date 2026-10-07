#![doc = include_str!("../README.md")]
use serde_json::Value;
use std::{fmt, sync::Arc};
mod delta;
mod diff;
mod export;
mod merge;
mod numbers;
mod patch;
mod text;
pub use merge::{compose_merge_patches, diff_merge_patch, merge_patch, merge_patch_in_place};
mod compare;
pub use compare::{
    ArrayMove, CompareNodeFilter, CompareOptions, CompareReport, CustomEqual, Difference,
    UnorderedArrays, compare,
};
mod rfc;
pub use rfc::{JsonPatchOptions, diff_json_patch, invert_json_patch, invert_json_patch_guarded};

/// A patch or diff failure, with a JSON Pointer identifying the affected value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub path: String,
    pub message: String,
}
impl Error {
    pub(crate) fn new(path: &str, message: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            message: message.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            if self.path.is_empty() {
                "<root>"
            } else {
                &self.path
            },
            self.message
        )
    }
}
impl std::error::Error for Error {}

/// Return an identity for an object/array item. Called once per item in each changed array.
pub type ObjectHash = Arc<dyn Fn(&Value, usize) -> Option<String> + Send + Sync>;
/// Return false to exclude an object property from comparison.
pub type PropertyFilter = Arc<dyn Fn(&str, &Value, &Value, &str) -> bool + Send + Sync>;
/// Select any JSON node, including roots, missing values and array positions.
pub type NodeFilter = Arc<dyn Fn(&str, Option<&Value>, Option<&Value>) -> bool + Send + Sync>;
/// Pair array elements using caller-defined identity or similarity.
pub type ArrayItemMatcher = Arc<dyn Fn(&str, &Value, &Value) -> bool + Send + Sync>;

/// Bounds for opt-in approximate text application. Distances use UTF-16 units.
#[derive(Clone, Debug)]
pub struct TextPatchOptions {
    pub max_distance: usize,
    pub max_error_ratio: f64,
}
impl Default for TextPatchOptions {
    fn default() -> Self {
        Self {
            max_distance: 1000,
            max_error_ratio: 0.5,
        }
    }
}
impl TextPatchOptions {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if !self.max_error_ratio.is_finite() || !(0.0..=1.0).contains(&self.max_error_ratio) {
            return Err(Error::new(
                "",
                "max_error_ratio must be finite and in 0..=1",
            ));
        }
        Ok(())
    }
}

/// Instance-local matching options.
#[derive(Clone)]
pub struct DiffOptions {
    /// Custom identity takes priority over positional matching; None falls back to the value.
    pub object_hash: Option<ObjectHash>,
    /// Receives (property name, left parent, right parent, parent JSON Pointer).
    pub property_filter: Option<PropertyFilter>,
    /// Filter roots, object properties and positional array projections.
    pub node_filter: Option<NodeFilter>,
    /// Custom pairing, mutually exclusive with object_hash.
    pub array_item_matcher: Option<ArrayItemMatcher>,
    /// Match objects and nested arrays by index when no identity callback is configured.
    pub match_by_position: bool,
    /// Pair unmatched equal tokens as moves instead of remove/add pairs.
    pub detect_moves: bool,
    /// Include the original moved value in the delta instead of an empty-string placeholder.
    pub include_value_on_move: bool,
    /// None disables text diffs; length is measured in UTF-16 code units.
    pub text_diff_min_length: Option<usize>,
    /// Maximum nested object/array depth, including the root, in 0..=128.
    pub max_depth: usize,
}
impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            object_hash: None,
            property_filter: None,
            node_filter: None,
            array_item_matcher: None,
            match_by_position: true,
            detect_moves: true,
            include_value_on_move: false,
            text_diff_min_length: Some(60),
            max_depth: 128,
        }
    }
}

/// A complete reversible delta, serialized using jsondiffpatch's JSON protocol.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct Delta(pub(crate) Value);
impl Delta {
    /// Decode a complete delta. Old values must not have been omitted by its producer.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        delta::validate(&value, "", 0)?;
        delta::check_depth(&value, delta::MAX_DELTA_DEPTH)?;
        Ok(Self(value))
    }
    /// Borrow the validated protocol JSON.
    pub fn as_value(&self) -> &Value {
        &self.0
    }
    /// Take ownership of the protocol JSON.
    pub fn into_value(self) -> Value {
        self.0
    }
    /// Validate against left and export an RFC 6902 sequence; text edits become replace.
    pub fn to_json_patch(&self, left: &Value) -> Result<json_patch::Patch, Error> {
        export::export(left, self)
    }
    /// Produce a forward-only delta with old replacement/deletion values omitted.
    pub fn to_forward_only(&self) -> ForwardDelta {
        ForwardDelta(delta::strip_old(&self.0))
    }
}
impl<'de> serde::Deserialize<'de> for Delta {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_value(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// A delta with omitted old values. Its wire representation has no reversible marker;
/// deserialize it as this type, never as Delta. It intentionally has no reverse method.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct ForwardDelta(Value);
impl ForwardDelta {
    /// Decode a forward-only delta whose producer may have omitted old values.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        delta::validate(&value, "", 0)?;
        delta::check_depth(&value, delta::MAX_DELTA_DEPTH)?;
        Ok(Self(value))
    }
    /// Borrow the protocol JSON.
    pub fn as_value(&self) -> &Value {
        &self.0
    }
    /// Apply without old-value equality checks; structural and text checks still apply.
    pub fn patch(&self, left: &Value) -> Result<Value, Error> {
        patch::apply(left, &self.0, false)
    }
}
impl<'de> serde::Deserialize<'de> for ForwardDelta {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_value(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// A reusable, shareable diff instance without global configuration.
#[derive(Clone, Default)]
pub struct DiffPatcher {
    options: DiffOptions,
}
impl DiffPatcher {
    /// Construct an independent instance. Callbacks must be thread safe.
    pub fn new(options: DiffOptions) -> Self {
        Self { options }
    }
    /// Match array objects by a named field, such as `"id"`.
    ///
    /// The key is a literal field name, not a JSON Pointer. Identities use the
    /// field's JSON representation, so `1` and `"1"` are distinct. Items without
    /// the field fall back to their complete JSON value. Repeated identities are
    /// paired deterministically; stable unique IDs usually produce smaller deltas.
    /// All other options retain their defaults.
    ///
    /// ```
    /// use palim::{DiffPatcher, patch, unpatch};
    /// use serde_json::json;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let before = json!([{"id": 1, "done": false}, {"id": 2, "done": false}]);
    /// let after = json!([{"id": 2, "done": true}, {"id": 1, "done": false}]);
    /// if let Some(delta) = DiffPatcher::by_key("id").diff(&before, &after)? {
    ///     assert_eq!(patch(&before, &delta)?, after);
    ///     assert_eq!(unpatch(&after, &delta)?, before);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn by_key(key: impl Into<String>) -> Self {
        let key = key.into();
        Self::new(DiffOptions {
            object_hash: Some(Arc::new(move |item, _| {
                item.as_object()?.get(&key).map(Value::to_string)
            })),
            ..Default::default()
        })
    }
    /// Compare JSON values; None means no differences after property filtering.
    pub fn diff(&self, left: &Value, right: &Value) -> Result<Option<Delta>, Error> {
        self.check_inputs(left, right)?;
        self.diff_validated(left, right)
    }
    fn check_inputs(&self, left: &Value, right: &Value) -> Result<usize, Error> {
        if self.options.object_hash.is_some() && self.options.array_item_matcher.is_some() {
            return Err(Error::new(
                "",
                "choose object_hash or array_item_matcher, not both",
            ));
        }
        if self.options.max_depth > 128 {
            return Err(Error::new("", "max_depth must not exceed 128"));
        }
        let left_depth = delta::measure_depth(left, self.options.max_depth)?;
        let right_depth = delta::measure_depth(right, self.options.max_depth)?;
        Ok(left_depth.max(right_depth))
    }
    fn diff_validated(&self, left: &Value, right: &Value) -> Result<Option<Delta>, Error> {
        let result = diff::node(left, right, &self.options, "")?;
        if let Some(value) = &result {
            delta::check_depth(value, delta::MAX_DELTA_DEPTH)?;
        }
        Ok(result.map(Delta))
    }
    /// Apply a delta without changing left. Matching options do not affect application.
    pub fn patch(&self, left: &Value, delta: &Delta) -> Result<Value, Error> {
        patch(left, delta)
    }
    /// Reverse the original delta without the source document.
    pub fn reverse(&self, delta: &Delta) -> Result<Delta, Error> {
        reverse(delta)
    }
    /// Apply the inverse of the original delta to right.
    pub fn unpatch(&self, right: &Value, delta: &Delta) -> Result<Value, Error> {
        unpatch(right, delta)
    }
    /// Generate optimized standard operations using this instance's matching choices.
    /// With all JSON Patch flags disabled, unique primitive reorders can use
    /// positional replacements when estimated move bytes offer little saving.
    /// Native deltas and their JSON Patch exporter retain their move strategy.
    pub fn diff_json_patch(
        &self,
        left: &Value,
        right: &Value,
        options: &JsonPatchOptions,
    ) -> Result<Patch, Error> {
        let input_depth = self.check_inputs(left, right)?;
        if (!options.tests || (!options.factorize && !options.rationalize))
            && self.options.node_filter.is_none()
            && self.options.property_filter.is_none()
            && self.options.array_item_matcher.is_none()
        {
            if let Some(mut standard) =
                export::disjoint_array_patch(left, right, !options.factorize)?
            {
                if options.tests && !standard.0.is_empty() {
                    // Positional replacements need one baseline array test to
                    // retain the structural guards of the remove/add pipeline.
                    standard.0.insert(
                        0,
                        PatchOperation::Test(json_patch::TestOperation {
                            path: json_patch::jsonptr::PointerBuf::new(),
                            value: export::copy_addition(left),
                        }),
                    );
                }
                return rfc::optimize(left, right, standard, options);
            }
        }
        if self.options.node_filter.is_none() && self.options.property_filter.is_none() {
            let mut matching = self.options.clone();
            matching.text_diff_min_length = None;
            let standard = export::direct_patch(left, right, &matching, options, input_depth)?;
            if standard.0.is_empty() {
                return Ok(standard);
            }
            return rfc::optimize(left, right, standard, options);
        }
        // Standard patches replace text values as a whole. Constructing a
        // text delta here would perform matching only to discard its result.
        let change = if self.options.text_diff_min_length.is_some() {
            let mut options = self.options.clone();
            options.text_diff_min_length = None;
            Self::new(options).diff_validated(left, right)?
        } else {
            self.diff_validated(left, right)?
        };
        let Some(change) = change else {
            return Ok(Patch::default());
        };
        if self.options.property_filter.is_some() || self.options.node_filter.is_some() {
            let projected = patch(left, &change)?;
            let standard = export::export_known(
                left,
                &projected,
                &change,
                !options.factorize && !options.tests,
            )?;
            rfc::optimize(left, &projected, standard, options)
        } else {
            let standard =
                export::export_known(left, right, &change, !options.factorize && !options.tests)?;
            rfc::optimize(left, right, standard, options)
        }
    }
}
/// Compare using default options. Unkeyed objects in arrays are matched by position.
pub fn diff(left: &Value, right: &Value) -> Result<Option<Delta>, Error> {
    DiffPatcher::default().diff(left, right)
}
/// Apply a complete delta with strict old-value, type, index and text context checks.
pub fn patch(left: &Value, delta: &Delta) -> Result<Value, Error> {
    patch::apply(left, &delta.0, true)
}
/// Consume a baseline instead of borrowing it.
pub fn patch_owned(left: Value, delta: &Delta) -> Result<Value, Error> {
    patch::apply_owned(left, &delta.0, true)
}
/// Atomically update a baseline after successful native application.
pub fn patch_in_place(left: &mut Value, delta: &Delta) -> Result<(), Error> {
    let result = patch(left, delta)?;
    *left = result;
    Ok(())
}
/// Apply a DMP text patch with bounded approximate matching.
pub fn apply_text_patch(
    source: &str,
    text_patch: &str,
    options: &TextPatchOptions,
) -> Result<String, Error> {
    options.validate()?;
    text::apply_fuzzy(source, text_patch, "", options)
}
/// Apply native deltas with approximate text matching; non-text changes stay strict.
pub fn patch_fuzzy(
    left: &Value,
    delta: &Delta,
    options: &TextPatchOptions,
) -> Result<Value, Error> {
    options.validate()?;
    patch::apply_fuzzy(left, &delta.0, options)
}
/// Generate the inverse solely from the original delta; no re-diff is performed.
pub fn reverse(delta: &Delta) -> Result<Delta, Error> {
    patch::invert(&delta.0, "").map(Delta)
}
/// Restore a source document by applying the inverse delta to right.
pub fn unpatch(right: &Value, delta: &Delta) -> Result<Value, Error> {
    patch(right, &reverse(delta)?)
}
pub use json_patch::{Patch, PatchOperation};
/// Resource limits for standard patch application.
#[derive(Clone, Debug)]
pub struct JsonPatchApplyOptions {
    /// Cumulative serialized UTF-8 bytes copied by copy operations, excluding moves.
    pub max_copy_bytes: Option<usize>,
    pub max_depth: usize,
}
impl Default for JsonPatchApplyOptions {
    fn default() -> Self {
        Self {
            max_copy_bytes: None,
            max_depth: 128,
        }
    }
}
/// Apply all six RFC 6902 operations without changing the source.
pub fn apply_json_patch(left: &Value, patch: &Patch) -> Result<Value, Error> {
    apply_json_patch_with_options(left, patch, &JsonPatchApplyOptions::default())
}
/// Check a sequence of RFC 6902 test operations without cloning the document.
/// Other operations are rejected in sequence. Source and expected values retain
/// the standard 128-container depth limit and mathematical number equality.
pub fn test_json_patch(left: &Value, patch: &Patch) -> Result<(), Error> {
    let options = JsonPatchApplyOptions::default();
    check_standard_source(left, &options)?;
    for (index, op) in patch.0.iter().enumerate() {
        let PatchOperation::Test(test) = op else {
            return Err(standard_error(
                index,
                operation_path(op),
                "only test operations are allowed",
            ));
        };
        test_standard(left, test, index, options.max_depth)?;
    }
    Ok(())
}
/// Apply with copy and nesting limits; failures return no partial document.
pub fn apply_json_patch_with_options(
    left: &Value,
    patch: &Patch,
    options: &JsonPatchApplyOptions,
) -> Result<Value, Error> {
    check_standard_source(left, options)?;
    apply_standard_checked(left.clone(), patch, options)
}
/// Consume the baseline, avoiding the initial whole-document clone.
pub fn apply_json_patch_owned(left: Value, patch: &Patch) -> Result<Value, Error> {
    apply_standard(left, patch, &JsonPatchApplyOptions::default())
}
/// Update the caller's document only after every operation succeeds.
pub fn apply_json_patch_in_place(left: &mut Value, patch: &Patch) -> Result<(), Error> {
    let result = apply_json_patch(left, patch)?;
    *left = result;
    Ok(())
}
fn standard_error(index: usize, path: &str, message: impl fmt::Display) -> Error {
    Error::new(
        path,
        format!("operation '/{index}' failed at path '{path}': {message}"),
    )
}
fn operation_path(op: &PatchOperation) -> &str {
    match op {
        PatchOperation::Add(op) => op.path.as_str(),
        PatchOperation::Remove(op) => op.path.as_str(),
        PatchOperation::Replace(op) => op.path.as_str(),
        PatchOperation::Move(op) => op.path.as_str(),
        PatchOperation::Copy(op) => op.path.as_str(),
        PatchOperation::Test(op) => op.path.as_str(),
    }
}
fn check_standard_source(left: &Value, options: &JsonPatchApplyOptions) -> Result<(), Error> {
    if options.max_depth > 128 {
        return Err(Error::new("", "max_depth must not exceed 128"));
    }
    delta::check_depth(left, options.max_depth)
}
fn apply_standard(
    result: Value,
    patch: &Patch,
    options: &JsonPatchApplyOptions,
) -> Result<Value, Error> {
    check_standard_source(&result, options)?;
    apply_standard_checked(result, patch, options)
}
fn apply_standard_checked(
    mut result: Value,
    patch: &Patch,
    options: &JsonPatchApplyOptions,
) -> Result<Value, Error> {
    let mut copied = 0usize;
    for (index, op) in patch.0.iter().enumerate() {
        if let (Some(limit), PatchOperation::Copy(copy)) = (options.max_copy_bytes, op) {
            let value = copy.from.resolve(&result).map_err(|_| {
                standard_error(index, copy.path.as_str(), "\"from\" path is invalid")
            })?;
            let mut size = ByteCount(0);
            serde_json::to_writer(&mut size, value)
                .map_err(|e| standard_error(index, copy.path.as_str(), e))?;
            copied = copied.checked_add(size.0).ok_or_else(|| {
                standard_error(index, copy.path.as_str(), "copy byte count overflow")
            })?;
            if copied > limit {
                return Err(standard_error(
                    index,
                    copy.path.as_str(),
                    "copy byte budget exceeded",
                ));
            }
        }
        step_with_depth(&mut result, op, index, options.max_depth)?;
    }
    Ok(result)
}
struct ByteCount(usize);
impl std::io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("JSON size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// Mutates private, already depth-checked scratch documents. A failing caller
/// must discard its scratch; public APIs never expose this partial state.
pub(crate) fn apply_json_patch_step(
    result: &mut Value,
    op: &PatchOperation,
    index: usize,
) -> Result<(), Error> {
    step_with_depth(result, op, index, 128)
}
fn step_with_depth(
    result: &mut Value,
    op: &PatchOperation,
    index: usize,
    max_depth: usize,
) -> Result<(), Error> {
    if let PatchOperation::Test(test) = op {
        return test_standard(result, test, index, max_depth);
    }
    let path = operation_path(op);
    let payload_height = match op {
        PatchOperation::Add(op) => Some(standard_subtree_height(&op.value, max_depth)),
        PatchOperation::Replace(op) => Some(standard_subtree_height(&op.value, max_depth)),
        _ => None,
    }
    .transpose()
    .map_err(|e| standard_error(index, path, e.message))?;
    if matches!(op, PatchOperation::Move(m) if m.from.as_str().is_empty() && m.path.as_str().is_empty())
    {
        return Ok(());
    }
    let target_depth = path.bytes().filter(|byte| *byte == b'/').count();
    let inserted_height = match op {
        PatchOperation::Move(op) => moved_subtree_height(result, &op.from, target_depth, max_depth),
        PatchOperation::Copy(op) => moved_subtree_height(result, &op.from, target_depth, max_depth),
        _ => Ok(payload_height),
    }
    .map_err(|e| standard_error(index, path, e.message))?;
    json_patch::patch_unsafe(result, std::slice::from_ref(op)).map_err(|mut e| {
        e.operation = index;
        Error::new(e.path.as_str(), e.to_string())
    })?;
    // A successful standard operation guarantees one container ancestor per
    // destination token, including an array append token. All unchanged branches
    // were bounded before this step, so only the inserted subtree can add depth.
    // Check after application to preserve invalid-path/from/move error priority.
    if inserted_height.is_some_and(|height| {
        target_depth
            .checked_add(height)
            .is_none_or(|depth| depth > max_depth)
    }) {
        return Err(standard_error(
            index,
            path,
            "JSON nesting exceeds max_depth",
        ));
    }
    Ok(())
}

fn test_standard(
    source: &Value,
    test: &json_patch::TestOperation,
    index: usize,
    max_depth: usize,
) -> Result<(), Error> {
    let path = test.path.as_str();
    standard_subtree_height(&test.value, max_depth)
        .map_err(|error| standard_error(index, path, error.message))?;
    let current = test
        .path
        .resolve(source)
        .map_err(|_| standard_error(index, path, "path is invalid"))?;
    if !json_equal(current, &test.value) {
        return Err(standard_error(index, path, "value did not match"));
    }
    Ok(())
}

fn moved_subtree_height(
    result: &Value,
    from: &json_patch::jsonptr::Pointer,
    target_depth: usize,
    max_depth: usize,
) -> Result<Option<usize>, Error> {
    let source_depth = from.count();
    if target_depth <= source_depth {
        return Ok(None);
    }
    // Missing sources are reported by the standard operation, whose validation
    // order also handles moves into their own descendants.
    from.resolve(result)
        .ok()
        .map(|source| standard_subtree_height(source, max_depth))
        .transpose()
}

fn standard_subtree_height(value: &Value, limit: usize) -> Result<usize, Error> {
    if !value.is_array() && !value.is_object() {
        return Ok(0);
    }
    let mut height = 0;
    let mut pending = vec![(value, 0)];
    while let Some((value, depth)) = pending.pop() {
        if !value.is_array() && !value.is_object() {
            continue;
        }
        if depth >= limit {
            return Err(Error::new("", "JSON nesting exceeds max_depth"));
        }
        height = height.max(depth + 1);
        match value {
            Value::Array(values) => pending.extend(
                values
                    .iter()
                    .filter(|value| value.is_array() || value.is_object())
                    .map(|value| (value, depth + 1)),
            ),
            Value::Object(values) => pending.extend(
                values
                    .values()
                    .filter(|value| value.is_array() || value.is_object())
                    .map(|value| (value, depth + 1)),
            ),
            _ => {}
        }
    }
    Ok(height)
}

/// Mathematical JSON equality used by the standard protocol and comparison reports.
/// Native reversible deltas deliberately retain serde_json's representation semantics.
pub(crate) fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => numbers::numbers_equal(a, b),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| json_equal(a, b)))
        }
        _ => left == right,
    }
}
pub(crate) fn pointer(path: &str, key: impl fmt::Display) -> String {
    let mut result = format!("{path}/{key}");
    let start = path.len() + 1;
    if result.as_bytes()[start..]
        .iter()
        .any(|byte| matches!(byte, b'~' | b'/'))
    {
        let escaped = result[start..].replace('~', "~0").replace('/', "~1");
        result.truncate(start);
        result.push_str(&escaped);
    }
    result
}
