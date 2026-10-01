use crate::{Delta, Error, delta, patch};
use json_patch::{
    AddOperation, MoveOperation, Patch, PatchOperation, RemoveOperation, ReplaceOperation,
    jsonptr::{PointerBuf, Token},
};
use serde_json::{Value, json};

pub(crate) fn export(left: &Value, change: &Delta) -> Result<Patch, Error> {
    let right = patch::apply(left, &change.0, true)?;
    export_known(left, &right, change)
}

// The diff pipeline already knows the target (or its filtered projection).
// Public Delta::to_json_patch still validates its baseline through export above.
pub(crate) fn export_known(left: &Value, right: &Value, change: &Delta) -> Result<Patch, Error> {
    let mut output = Vec::new();
    walk(left, right, &change.0, &PointerBuf::new(), &mut output)?;
    Ok(Patch(output))
}

fn child<'a>(path: &PointerBuf, token: impl Into<Token<'a>>) -> PointerBuf {
    let mut result = path.clone();
    result.push_back(token);
    result
}

// Occupied original/final slots have frequency one. Prefix sums give the
// current sequential rank while point updates move an item between its slots.
// This is the binary indexed tree from Fenwick (1994), DOI 10.1002/spe.4380240306.
struct Ranks(Vec<usize>);

impl Ranks {
    fn new(mut frequencies: Vec<usize>) -> Self {
        for index in 0..frequencies.len() {
            let parent = index | (index + 1);
            if parent < frequencies.len() {
                frequencies[parent] += frequencies[index];
            }
        }
        Self(frequencies)
    }

    fn before(&self, mut end: usize) -> usize {
        let mut rank = 0;
        while end > 0 {
            rank += self.0[end - 1];
            end &= end - 1;
        }
        rank
    }

    fn occupy(&mut self, mut index: usize, occupied: bool) {
        while index < self.0.len() {
            if occupied {
                self.0[index] += 1;
            } else {
                self.0[index] -= 1;
            }
            index |= index + 1;
        }
    }
}

fn walk(
    left: &Value,
    right: &Value,
    change: &Value,
    path: &PointerBuf,
    out: &mut Vec<PatchOperation>,
) -> Result<(), Error> {
    match change {
        Value::Array(a) => match a.len() {
            1 => out.push(PatchOperation::Add(AddOperation {
                path: path.clone(),
                value: a[0].clone(),
            })),
            2 => out.push(PatchOperation::Replace(ReplaceOperation {
                path: path.clone(),
                value: a[1].clone(),
            })),
            3 if a[2] == json!(0) => out.push(PatchOperation::Remove(RemoveOperation {
                path: path.clone(),
            })),
            3 if a[2] == json!(2) => out.push(PatchOperation::Replace(ReplaceOperation {
                path: path.clone(),
                value: right.clone(),
            })),
            _ => {
                return Err(Error::new(
                    path.as_str(),
                    "invalid delta in JSON Patch export",
                ));
            }
        },
        Value::Object(map) if delta::is_array(map) => {
            let source = left
                .as_array()
                .ok_or_else(|| Error::new(path.as_str(), "array delta requires an array"))?;
            let target = right
                .as_array()
                .ok_or_else(|| Error::new(path.as_str(), "array target is missing"))?;
            let parts = delta::parts(map, path.as_str())?;
            let mut moved = vec![false; source.len()];
            let mut removed = vec![false; source.len()];
            for &old in parts.removals.keys() {
                removed[old] = true;
            }
            for &old in parts.moves.values() {
                moved[old] = true;
            }
            for (&old, change) in parts.removals.iter().rev() {
                if change[2] == json!(0) {
                    out.push(PatchOperation::Remove(RemoveOperation {
                        path: child(path, old),
                    }));
                }
            }
            let mut remaining = (0..source.len()).filter(|&old| !removed[old]);
            let mut desired = Vec::new();
            let mut old_at_target = vec![None; target.len()];
            for (&new, &old) in &parts.moves {
                old_at_target[new] = Some(old);
            }
            let mut additions = parts.additions.keys().copied().peekable();
            for (new, old_slot) in old_at_target.iter_mut().enumerate() {
                if additions.peek() == Some(&new) {
                    additions.next();
                    continue;
                }
                let old = old_slot
                    .as_ref()
                    .copied()
                    .or_else(|| remaining.next())
                    .ok_or_else(|| Error::new(path.as_str(), "invalid array mapping"))?;
                *old_slot = Some(old);
                desired.push(old);
            }
            if !parts.moves.is_empty() {
                // Reserve a final slot for every moved item immediately before its
                // next survivor, or after all originals if no survivor follows.
                // Survivors are already in original order. Thus the final occupied
                // slots spell `desired` without a mutable Vec or an order-stat tree.
                let mut originals = vec![0; source.len()];
                let mut finals = vec![0; source.len()];
                let mut frequencies = Vec::with_capacity(desired.len() + parts.moves.len());
                let mut cursor = 0;
                for old in 0..source.len() {
                    if removed[old] && !moved[old] {
                        continue;
                    }
                    if !moved[old] {
                        while desired.get(cursor) != Some(&old) {
                            let pending = desired
                                .get(cursor)
                                .copied()
                                .filter(|&i| moved[i])
                                .ok_or_else(|| {
                                    Error::new(path.as_str(), "inconsistent survivor order")
                                })?;
                            finals[pending] = frequencies.len();
                            frequencies.push(0);
                            cursor += 1;
                        }
                        cursor += 1;
                    }
                    originals[old] = frequencies.len();
                    frequencies.push(1);
                }
                for &old in &desired[cursor..] {
                    if !moved[old] {
                        return Err(Error::new(path.as_str(), "inconsistent survivor order"));
                    }
                    finals[old] = frequencies.len();
                    frequencies.push(0);
                }
                let mut ranks = Ranks::new(frequencies);
                for &old in desired.iter().rev().filter(|&&old| moved[old]) {
                    let from = ranks.before(originals[old]);
                    ranks.occupy(originals[old], false);
                    let to = ranks.before(finals[old]);
                    ranks.occupy(finals[old], true);
                    if from != to {
                        out.push(PatchOperation::Move(MoveOperation {
                            from: child(path, from),
                            path: child(path, to),
                        }));
                    }
                }
            }
            for (&new, value) in &parts.additions {
                out.push(PatchOperation::Add(AddOperation {
                    path: child(path, new),
                    value: (*value).clone(),
                }));
            }
            for (&new, change) in &parts.changes {
                let old = old_at_target[new]
                    .ok_or_else(|| Error::new(path.as_str(), "cannot modify an added item"))?;
                walk(&source[old], &target[new], change, &child(path, new), out)?;
            }
        }
        Value::Object(map) => {
            for (key, change) in map {
                walk(
                    left.get(key).unwrap_or(&Value::Null),
                    right.get(key).unwrap_or(&Value::Null),
                    change,
                    &child(path, key.as_str()),
                    out,
                )?;
            }
        }
        _ => {
            return Err(Error::new(
                path.as_str(),
                "invalid delta in JSON Patch export",
            ));
        }
    }
    Ok(())
}
