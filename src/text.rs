//! UTF-16 coordinates and UTF-8 URI text, as used by JavaScript diff-match-patch.
use crate::Error;
use imara_diff::{Algorithm, Diff, Hunk, InternedInput, Interner};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

const ENCODE: &AsciiSet = &CONTROLS
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'|');

#[derive(Clone)]
pub(crate) struct TextPatch {
    start_old: usize,
    start_new: usize,
    len_old: usize,
    len_new: usize,
    ops: Vec<(char, String)>,
}

fn len(text: &str) -> usize {
    if text.is_ascii() {
        return text.len();
    }
    if text.len() < 32 {
        return text.encode_utf16().count();
    }
    // Valid UTF-8 has one scalar per non-continuation byte, plus one extra
    // UTF-16 unit per four-byte leading byte. Classify eight byte lanes at a
    // time, independently of native endianness, without decoding each scalar.
    const HIGH: u64 = 0x8080_8080_8080_8080;
    const SECOND: u64 = 0x4040_4040_4040_4040;
    let mut chunks = text.as_bytes().chunks_exact(8);
    let mut count = 0usize;
    for chunk in chunks.by_ref() {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(chunk);
        let word = u64::from_ne_bytes(bytes);
        let continuation = word & !((word & SECOND) << 1) & HIGH;
        let astral = word & (word << 1) & (word << 2) & (word << 3) & HIGH;
        count += 8 - continuation.count_ones() as usize + astral.count_ones() as usize;
    }
    for byte in chunks.remainder() {
        count += usize::from(byte & 0xC0 != 0x80) + usize::from(*byte >= 0xF0);
    }
    count
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let mut matched = 0;
    // Slice equality uses the standard library's optimized byte comparison.
    // The scalar tail sees at most one mismatching or partial block.
    for (a, b) in a.chunks_exact(256).zip(b.chunks_exact(256)) {
        if a != b {
            break;
        }
        matched += 256;
    }
    matched
        + a[matched..]
            .iter()
            .zip(&b[matched..])
            .take_while(|(a, b)| a == b)
            .count()
}

fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    let mut matched = 0;
    for (a, b) in a.rchunks_exact(256).zip(b.rchunks_exact(256)) {
        if a != b {
            break;
        }
        matched += 256;
    }
    matched
        + a[..a.len() - matched]
            .iter()
            .rev()
            .zip(b[..b.len() - matched].iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
}
fn coord(start: usize, length: usize) -> String {
    if length == 0 {
        format!("{start},0")
    } else if length == 1 {
        (start + 1).to_string()
    } else {
        format!("{},{}", start + 1, length)
    }
}
fn render(patches: &[TextPatch]) -> String {
    let mut result = String::new();
    for patch in patches {
        result.push_str(&format!(
            "@@ -{} +{} @@\n",
            coord(patch.start_old, patch.len_old),
            coord(patch.start_new, patch.len_new)
        ));
        for (op, text) in &patch.ops {
            result.push(*op);
            result.push_str(&utf8_percent_encode(text, ENCODE).to_string());
            result.push('\n');
        }
    }
    result
}
fn parse_coord(coord: &str, path: &str) -> Result<(usize, usize), Error> {
    let (start, count) = coord
        .split_once(',')
        .map_or((coord, None), |(s, c)| (s, Some(c)));
    let start = start
        .parse::<usize>()
        .map_err(|_| Error::new(path, "invalid text patch coordinate"))?;
    let count = count.map_or(Ok(1), |s| {
        s.parse::<usize>()
            .map_err(|_| Error::new(path, "invalid text patch length"))
    })?;
    if count == 0 {
        Ok((start, 0))
    } else {
        Ok((
            start
                .checked_sub(1)
                .ok_or_else(|| Error::new(path, "text patch coordinate must start at one"))?,
            count,
        ))
    }
}

