use crate::Error;
use num_bigint::BigInt;
use serde_json::Number;
use std::{borrow::Cow, cmp::Ordering, hash::Hash};

/// A finite decimal value: signed significant digits times ten to `exponent`.
/// Keeping the exponent symbolic avoids expanding numbers such as 1e1000000000.
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct NumberKey<'a> {
    negative: bool,
    digits: Cow<'a, str>,
    exponent: BigInt,
}

impl NumberKey<'_> {
    pub(crate) fn into_owned(self) -> NumberKey<'static> {
        NumberKey {
            negative: self.negative,
            digits: Cow::Owned(self.digits.into_owned()),
            exponent: self.exponent,
        }
    }
}

pub(crate) fn numbers_equal(left: &Number, right: &Number) -> bool {
    if left == right {
        return true;
    }
    // Common integral values require neither canonical digit allocation nor BigInt.
    if let (Some(left), Some(right)) = (left.as_i128(), right.as_i128()) {
        return left == right;
    }
    if let (Some(left), Some(right)) = (left.as_u128(), right.as_u128()) {
        return left == right;
    }
    number_key(left) == number_key(right)
}

pub(crate) fn number_key(number: &Number) -> NumberKey<'_> {
    match number_text(number) {
        Cow::Borrowed(text) => number_key_text(text),
        Cow::Owned(text) => number_key_text(&text).into_owned(),
    }
}

pub(crate) fn number_text(number: &Number) -> Cow<'_, str> {
    #[cfg(feature = "exact-numbers")]
    {
        Cow::Borrowed(number.as_str())
    }
    #[cfg(not(feature = "exact-numbers"))]
    {
        Cow::Owned(number.to_string())
    }
}

fn number_key_text(text: &str) -> NumberKey<'_> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, text),
    };
    let (mantissa, exponent_text) = match unsigned.find(['e', 'E']) {
        Some(index) => (&unsigned[..index], Some(&unsigned[index + 1..])),
        None => (unsigned, None),
    };
    let (digits, fractional_digits) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (Cow::Owned(format!("{whole}{fraction}")), fraction.len()),
        None => (Cow::Borrowed(mantissa), 0),
    };
    let Some(start) = digits.as_bytes().iter().position(|digit| *digit != b'0') else {
        // All signed zero representations are the same value, even with huge exponents.
        return NumberKey {
            negative: false,
            digits: Cow::Borrowed("0"),
            exponent: BigInt::from(0),
        };
    };
    let end = digits.trim_end_matches('0').len();
    let trailing_zeros = digits.len() - end;
    let digits = match digits {
        Cow::Borrowed(digits) => Cow::Borrowed(&digits[start..end]),
        Cow::Owned(mut digits) => {
            digits.truncate(end);
            digits.replace_range(..start, "");
            Cow::Owned(digits)
        }
    };
    // Number's validated JSON grammar guarantees a signed decimal exponent.
    let offset = if trailing_zeros >= fractional_digits {
        BigInt::from(trailing_zeros - fractional_digits)
    } else {
        -BigInt::from(fractional_digits - trailing_zeros)
    };
    let exponent = match exponent_text {
        None => offset,
        Some(exponent) => {
            BigInt::parse_bytes(exponent.as_bytes(), 10).expect("valid JSON number exponent")
                + &offset
        }
    };
    NumberKey {
        negative,
        digits,
        exponent,
    }
}

/// Tolerances are the exact decimal values serialized from the validated f64 options.
/// Loops are bounded by stored coefficient digits; exponent gaps stay symbolic.
pub(crate) fn within_tolerance(
    left: &Number,
    right: &Number,
    absolute: f64,
    relative: f64,
    mut spend: impl FnMut(usize) -> Result<(), Error>,
) -> Result<bool, Error> {
    if left == right {
        return Ok(true);
    }
    spend(number_text(left).len())?;
    spend(number_text(right).len())?;
    let left = number_key(left);
    let right = number_key(right);
    if left == right {
        return Ok(true);
    }
    if absolute > 0.0 {
        let number = Number::from_f64(absolute).expect("validated finite tolerance");
        let tolerance = number_key(&number);
        if difference_within(&left, &right, &tolerance, &mut spend)? {
            return Ok(true);
        }
    }
    if relative > 0.0 {
        let number = Number::from_f64(relative).expect("validated finite tolerance");
        let tolerance = number_key(&number);
        let maximum = if magnitude_cmp(&left, &right, &mut spend)? == Ordering::Less {
            &right
        } else {
            &left
        };
        let threshold = multiply_magnitudes(maximum, &tolerance, &mut spend)?;
        return difference_within(&left, &right, &threshold, &mut spend);
    }
    Ok(false)
}

