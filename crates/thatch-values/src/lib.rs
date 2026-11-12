//! Scratch value semantics, mirroring scratch-vm's `Cast` and the operator blocks.
//!
//! A Scratch value is a number, a string or a boolean. Every block casts its inputs the way
//! scratch-vm does, and those casts follow JavaScript: `Number("0x10")` is 16, `String(1e21)` is
//! `1e+21`, and `"abc" < "b"` compares strings case-insensitively. This crate holds those rules in one
//! place so the compiler (for constant folding) and the runtime (for everything else) agree exactly.
//!
//! Strings here are Rust strings. Scratch strings are JavaScript strings, which are sequences of
//! UTF-16 code units, so `length of` and `letter of` count UTF-16 units. A lone surrogate cannot be
//! stored in a Rust string, so `letter of` on half of an astral character gives U+FFFD instead; that
//! is the one known difference.

use std::cmp::Ordering;

/// A borrowed Scratch value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueRef<'a> {
    Num(f64),
    Str(&'a str),
    Bool(bool),
}

/// Whether `c` is whitespace to JavaScript's `String.prototype.trim` and `Number()`.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `Cast.isWhiteSpace`: whether the string is empty or only whitespace.
pub fn is_whitespace(s: &str) -> bool {
    s.chars().all(is_js_space)
}

/// JavaScript's `Number(string)`: NaN when the string is not a numeric literal.
pub fn js_number(s: &str) -> f64 {
    let s = s.trim_matches(is_js_space);
    if s.is_empty() {
        return 0.0;
    }
    let bytes = s.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            return parse_radix(&s[2..], radix);
        }
    }
    let (sign, rest) = match bytes[0] {
        b'+' => (1.0, &s[1..]),
        b'-' => (-1.0, &s[1..]),
        _ => (1.0, s),
    };
    if rest == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(rest) {
        return f64::NAN;
    }
    // Rust parses the same decimal grammar with correct rounding, and the sign keeps -0 as -0.
    match rest.parse::<f64>() {
        Ok(n) => sign * n,
        Err(_) => f64::NAN,
    }
}

/// Whether `s` is a JavaScript `StrUnsignedDecimalLiteral` other than `Infinity`.
fn is_decimal_literal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let int_digits = count_digits(b, &mut i);
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        frac_digits = count_digits(b, &mut i);
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if count_digits(b, &mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

fn count_digits(b: &[u8], i: &mut usize) -> usize {
    let start = *i;
    while *i < b.len() && b[*i].is_ascii_digit() {
        *i += 1;
    }
    *i - start
}

fn parse_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut n = 0.0f64;
    for c in digits.chars() {
        match c.to_digit(radix) {
            Some(d) => n = n * f64::from(radix) + f64::from(d),
            None => return f64::NAN,
        }
    }
    n
}

/// `Cast.toNumber` for a string: `Number(s)`, with NaN becoming 0.
pub fn str_to_number(s: &str) -> f64 {
    let n = js_number(s);
    if n.is_nan() { 0.0 } else { n }
}

/// `Cast.toNumber` for a number: NaN becomes 0, everything else stays.
pub fn num_to_number(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n }
}

/// `Cast.toNumber`.
pub fn to_number(v: ValueRef<'_>) -> f64 {
    match v {
        ValueRef::Num(n) => num_to_number(n),
        ValueRef::Str(s) => str_to_number(s),
        ValueRef::Bool(b) => f64::from(b),
    }
}

/// JavaScript's `String(number)`.
pub fn number_to_string(n: f64) -> String {
    if n == 0.0 {
        return "0".to_owned();
    }
    let mut buffer = ryu_js::Buffer::new();
    buffer.format(n).to_owned()
}

/// `Cast.toString`.
pub fn to_string(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Num(n) => number_to_string(n),
        ValueRef::Str(s) => s.to_owned(),
        ValueRef::Bool(b) => bool_to_string(b).to_owned(),
    }
}