fn decode_uri(encoded: &str, path: &str) -> Result<String, Error> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            decoded.push(bytes[i]);
            i += 1;
            continue;
        }
        let digits = bytes
            .get(i + 1..i + 3)
            .filter(|digits| digits.iter().all(u8::is_ascii_hexdigit))
            .ok_or_else(|| Error::new(path, "invalid URI escape in text patch"))?;
        let hex = |b: u8| {
            if b.is_ascii_digit() {
                b - b'0'
            } else {
                b.to_ascii_lowercase() - b'a' + 10
            }
        };
        let byte = hex(digits[0]) * 16 + hex(digits[1]);
        // JavaScript decodeURI preserves escapes for URI syntax, including their
        // original hex case. decodeURIComponent would have different semantics.
        if b";/?:@&=+$,#".contains(&byte) {
            decoded.extend_from_slice(&bytes[i..i + 3]);
        } else {
            decoded.push(byte);
        }
        i += 3;
    }
    String::from_utf8(decoded).map_err(|_| Error::new(path, "text patch is not valid Unicode"))
}

pub(crate) fn parse(text: &str, path: &str) -> Result<Vec<TextPatch>, Error> {
    let mut result = Vec::new();
    let mut lines = text.split_terminator('\n').peekable();
    while let Some(line) = lines.next() {
        let header = line
            .strip_prefix("@@ -")
            .and_then(|s| s.strip_suffix(" @@"))
            .ok_or_else(|| Error::new(path, "invalid text patch header"))?;
        let (old, new) = header
            .split_once(" +")
            .ok_or_else(|| Error::new(path, "invalid text patch header"))?;
        let (start_old, len_old) = parse_coord(old, path)?;
        let (start_new, len_new) = parse_coord(new, path)?;
        let mut patch = TextPatch {
            start_old,
            start_new,
            len_old,
            len_new,
            ops: Vec::new(),
        };
        let (mut actual_old, mut actual_new) = (0usize, 0usize);
        while lines.peek().is_some_and(|l| !l.starts_with("@@ ")) {
            let line = lines
                .next()
                .ok_or_else(|| Error::new(path, "missing text operation"))?;
            let Some(op) = line.chars().next() else {
                return Err(Error::new(path, "empty text patch operation"));
            };
            if !matches!(op, ' ' | '+' | '-') {
                return Err(Error::new(path, "invalid text patch operation"));
            }
            let value = decode_uri(&line[1..], path)?;
            let length = len(&value);
            if op != '+' {
                actual_old = actual_old
                    .checked_add(length)
                    .ok_or_else(|| Error::new(path, "text patch length overflow"))?;
            }
            if op != '-' {
                actual_new = actual_new
                    .checked_add(length)
                    .ok_or_else(|| Error::new(path, "text patch length overflow"))?;
            }
            patch.ops.push((op, value));
        }
        // @dmsnell/diff-match-patch 1.1.0 repairs surrogate boundaries after
        // counting context. Its two declared lengths can therefore share an
        // offset. The edit length must still agree; operations define context.
        if actual_new as i128 - actual_old as i128 != len_new as i128 - len_old as i128 {
            return Err(Error::new(
                path,
                "text patch header length does not match operations",
            ));
        }
        patch.len_old = actual_old;
        patch.len_new = actual_new;
        start_old
            .checked_add(len_old.max(actual_old))
            .and_then(|_| start_new.checked_add(len_new.max(actual_new)))
            .ok_or_else(|| Error::new(path, "text patch coordinate overflow"))?;
        result.push(patch);
    }
    Ok(result)
}

// Bound character-level work: imara-diff 0.2's Myers preprocessing repeatedly
// scans common tokens on a small alphabet. Larger replacements may be coarser,
// but still describe the exact edit without a timeout or a partial result.
const MAX_DIFF_TOKENS: usize = 4096;

