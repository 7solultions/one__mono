//! Reading scalars as typed values.
//!
//! SYON (layer 1) only hands back text; typing is layer 2, Sheni. Settings files
//! are stricter than Sheni's `bool`, which also accepts `yes`, `no` and any case:
//! a flag that is present must say `true` or `false`, so a typo is an error and
//! not a quiet `true`. Integers go through Sheni.

use sheni_types::{IntWidth, PrimitiveType, PrimitiveValue};
use thiserror::Error;

/// A scalar that is not the type the schema declares.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScalarError {
    #[error("`{raw}` is not a boolean; write `true` or `false`")]
    NotABoolean { raw: String },

    #[error("`{raw}` is not an integer: {reason}")]
    NotAnInteger { raw: String, reason: String },
}

/// Exactly `true` or `false`, nothing else.
pub fn read_boolean(raw: &str) -> Result<bool, ScalarError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ScalarError::NotABoolean {
            raw: raw.to_string(),
        }),
    }
}

/// A signed 64-bit decimal integer, read through Sheni.
pub fn read_integer(raw: &str) -> Result<i64, ScalarError> {
    let not_an_integer = |reason: String| ScalarError::NotAnInteger {
        raw: raw.to_string(),
        reason,
    };
    // Sheni reads `-0` as 0. The settings grammar never has, because a minus
    // sign on zero is a typo and not a number, so the rule is kept here.
    if raw == "-0" {
        return Err(not_an_integer("a minus sign needs a non-zero number".to_string()));
    }
    match PrimitiveType::Signed(IntWidth::W64).read(raw) {
        Ok(PrimitiveValue::Signed { value, .. }) => {
            i64::try_from(value).map_err(|_| not_an_integer("out of range".to_string()))
        }
        Ok(_) => Err(not_an_integer("not a signed integer".to_string())),
        Err(error) => Err(not_an_integer(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_are_exactly_true_or_false() {
        assert_eq!(read_boolean("true"), Ok(true));
        assert_eq!(read_boolean("false"), Ok(false));
    }

    #[test]
    fn booleans_in_other_spellings_are_errors() {
        // Sheni's bool is looser (yes, no, any case); a settings file is not.
        for raw in ["True", "FALSE", "yes", "no", "on", "off", "1", "", " true"] {
            assert!(read_boolean(raw).is_err(), "{raw:?} must not be a boolean");
        }
    }

    #[test]
    fn integers_follow_the_old_grammar() {
        assert_eq!(read_integer("0"), Ok(0));
        assert_eq!(read_integer("42"), Ok(42));
        assert_eq!(read_integer("-7"), Ok(-7));
        for raw in [
            "+7",
            "007",
            "00",
            "-0",
            "",
            "-",
            "1_000",
            " 1",
            "1.5",
            "9223372036854775808",
        ] {
            assert!(read_integer(raw).is_err(), "{raw:?} must not be an integer");
        }
    }
}
