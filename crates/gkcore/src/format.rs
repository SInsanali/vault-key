//! Number formatting for text shown to the user.

/// A whole number with thousands separators: `1,234,567`.
pub fn number(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// An amount of money: `$123,456`.
pub fn money(value: i64) -> String {
    if value < 0 {
        format!("-${}", number(value.saturating_neg()))
    } else {
        format!("${}", number(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_money() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(1_000), "1,000");
        assert_eq!(number(1_234_567), "1,234,567");
        assert_eq!(number(-1_234), "-1,234");
        assert_eq!(number(i64::MIN), "-9,223,372,036,854,775,808");
        assert_eq!(money(0), "$0");
        assert_eq!(money(123_456), "$123,456");
        assert_eq!(money(-5), "-$5");
    }
}