fn character_hunks(a: &[char], b: &[char]) -> Vec<Hunk> {
    let head = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let a = &a[head..a.len() - tail];
    let b = &b[head..b.len() - tail];
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    if a.is_empty() || b.is_empty() || a.len() + b.len() > MAX_DIFF_TOKENS {
        return vec![Hunk {
            before: head as u32..(head + a.len()) as u32,
            after: head as u32..(head + b.len()) as u32,
        }];
    }
    let mut interner = Interner::new(a.len() + b.len());
    let before = a.iter().map(|c| interner.intern(*c)).collect();
    let after = b.iter().map(|c| interner.intern(*c)).collect();
    let input = InternedInput {
        before,
        after,
        interner,
    };
    Diff::compute(Algorithm::Histogram, &input)
        .hunks()
        .map(|h| Hunk {
            before: h.before.start + head as u32..h.before.end + head as u32,
            after: h.after.start + head as u32..h.after.end + head as u32,
        })
        .collect()
}

fn line_boundaries(text: &str, group: usize) -> Vec<usize> {
    let mut boundaries = vec![0];
    let mut lines = 0;
    let mut newline_at = |byte| {
        lines += 1;
        if lines == group {
            boundaries.push(byte + 1);
            lines = 0;
        }
    };
    const LOW: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let mut chunks = text.as_bytes().chunks_exact(8);
    for (block, chunk) in chunks.by_ref().enumerate() {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(chunk);
        let different = u64::from_le_bytes(bytes) ^ 0x0a0a_0a0a_0a0a_0a0a;
        // Each low-seven-bit lane adds at most 254, so no carry reaches
        // its neighbor. Only exact zero lanes retain a high bit.
        let mut newlines = !((different & LOW).wrapping_add(LOW) | different | LOW) & HIGH;
        while newlines != 0 {
            newline_at(block * 8 + newlines.trailing_zeros() as usize / 8);
            newlines &= newlines - 1;
        }
    }
    let remainder_start = text.len() - chunks.remainder().len();
    for (byte, value) in chunks.remainder().iter().enumerate() {
        if *value == b'\n' {
            newline_at(remainder_start + byte);
        }
    }
    if boundaries.last() != Some(&text.len()) {
        boundaries.push(text.len());
    }
    boundaries
}

fn text_hunks(a: &[char], b: &[char], old: &str, new: &str) -> Vec<Hunk> {
    if a.len() + b.len() <= MAX_DIFF_TOKENS {
        return character_hunks(a, b);
    }
    // As in DMP's line mode, find unchanged lines before refining changes into
    // character edits. Group very large line counts to bound the token diff,
    // including documents containing only a few repeated lines.
    let lines = a.iter().chain(b).filter(|c| **c == '\n').count() + 2;
    // Rounding the two documents separately can add one token to their sum.
    let group = lines.div_ceil(MAX_DIFF_TOKENS - 1);
    let before_lines = line_boundaries(old, group);
    let after_lines = line_boundaries(new, group);
    let mut interner = Interner::new(before_lines.len() + after_lines.len());
    let before = before_lines
        .windows(2)
        .map(|w| interner.intern(&old[w[0]..w[1]]))
        .collect();
    let after = after_lines
        .windows(2)
        .map(|w| interner.intern(&new[w[0]..w[1]]))
        .collect();
    let input = InternedInput {
        before,
        after,
        interner,
    };
    let mut before_byte = 0;
    let mut after_byte = 0;
    let mut before_chars = 0;
    let mut after_chars = 0;
    Diff::compute(Algorithm::Histogram, &input)
        .hunks()
        .flat_map(|h| {
            // Line hunks are ordered and disjoint in both documents. Count
            // character offsets only when refining a changed line range.
            let start_byte_a = before_lines[h.before.start as usize];
            let end_byte_a = before_lines[h.before.end as usize];
            before_chars += old[before_byte..start_byte_a].chars().count();
            let start_a = before_chars;
            before_chars += old[start_byte_a..end_byte_a].chars().count();
            before_byte = end_byte_a;
            let start_byte_b = after_lines[h.after.start as usize];
            let end_byte_b = after_lines[h.after.end as usize];
            after_chars += new[after_byte..start_byte_b].chars().count();
            let start_b = after_chars;
            after_chars += new[start_byte_b..end_byte_b].chars().count();
            after_byte = end_byte_b;
            character_hunks(&a[start_a..before_chars], &b[start_b..after_chars])
                .into_iter()
                .map(move |h| Hunk {
                    before: h.before.start + start_a as u32..h.before.end + start_a as u32,
                    after: h.after.start + start_b as u32..h.after.end + start_b as u32,
                })
        })
        .collect()
}