pub fn bool_to_string(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// `Cast.toBoolean` for a string: empty, `0` and `false` (in any case) are false.
pub fn str_to_boolean(s: &str) -> bool {
    !(s.is_empty() || s == "0" || s.eq_ignore_ascii_case("false"))
}

/// `Cast.toBoolean` for a number: 0, -0 and NaN are false.
pub fn num_to_boolean(n: f64) -> bool {
    n != 0.0 && !n.is_nan()
}

/// `Cast.toBoolean`.
pub fn to_boolean(v: ValueRef<'_>) -> bool {
    match v {
        ValueRef::Num(n) => num_to_boolean(n),
        ValueRef::Str(s) => str_to_boolean(s),
        ValueRef::Bool(b) => b,
    }
}

/// `Cast.compare`: negative when `a < b`, positive when `a > b`, 0 when equal. Numbers compare as
/// numbers; when either side is not numeric, both compare as lowercased strings.
pub fn compare(a: ValueRef<'_>, b: ValueRef<'_>) -> f64 {
    let mut n1 = js_value_number(a);
    let mut n2 = js_value_number(b);
    if n1 == 0.0 && value_is_whitespace(a) {
        n1 = f64::NAN;
    } else if n2 == 0.0 && value_is_whitespace(b) {
        n2 = f64::NAN;
    }
    if n1.is_nan() || n2.is_nan() {
        let s1 = to_string(a).to_lowercase();
        let s2 = to_string(b).to_lowercase();
        return match compare_utf16(&s1, &s2) {
            Ordering::Less => -1.0,
            Ordering::Equal => 0.0,
            Ordering::Greater => 1.0,
        };
    }
    compare_numbers(n1, n2)
}

/// `Cast.compare` once both sides are known to be numbers that are not NaN.
pub fn compare_numbers(n1: f64, n2: f64) -> f64 {
    if n1.is_infinite() && n2.is_infinite() && n1 == n2 {
        return 0.0;
    }
    n1 - n2
}

/// `Number(v)` as `Cast.compare` applies it: NaN stays NaN here.
fn js_value_number(v: ValueRef<'_>) -> f64 {
    match v {
        ValueRef::Num(n) => n,
        ValueRef::Str(s) => js_number(s),
        ValueRef::Bool(b) => f64::from(b),
    }
}

fn value_is_whitespace(v: ValueRef<'_>) -> bool {
    match v {
        ValueRef::Str(s) => is_whitespace(s),
        _ => false,
    }
}

/// JavaScript string comparison: by UTF-16 code unit.
fn compare_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `Cast.isInt`: whether a value looks like an integer, as `pick random` decides it.
pub fn is_int(v: ValueRef<'_>) -> bool {
    match v {
        ValueRef::Num(n) => n.is_nan() || n == n.floor(),
        ValueRef::Bool(_) => true,
        ValueRef::Str(s) => !s.contains('.'),
    }
}

/// `Math.round`: halves round up, toward positive infinity.
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    let r = if x - f < 0.5 { f } else { f + 1.0 };
    if r == 0.0 && x < 0.0 { -0.0 } else { r }
}

/// The `mod` block: JavaScript's `%` with the result moved into the sign of the divisor.
pub fn modulo(n: f64, modulus: f64) -> f64 {
    let mut result = n % modulus;
    if result / modulus < 0.0 {
        result += modulus;
    }
    result
}

/// `pick random`, given a uniform random number in `[0, 1)`.
pub fn random(from: ValueRef<'_>, to: ValueRef<'_>, r: f64) -> f64 {
    let n_from = to_number(from);
    let n_to = to_number(to);
    let low = if n_from <= n_to { n_from } else { n_to };
    let high = if n_from <= n_to { n_to } else { n_from };
    if low == high {
        return low;
    }
    if is_int(from) && is_int(to) {
        return low + (r * ((high + 1.0) - low)).floor();
    }
    (r * (high - low)) + low
}

/// `Number.prototype.toFixed(digits)`.
pub fn to_fixed(x: f64, digits: u32) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return number_to_string(x);
    }
    let n = scaled_half_up(x.abs(), digits);
    let scale = 10u128.pow(digits);
    let sign = if x < 0.0 { "-" } else { "" };
    if digits == 0 {
        return format!("{sign}{n}");
    }
    format!("{sign}{}.{:0width$}", n / scale, n % scale, width = digits as usize)
}