fn magnitude_cmp(
    left: &NumberKey<'_>,
    right: &NumberKey<'_>,
    spend: &mut impl FnMut(usize) -> Result<(), Error>,
) -> Result<Ordering, Error> {
    match (left.digits == "0", right.digits == "0") {
        (true, true) => return Ok(Ordering::Equal),
        (true, false) => return Ok(Ordering::Less),
        (false, true) => return Ok(Ordering::Greater),
        (false, false) => {}
    }
    let left_order = &left.exponent + BigInt::from(left.digits.len());
    let right_order = &right.exponent + BigInt::from(right.digits.len());
    let ordering = left_order.cmp(&right_order);
    if ordering != Ordering::Equal {
        return Ok(ordering);
    }
    let left = left.digits.as_bytes();
    let right = right.digits.as_bytes();
    for i in 0..left.len().max(right.len()) {
        spend(1)?;
        let ordering = left
            .get(i)
            .unwrap_or(&b'0')
            .cmp(right.get(i).unwrap_or(&b'0'));
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn multiply_magnitudes(
    left: &NumberKey<'_>,
    right: &NumberKey<'_>,
    spend: &mut impl FnMut(usize) -> Result<(), Error>,
) -> Result<NumberKey<'static>, Error> {
    let left_digits = left.digits.as_bytes();
    let right_digits = right.digits.as_bytes();
    // The right coefficient comes from a finite f64's shortest decimal (at most
    // 17 significant digits). Charge all schoolbook work before allocating.
    for _ in left_digits {
        spend(right_digits.len())?;
    }
    let mut product = vec![0u8; left_digits.len() + right_digits.len()];
    for (i, left) in left_digits.iter().rev().enumerate() {
        let mut carry = 0u16;
        for (j, right) in right_digits.iter().rev().enumerate() {
            let position = i + j;
            let total = u16::from(product[position])
                + u16::from(*left - b'0') * u16::from(*right - b'0')
                + carry;
            product[position] = (total % 10) as u8;
            carry = total / 10;
        }
        product[i + right_digits.len()] = carry as u8;
    }
    while product.len() > 1 && product.last() == Some(&0) {
        product.pop();
    }
    let mut digits: String = product
        .iter()
        .rev()
        .map(|digit| char::from(*digit + b'0'))
        .collect();
    let end = digits.trim_end_matches('0').len();
    let trailing = digits.len() - end;
    let exponent = &left.exponent + &right.exponent + BigInt::from(trailing);
    digits.truncate(end);
    Ok(NumberKey {
        negative: false,
        digits: Cow::Owned(digits),
        exponent,
    })
}

fn difference_within(
    left: &NumberKey<'_>,
    right: &NumberKey<'_>,
    threshold: &NumberKey<'_>,
    spend: &mut impl FnMut(usize) -> Result<(), Error>,
) -> Result<bool, Error> {
    if let Some(within) = small_difference_within(left, right, threshold, spend)? {
        return Ok(within);
    }
    let terms = if left.negative != right.negative {
        // Opposite signs: |left-right| = |left|+|right|.
        [(left, false), (right, false), (threshold, true)]
    } else if magnitude_cmp(left, right, spend)? == Ordering::Less {
        [(right, false), (left, true), (threshold, true)]
    } else {
        [(left, false), (right, true), (threshold, true)]
    };
    Ok(sparse_sum_sign(terms, spend)? != Ordering::Greater)
}

/// Exact bounded arithmetic for ordinary decimals. Every scale, subtraction
/// and absolute value is checked; values outside this representation use the
/// same sparse decimal algorithm without rounding.
fn small_difference_within(
    left: &NumberKey<'_>,
    right: &NumberKey<'_>,
    threshold: &NumberKey<'_>,
    spend: &mut impl FnMut(usize) -> Result<(), Error>,
) -> Result<Option<bool>, Error> {
    let values = [left, right, threshold];
    if values.iter().any(|value| value.digits.len() > 38) {
        return Ok(None);
    }
    let exponents = values.map(|value| i64::try_from(&value.exponent).ok());
    let [Some(a), Some(b), Some(t)] = exponents else {
        return Ok(None);
    };
    let minimum = a.min(b).min(t);
    let offsets = [a, b, t].map(|exponent| exponent.checked_sub(minimum));
    let [Some(a), Some(b), Some(t)] = offsets else {
        return Ok(None);
    };
    if a > 38 || b > 38 || t > 38 {
        return Ok(None);
    }
    spend(values.iter().map(|value| value.digits.len()).sum())?;
    let scale = |value: &NumberKey<'_>, offset: i64| {
        let coefficient = value.digits.parse::<i128>().ok()?;
        let power = 10_i128.checked_pow(offset as u32)?;
        let scaled = coefficient.checked_mul(power)?;
        Some(if value.negative { -scaled } else { scaled })
    };
    let [Some(a), Some(b), Some(threshold)] =
        [scale(left, a), scale(right, b), scale(threshold, t)]
    else {
        return Ok(None);
    };
    Ok(a.checked_sub(b)
        .and_then(i128::checked_abs)
        .map(|distance| distance <= threshold))
}