pub(crate) fn diff(old: &str, new: &str, path: &str) -> Result<String, Error> {
    // Trim on borrowed UTF-8 first: sparse long-text edits should not allocate
    // character vectors and UTF-16 prefix sums for the unchanged document.
    let mut head = common_prefix(old.as_bytes(), new.as_bytes());
    while !old.is_char_boundary(head) || !new.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = common_suffix(&old.as_bytes()[head..], &new.as_bytes()[head..]);
    while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
        tail -= 1;
    }
    let window_start = old[..head]
        .char_indices()
        .rev()
        .nth(3)
        .map_or(0, |(i, _)| i);
    let context_end = old[old.len() - tail..]
        .char_indices()
        .nth(4)
        .map_or(tail, |(i, _)| i);
    let changed_old = &old[head..old.len() - tail];
    let changed_new = &new[head..new.len() - tail];
    let a: Vec<char> = old[window_start..old.len() - tail + context_end]
        .chars()
        .collect();
    let b: Vec<char> = new[window_start..new.len() - tail + context_end]
        .chars()
        .collect();
    if a.len()
        .checked_add(b.len())
        .is_none_or(|n| n >= i32::MAX as usize)
    {
        return Err(Error::new(path, "text exceeds sequence index capacity"));
    }
    // Sparse edits in long strings should not intern the unchanged prefix/tail.
    let head = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let changed_a = &a[head..a.len() - tail];
    let changed_b = &b[head..b.len() - tail];
    let hunks: Vec<_> = text_hunks(changed_a, changed_b, changed_old, changed_new)
        .into_iter()
        .map(|mut h| {
            h.before.start += head as u32;
            h.before.end += head as u32;
            h.after.start += head as u32;
            h.after.end += head as u32;
            h
        })
        .collect();
    let mut result = Vec::new();
    let mut i = 0;
    // Measure the unchanged document prefix once; then count target coordinates
    // only as far as each header needs. ASCII prefixes require no UTF-16 decoding.
    let prefix = &old[..window_start];
    let mut offset = len(prefix);
    let mut coordinate_cursor = 0;
    while i < hunks.len() {
        let first = &hunks[i];
        let start_a = (first.before.start as usize).saturating_sub(4);
        let start_b = (first.after.start as usize).saturating_sub(4);
        let mut cursor_a = start_a;
        let mut cursor_b;
        let mut ops: Vec<(char, String)> = Vec::new();
        loop {
            let h = &hunks[i];
            let old_start = h.before.start as usize;
            let new_start = h.after.start as usize;
            if cursor_a < old_start {
                ops.push((' ', a[cursor_a..old_start].iter().collect()));
            }
            if old_start < h.before.end as usize {
                ops.push(('-', a[old_start..h.before.end as usize].iter().collect()));
            }
            if new_start < h.after.end as usize {
                ops.push(('+', b[new_start..h.after.end as usize].iter().collect()));
            }
            cursor_a = h.before.end as usize;
            cursor_b = h.after.end as usize;
            i += 1;
            if i == hunks.len() || hunks[i].before.start as usize - cursor_a > 8 {
                break;
            }
        }
        let tail = 4.min(a.len() - cursor_a).min(b.len() - cursor_b);
        if tail > 0 {
            ops.push((' ', a[cursor_a..cursor_a + tail].iter().collect()));
        }
        let len_old = ops
            .iter()
            .filter(|(op, _)| *op != '+')
            .map(|(_, s)| len(s))
            .sum();
        let len_new = ops
            .iter()
            .filter(|(op, _)| *op != '-')
            .map(|(_, s)| len(s))
            .sum();
        // Hunks are ordered in target coordinates. Count only the next header's
        // UTF-16 prefix instead of materializing a coordinate for every character.
        offset += b[coordinate_cursor..start_b]
            .iter()
            .map(|c| c.len_utf16())
            .sum::<usize>();
        coordinate_cursor = start_b;
        // Earlier patches have already been applied: both headers use the target prefix.
        result.push(TextPatch {
            start_old: offset,
            start_new: offset,
            len_old,
            len_new,
            ops,
        });
    }
    Ok(render(&result))
}