/// `parseFloat(x.toFixed(digits))`, which is how the trigonometric blocks tidy their results.
pub fn to_fixed_parsed(x: f64, digits: u32) -> f64 {
    if !x.is_finite() || x.abs() >= 1e21 {
        return x;
    }
    to_fixed(x, digits).parse().unwrap_or(0.0)
}

/// The text a say or think bubble shows for a number, as scratch-vm's `_formatBubbleText` makes
/// it: non-integers from 0.01 up are shown with exactly two decimals, everything else as is.
pub fn bubble_number(n: f64) -> String {
    if n.abs() >= 0.01 && n % 1.0 != 0.0 { to_fixed(n, 2) } else { number_to_string(n) }
}

/// `round(x * 10^digits)` with ties going up, computed exactly from the bits of `x`.
fn scaled_half_up(x: f64, digits: u32) -> u128 {
    debug_assert!((0.0..1e21).contains(&x));
    if x == 0.0 {
        return 0;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exp) = if exponent == 0 { (fraction, -1074) } else { (fraction | (1u64 << 52), exponent - 1075) };
    let scaled = u128::from(mantissa) * 10u128.pow(digits);
    if exp >= 0 {
        scaled << exp
    } else {
        let shift = -exp;
        if shift > 100 { 0 } else { (scaled + (1u128 << (shift - 1))) >> shift }
    }
}

/// The operations of the `mathop` block, in the order of its menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathOp {
    Abs,
    Floor,
    Ceiling,
    Sqrt,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Ln,
    Log,
    Exp,
    Pow10,
}

impl MathOp {
    /// The menu value of the operation in a project file.
    pub fn from_menu(name: &str) -> Option<Self> {
        Some(match name {
            "abs" => Self::Abs,
            "floor" => Self::Floor,
            "ceiling" => Self::Ceiling,
            "sqrt" => Self::Sqrt,
            "sin" => Self::Sin,
            "cos" => Self::Cos,
            "tan" => Self::Tan,
            "asin" => Self::Asin,
            "acos" => Self::Acos,
            "atan" => Self::Atan,
            "ln" => Self::Ln,
            "log" => Self::Log,
            "e ^" => Self::Exp,
            "10 ^" => Self::Pow10,
            _ => return None,
        })
    }

    pub fn apply(self, n: f64) -> f64 {
        match self {
            Self::Abs => n.abs(),
            Self::Floor => n.floor(),
            Self::Ceiling => n.ceil(),
            Self::Sqrt => n.sqrt(),
            Self::Sin => to_fixed_parsed((std::f64::consts::PI * n / 180.0).sin(), 10),
            Self::Cos => to_fixed_parsed((std::f64::consts::PI * n / 180.0).cos(), 10),
            Self::Tan => tan(n),
            Self::Asin => n.asin() * 180.0 / std::f64::consts::PI,
            Self::Acos => n.acos() * 180.0 / std::f64::consts::PI,
            Self::Atan => n.atan() * 180.0 / std::f64::consts::PI,
            Self::Ln => n.ln(),
            Self::Log => n.ln() / std::f64::consts::LN_10,
            Self::Exp => n.exp(),
            Self::Pow10 => 10f64.powf(n),
        }
    }
}

/// `MathUtil.tan`: exact infinities at 90 and 270 degrees.
fn tan(angle: f64) -> f64 {
    let angle = angle % 360.0;
    if angle == 90.0 || angle == -270.0 {
        f64::INFINITY
    } else if angle == -90.0 || angle == 270.0 {
        f64::NEG_INFINITY
    } else {
        to_fixed_parsed((std::f64::consts::PI * angle / 180.0).tan(), 10)
    }
}

/// `length of`: the string's length in UTF-16 code units.
pub fn length(s: &str) -> f64 {
    s.encode_utf16().count() as f64
}