fn sparse_sum_sign(
    terms: [(&NumberKey<'_>, bool); 3],
    spend: &mut impl FnMut(usize) -> Result<(), Error>,
) -> Result<Ordering, Error> {
    struct Cursor<'a> {
        digits: &'a [u8],
        position: BigInt,
        remaining: usize,
        negative: bool,
    }
    let mut cursors = terms.map(|(value, negative)| Cursor {
        digits: value.digits.as_bytes(),
        position: value.exponent.clone(),
        remaining: if value.digits == "0" {
            0
        } else {
            value.digits.len()
        },
        negative,
    });
    let mut expected: Option<BigInt> = None;
    let mut carry = 0i16;
    let mut nonzero = false;
    while let Some(next) = cursors
        .iter()
        .filter(|cursor| cursor.remaining != 0)
        .map(|cursor| &cursor.position)
        .min()
    {
        if let Some(expected) = &mut expected {
            if next > expected {
                // At most three terms give carry in -2..=2. Across any nonempty
                // zero gap its next carry is exactly -1 or 0; further zeroes
                // cannot change the final sign. Skip the entire symbolic gap.
                nonzero |= carry != 0;
                carry = carry.div_euclid(10);
                expected.clone_from(next);
            }
        } else {
            expected = Some(next.clone());
        }
        let Some(expected) = &mut expected else { break };
        let mut total = carry;
        for cursor in &mut cursors {
            if cursor.remaining != 0 && cursor.position == *expected {
                spend(1)?;
                let digit = i16::from(cursor.digits[cursor.remaining - 1] - b'0');
                total += if cursor.negative { -digit } else { digit };
                cursor.remaining -= 1;
                cursor.position += 1u8;
            }
        }
        nonzero |= total.rem_euclid(10) != 0;
        carry = total.div_euclid(10);
        *expected += 1u8;
    }
    Ok(if carry < 0 {
        Ordering::Less
    } else if carry > 0 || nonzero {
        Ordering::Greater
    } else {
        Ordering::Equal
    })
}

#[cfg(test)]
mod tests {
    use super::{number_key, numbers_equal};
    use serde_json::Number;
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };

    #[test]
    fn equal_decimal_values_have_equal_hashes() {
        for (left, right) in [
            ("1", "1.0"),
            ("0", "-0.0"),
            ("1000", "1E+3"),
            ("-123.4500", "-12345e-2"),
            #[cfg(feature = "exact-numbers")]
            ("18446744073709551616", "1.8446744073709551616e19"),
            #[cfg(feature = "exact-numbers")]
            ("1e10000", "10e9999"),
        ] {
            let left: Number = serde_json::from_str(left).unwrap();
            let right: Number = serde_json::from_str(right).unwrap();
            assert!(numbers_equal(&left, &right));
            let mut left_hash = DefaultHasher::new();
            let mut right_hash = DefaultHasher::new();
            number_key(&left).hash(&mut left_hash);
            number_key(&right).hash(&mut right_hash);
            assert_eq!(left_hash.finish(), right_hash.finish());
        }
    }
}