pub(crate) fn apply(source: &str, text: &str, path: &str) -> Result<String, Error> {
    let patches = parse(text, path)?;
    let mut result: Vec<u16> = source.encode_utf16().collect();
    for patch in patches {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for (op, s) in patch.ops {
            if op != '+' {
                old.extend(s.encode_utf16());
            }
            if op != '-' {
                new.extend(s.encode_utf16());
            }
        }
        let end = patch
            .start_new
            .checked_add(old.len())
            .ok_or_else(|| Error::new(path, "text patch index overflow"))?;
        if result.get(patch.start_new..end) != Some(old.as_slice()) {
            return Err(Error::new(
                path,
                "text patch context does not match at expected position",
            ));
        }
        result.splice(patch.start_new..end, new);
    }
    String::from_utf16(&result).map_err(|_| Error::new(path, "text patch produced invalid Unicode"))
}

// Locate a short Unicode anchor in a bounded UTF-16 window. Semi-global edit
// distance permits an insertion/deletion/substitution in the context while
// retaining the start and end of the actual substring. Only two rows are kept.
fn anchor_match(
    source: &[char],
    coords: &[usize],
    anchor: &[char],
    expected: i128,
    options: &crate::TextPatchOptions,
) -> Option<(usize, usize)> {
    let distance = options.max_distance as i128;
    let low = expected.saturating_sub(distance).max(0);
    let high = expected.saturating_add(distance);
    if high < 0 || low > *coords.last()? as i128 {
        return None;
    }
    let first = coords.partition_point(|p| (*p as i128) < low);
    let last = coords
        .partition_point(|p| (*p as i128) <= high)
        .saturating_sub(1);
    if first > last {
        return None;
    }
    if let Ok(expected) = usize::try_from(expected) {
        if let Ok(at) = coords.binary_search(&expected) {
            if source.get(at..at.saturating_add(anchor.len())) == Some(anchor) {
                return Some((at, at + anchor.len()));
            }
        }
    }
    let budget = (anchor.len() as f64 * options.max_error_ratio).floor() as usize;
    let end = last
        .saturating_add(anchor.len())
        .saturating_add(budget)
        .min(source.len());
    let window = &source[first..end];
    let invalid = anchor.len() + 1;
    let mut previous: Vec<_> = (0..=window.len())
        .map(|j| if first + j <= last { 0 } else { invalid })
        .collect();
    let mut origins: Vec<_> = (first..=end).collect();
    let mut current = vec![invalid; window.len() + 1];
    let mut next_origins = vec![first; window.len() + 1];
    let rank =
        |cost: usize, start: usize| (cost, (coords[start] as i128 - expected).unsigned_abs());
    for (i, a) in anchor.iter().enumerate() {
        current[0] = i + 1;
        next_origins[0] = first;
        for (j, b) in window.iter().enumerate() {
            let candidates = [
                (previous[j] + usize::from(a != b), origins[j]),
                (previous[j + 1] + 1, origins[j + 1]),
                (current[j] + 1, next_origins[j]),
            ];
            let &(cost, origin) = candidates
                .iter()
                .min_by_key(|(cost, start)| rank(*cost, *start))?;
            current[j + 1] = cost.min(invalid);
            next_origins[j + 1] = origin;
        }
        std::mem::swap(&mut previous, &mut current);
        std::mem::swap(&mut origins, &mut next_origins);
    }
    let mut best: Option<(f64, usize, u128, usize, usize, usize)> = None;
    for (j, errors) in previous.into_iter().enumerate() {
        let start = origins[j];
        let displacement = (coords[start] as i128 - expected).unsigned_abs();
        if errors > budget || displacement > options.max_distance as u128 || start > first + j {
            continue;
        }
        let score = errors as f64 / anchor.len().max(1) as f64
            + displacement as f64 / (options.max_distance as f64 + 1.0);
        // A trailing substitution and deletion can have the same edit cost.
        // Prefer the span closest to the expected size, rather than a shorter
        // match that leaves the substituted character outside the replacement.
        let end = first + j;
        let length_gap = (end - start).abs_diff(anchor.len());
        let candidate = (score, errors, displacement, length_gap, start, end);
        if best.as_ref().is_none_or(|old| candidate < *old) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, _, _, start, end)| (start, end))
}