/// `letter (index) of (s)`: 1-based, empty when out of range.
pub fn letter_of(index: f64, s: &str) -> String {
    let len = length(s);
    if index < 1.0 || index > len {
        return String::new();
    }
    let i = index - 1.0;
    let i = if i.is_nan() { 0 } else { i.trunc() as usize };
    match s.encode_utf16().nth(i) {
        Some(unit) => String::from_utf16_lossy(&[unit]),
        None => String::new(),
    }
}

/// `(s) contains (part)?`, case-insensitively.
pub fn contains(s: &str, part: &str) -> bool {
    s.to_lowercase().contains(&part.to_lowercase())
}

/// Whether a string denotes the same value as the number it reads as: `"5"` does, `"5.0"`, `" 5"`
/// and `"1e3"` do not.
pub fn is_number_like(s: &str) -> bool {
    let n = js_number(s);
    !n.is_nan() && number_to_string(n) == s
}

/// Whether a compiler may treat a string literal in a project as the number it reads as. Every
/// block casts such a string and the number alike, with one exception: a say or think bubble
/// formats numbers (see [`bubble_number`]) and shows strings as they are. So a number-like string is
/// safe exactly when the bubble would show the same text for the number.
pub fn is_safe_number_literal(s: &str) -> bool {
    is_number_like(s) && bubble_number(js_number(s)) == s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_parse_like_javascript() {
        assert_eq!(js_number("  12.5 "), 12.5);
        assert_eq!(js_number(""), 0.0);
        assert_eq!(js_number("0x1A"), 26.0);
        assert_eq!(js_number("0b101"), 5.0);
        assert!(js_number("-0x10").is_nan());
        assert_eq!(js_number("1e3"), 1000.0);
        assert_eq!(js_number(".5"), 0.5);
        assert_eq!(js_number("5."), 5.0);
        assert!(js_number("1_000").is_nan());
        assert!(js_number("inf").is_nan());
        assert!(js_number("nan").is_nan());
        assert_eq!(js_number("-Infinity"), f64::NEG_INFINITY);
        assert!(js_number("12abc").is_nan());
        assert_eq!(js_number("-0").to_bits(), (-0.0f64).to_bits());
        assert_eq!(str_to_number("abc"), 0.0);
    }

    #[test]
    fn numbers_format_like_javascript() {
        assert_eq!(number_to_string(1.0), "1");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
    }

    #[test]
    fn comparison_follows_cast_compare() {
        assert!(compare(ValueRef::Str("10"), ValueRef::Str("9")) > 0.0);
        assert!(compare(ValueRef::Str("abc"), ValueRef::Str("ABD")) < 0.0);
        assert_eq!(compare(ValueRef::Str("abc"), ValueRef::Str("ABC")), 0.0);
        assert!(compare(ValueRef::Str(""), ValueRef::Num(0.0)) != 0.0);
        assert_eq!(compare(ValueRef::Str(" "), ValueRef::Str("")), 1.0);
        assert_eq!(compare(ValueRef::Num(f64::INFINITY), ValueRef::Num(f64::INFINITY)), 0.0);
        assert!(compare(ValueRef::Num(f64::NAN), ValueRef::Num(1.0)) > 0.0);
        assert_eq!(compare(ValueRef::Bool(true), ValueRef::Num(1.0)), 0.0);
        assert_eq!(compare(ValueRef::Str("true"), ValueRef::Bool(true)), 0.0);
    }

    #[test]
    fn rounding_follows_math_round() {
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(0.49999999999999994), 0.0);
        assert_eq!(round(-0.3).to_bits(), (-0.0f64).to_bits());
        assert!(round(f64::NAN).is_nan());
    }

    #[test]
    fn modulo_takes_the_sign_of_the_divisor() {
        assert_eq!(modulo(-7.0, 3.0), 2.0);
        assert_eq!(modulo(7.0, -3.0), -2.0);
        assert_eq!(modulo(5.5, 2.0), 1.5);
        assert!(modulo(1.0, 0.0).is_nan());
    }

    #[test]
    fn to_fixed_rounds_like_javascript() {
        assert_eq!(to_fixed_parsed(1.0 / 3.0, 2), 0.33);
        assert_eq!(to_fixed_parsed(2.675, 2), 2.67);
        assert_eq!(to_fixed_parsed(1.005, 2), 1.0);
        assert_eq!(to_fixed_parsed(0.125, 2), 0.13);
        assert_eq!(to_fixed_parsed(-1.5, 0), -2.0);
        assert_eq!(to_fixed_parsed(1e21, 2), 1e21);
        assert_eq!(to_fixed_parsed(123.456, 2), 123.46);
        assert_eq!(to_fixed_parsed(-0.001, 2).to_bits(), (-0.0f64).to_bits());
        assert_eq!(to_fixed_parsed(1e-20, 2), 0.0);
        assert_eq!(to_fixed(0.5, 2), "0.50");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(2.0, 0), "2");
        assert_eq!(to_fixed(1e21, 2), "1e+21");
    }

    #[test]
    fn bubbles_format_numbers_like_scratch() {
        assert_eq!(bubble_number(1.0 / 3.0), "0.33");
        assert_eq!(bubble_number(0.5), "0.50");
        assert_eq!(bubble_number(3.0), "3");
        assert_eq!(bubble_number(0.001), "0.001");
        assert_eq!(bubble_number(-2.5), "-2.50");
        assert_eq!(bubble_number(f64::INFINITY), "Infinity");
        assert_eq!(bubble_number(f64::NAN), "NaN");
        assert_eq!(bubble_number(1e21), "1e+21");
    }

    #[test]
    fn math_ops() {
        assert_eq!(MathOp::Sin.apply(30.0), 0.5);
        assert_eq!(MathOp::Cos.apply(90.0), 0.0);
        assert_eq!(MathOp::Tan.apply(90.0), f64::INFINITY);
        assert_eq!(MathOp::Tan.apply(45.0), 1.0);
        // JavaScript divides by Math.LN10 instead of using log10, and Scratch inherits the error.
        assert_eq!(MathOp::Log.apply(1000.0), 2.9999999999999996);
        assert_eq!(MathOp::Log.apply(100.0), 2.0);
        assert_eq!(MathOp::Pow10.apply(2.0), 100.0);
    }

    #[test]
    fn strings() {
        assert_eq!(length("héllo"), 5.0);
        assert_eq!(length("😀"), 2.0);
        assert_eq!(letter_of(1.0, "abc"), "a");
        assert_eq!(letter_of(1.7, "abc"), "a");
        assert_eq!(letter_of(0.0, "abc"), "");
        assert_eq!(letter_of(4.0, "abc"), "");
        assert_eq!(letter_of(f64::NAN, "abc"), "a");
        assert!(contains("Hello World", "WORLD"));
        assert!(!str_to_boolean("FALSE"));
        assert!(!str_to_boolean("0"));
        assert!(str_to_boolean("0.0"));
    }

    #[test]
    fn number_like_strings() {
        assert!(is_number_like("5"));
        assert!(is_number_like("-1.5"));
        assert!(is_number_like("Infinity"));
        assert!(!is_number_like("5.0"));
        assert!(!is_number_like(" 5"));
        assert!(!is_number_like("1e3"));
        assert!(!is_number_like(""));
        assert!(!is_number_like("-0"));
        assert!(!is_number_like("NaN"));
        assert!(is_safe_number_literal("0"));
        assert!(is_safe_number_literal("3.14"));
        assert!(is_safe_number_literal("0.001"));
        assert!(!is_safe_number_literal("-2.5"));
        assert!(!is_safe_number_literal("3.14159"));
        assert!(!is_safe_number_literal("abc"));
    }

    #[test]
    fn random_ranges() {
        assert_eq!(random(ValueRef::Num(1.0), ValueRef::Num(10.0), 0.0), 1.0);
        assert_eq!(random(ValueRef::Num(1.0), ValueRef::Num(10.0), 0.999), 10.0);
        assert_eq!(random(ValueRef::Str("1.0"), ValueRef::Num(2.0), 0.5), 1.5);
        assert_eq!(random(ValueRef::Num(5.0), ValueRef::Num(5.0), 0.5), 5.0);
    }
}