// Map boundaries of the expected old fragment into the nearby actual fragment.
// Unchanged runs remain exact; changed runs are mapped monotonically so extra
// baseline context is preserved rather than replaced by the patch's old text.
fn alignment(old: &[char], actual: &[char], path: &str) -> Result<(Vec<usize>, usize), Error> {
    if old
        .len()
        .checked_add(actual.len())
        .is_none_or(|n| n >= i32::MAX as usize)
    {
        return Err(Error::new(path, "text exceeds sequence index capacity"));
    }
    let mut interner = Interner::new(old.len() + actual.len());
    let before = old.iter().map(|c| interner.intern(*c)).collect();
    let after = actual.iter().map(|c| interner.intern(*c)).collect();
    let input = InternedInput {
        before,
        after,
        interner,
    };
    let differences = Diff::compute(Algorithm::Histogram, &input);
    let mut map = vec![0usize; old.len() + 1];
    let (mut before, mut after, mut errors) = (0usize, 0usize, 0usize);
    for hunk in differences.hunks() {
        let start = hunk.before.start as usize;
        for (i, mapped) in map.iter_mut().enumerate().take(start + 1).skip(before) {
            *mapped = after + i - before;
        }
        let old_count = hunk.before.len();
        let new_count = hunk.after.len();
        let new_start = hunk.after.start as usize;
        for (i, mapped) in map
            .iter_mut()
            .enumerate()
            .take(hunk.before.end as usize)
            .skip(start)
        {
            *mapped = new_start + (i - start).min(new_count);
        }
        before = hunk.before.end as usize;
        after = hunk.after.end as usize;
        map[before] = after;
        errors += old_count.max(new_count);
    }
    for (i, mapped) in map.iter_mut().enumerate().skip(before) {
        *mapped = after + i - before;
    }
    Ok((map, errors))
}

// Prefer the complete context when it still exists. The two bounded searches
// include overlapping occurrences on either side of the expected position.
fn exact_fuzzy_match(
    source: &[char],
    coords: &[usize],
    old: &[char],
    expected: i128,
    distance: usize,
) -> Option<(usize, usize)> {
    if old.is_empty() {
        return None;
    }
    if let Ok(expected) = usize::try_from(expected) {
        if let Ok(at) = coords.binary_search(&expected) {
            if source.get(at..at.saturating_add(old.len())) == Some(old) {
                return Some((at, at + old.len()));
            }
        }
    }
    let low = expected.saturating_sub(distance as i128).max(0);
    let high = expected.saturating_add(distance as i128);
    let first = coords.partition_point(|p| (*p as i128) < low);
    let last = coords
        .partition_point(|p| (*p as i128) <= high)
        .checked_sub(1)?;
    if first > last || first > source.len() {
        return None;
    }
    let middle = coords
        .partition_point(|p| (*p as i128) < expected)
        .clamp(first, last);
    let wanted: String = old.iter().collect();
    let left: String = source[first..middle.saturating_add(old.len()).min(source.len())]
        .iter()
        .collect();
    let right: String = source[middle..last.saturating_add(old.len()).min(source.len())]
        .iter()
        .collect();
    let left = left
        .rfind(&wanted)
        .map(|at| first + left[..at].chars().count());
    let right = right
        .find(&wanted)
        .map(|at| middle + right[..at].chars().count());
    [left, right]
        .into_iter()
        .flatten()
        .filter(|at| *at >= first && *at <= last)
        .min_by_key(|at| ((coords[*at] as i128 - expected).unsigned_abs(), *at))
        .map(|at| (at, at + old.len()))
}

// Match edited text with a small Unicode-safe context. Overlapping hunks can
// retain context from an earlier state that no longer occurs in the baseline.
fn trim_fuzzy_context(patch: &mut TextPatch) {
    if let Some((' ', text)) = patch.ops.first_mut() {
        let mut units = 0;
        let at = text
            .char_indices()
            .rev()
            .take_while(|(_, c)| {
                units += c.len_utf16();
                units <= 4
            })
            .last()
            .map_or(text.len(), |(at, _)| at);
        let removed = len(&text[..at]);
        *text = text[at..].to_owned();
        patch.start_old += removed;
        patch.start_new += removed;
        patch.len_old -= removed;
        patch.len_new -= removed;
    }
    if let Some((' ', text)) = patch.ops.last_mut() {
        let mut units = 0;
        let at = text
            .char_indices()
            .take_while(|(_, c)| {
                units += c.len_utf16();
                units <= 4
            })
            .last()
            .map_or(0, |(at, c)| at + c.len_utf8());
        let removed = len(&text[at..]);
        text.truncate(at);
        patch.len_old -= removed;
        patch.len_new -= removed;
    }
}

/// Bounded fuzzy application, intentionally distinct from DMP's match scoring.
/// Protocol coordinates and displacement bounds use UTF-16; edits and matching
/// operate on Unicode scalars. Complete exact context takes priority. For long
/// hunks without an exact match, nonzero-error matching retains at most four
/// UTF-16 units of surrounding context per side, then checks the retained
/// fragment's error ratio. A failed hunk discards the private result.
pub(crate) fn apply_fuzzy(
    source: &str,
    text: &str,
    path: &str,
    options: &crate::TextPatchOptions,
) -> Result<String, Error> {
    let patches = parse(text, path)?;
    let mut result: Vec<char> = source.chars().collect();
    let mut offset = 0i128;
    for mut patch in patches {
        let mut coords = Vec::with_capacity(result.len() + 1);
        coords.push(0usize);
        for c in &result {
            coords.push(coords.last().copied().unwrap_or(0) + c.len_utf16());
        }
        let mut old: Vec<char> = patch
            .ops
            .iter()
            .filter(|(op, _)| *op != '+')
            .flat_map(|(_, s)| s.chars())
            .collect();
        let mut expected = patch.start_new as i128 + offset;
        let exact = exact_fuzzy_match(&result, &coords, &old, expected, options.max_distance);
        if exact.is_none() && patch.len_old > 32 && options.max_error_ratio > 0.0 {
            trim_fuzzy_context(&mut patch);
            old = patch
                .ops
                .iter()
                .filter(|(op, _)| *op != '+')
                .flat_map(|(_, s)| s.chars())
                .collect();
            expected = patch.start_new as i128 + offset;
        }
        let (start, end) = if let Some(found) = exact {
            found
        } else if old.is_empty() {
            let at = usize::try_from(expected)
                .ok()
                .and_then(|p| coords.binary_search(&p).ok())
                .ok_or_else(|| {
                    Error::new(
                        path,
                        "text insertion position does not match a Unicode boundary",
                    )
                })?;
            (at, at)
        } else if old.len() <= 32 {
            anchor_match(&result, &coords, &old, expected, options).ok_or_else(|| {
                Error::new(path, "text patch context not found within fuzzy bounds")
            })?
        } else {
            let (start, head_end) = anchor_match(&result, &coords, &old[..32], expected, options)
                .ok_or_else(|| {
                Error::new(
                    path,
                    "text patch leading context not found within fuzzy bounds",
                )
            })?;
            let tail = &old[old.len() - 32..];
            let tail_units: usize = tail.iter().map(|c| c.len_utf16()).sum();
            let expected_tail = coords[start] as i128 + patch.len_old as i128 - tail_units as i128;
            let (_, end) = anchor_match(&result, &coords, tail, expected_tail, options)
                .filter(|(_, end)| *end >= head_end)
                .ok_or_else(|| {
                    Error::new(
                        path,
                        "text patch trailing context not found within fuzzy bounds",
                    )
                })?;
            (start, end)
        };
        let actual = &result[start..end];
        let (map, errors) = alignment(&old, actual, path)?;
        if errors as f64 > old.len().max(1) as f64 * options.max_error_ratio {
            return Err(Error::new(path, "text patch exceeds max_error_ratio"));
        }
        let mut replacement = Vec::new();
        let (mut old_cursor, mut actual_cursor) = (0usize, 0usize);
        for (op, value) in patch.ops {
            let at = map[old_cursor];
            replacement.extend_from_slice(&actual[actual_cursor..at]);
            actual_cursor = at;
            if op == '+' {
                replacement.extend(value.chars());
                continue;
            }
            old_cursor += value.chars().count();
            let end = map[old_cursor];
            if op == ' ' {
                replacement.extend_from_slice(&actual[actual_cursor..end]);
            }
            actual_cursor = end;
        }
        replacement.extend_from_slice(&actual[actual_cursor..]);
        offset = coords[start] as i128 - patch.start_new as i128;
        result.splice(start..end, replacement);
    }
    Ok(result.into_iter().collect())
}

pub(crate) fn reverse(text: &str, path: &str) -> Result<String, Error> {
    let mut patches = parse(text, path)?;
    // Context from a later patch can overlap an earlier insertion. Undo in
    // reverse chronological order so that earlier context is restored first.
    patches.reverse();
    for patch in &mut patches {
        std::mem::swap(&mut patch.start_old, &mut patch.start_new);
        std::mem::swap(&mut patch.len_old, &mut patch.len_new);
        for (op, _) in &mut patch.ops {
            *op = match *op {
                '+' => '-',
                '-' => '+',
                other => other,
            };
        }
    }
    Ok(render(&patches))
}

#[cfg(test)]
mod tests {
    use super::line_boundaries;

    #[test]
    fn newline_words_match_character_boundaries() {
        let check = |text: &str| {
            for group in [1, 2, 3, 7, 8, 9, 12] {
                let mut expected = vec![0];
                let mut lines = 0;
                for (byte, character) in text.char_indices() {
                    if character == '\n' {
                        lines += 1;
                        if lines == group {
                            expected.push(byte + 1);
                            lines = 0;
                        }
                    }
                }
                if expected.last() != Some(&text.len()) {
                    expected.push(text.len());
                }
                assert_eq!(line_boundaries(text, group), expected);
            }
        };
        // LF next to VT catches the false neighboring zero lane produced
        // by the usual subtract-one mask when a borrow crosses bytes.
        for lane in 0..8 {
            for value in 0..=127 {
                let mut bytes = [b'\n'; 8];
                bytes[lane] = value;
                check(std::str::from_utf8(&bytes).unwrap());
            }
        }
        for prefix in 0..8 {
            for suffix in 0..8 {
                check(&format!(
                    "{}🦀界\n\u{b}\n\0e\u{301}\r\n🚀\n\n{}",
                    "x".repeat(prefix),
                    "y".repeat(suffix)
                ));
            }
            check(&"x".repeat(prefix));
            check(&"\n".repeat(prefix));
        }
    }
}
