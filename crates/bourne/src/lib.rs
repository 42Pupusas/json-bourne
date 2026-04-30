#![cfg_attr(not(feature = "std"), no_std)]

//! `bourne` — type-driven JSON parsing.
//!
//! The top-level entry point is [`parse`], which deserializes directly into
//! the caller's chosen type. There is no generic `Value` middle layer.
//!
//! ```
//! use bourne::parse_str;
//!
//! let n: u32 = parse_str("42").unwrap();
//! assert_eq!(n, 42);
//! ```

#[cfg(feature = "alloc")]
extern crate alloc;

mod de;

pub use bourne_core::{Error, ErrorKind, Event, JsonNum, JsonStr, Parser, Position};
pub use de::{EventSource, FromJson, PeekableParser, parse, parse_str};

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written `FromJson` for a representative struct.
    ///
    /// This is the shape a derive macro would generate. Validating the trait
    /// shape with a real struct before writing the macro means we know the
    /// generated code can be both correct and ergonomic.
    #[derive(Debug, PartialEq)]
    struct User<'input> {
        id: u64,
        name: &'input str,
        active: bool,
        nickname: Option<&'input str>,
    }

    impl<'input> FromJson<'input> for User<'input> {
        fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
            let first = source
                .next_event()?
                .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, source.position()))?;
            if !matches!(first, Event::StartObject) {
                return Err(Error::new(ErrorKind::ExpectedObject, source.position()));
            }

            let mut id: Option<u64> = None;
            let mut name: Option<&'input str> = None;
            let mut active: Option<bool> = None;
            let mut nickname: Option<&'input str> = None;

            loop {
                let ev = source
                    .next_event()?
                    .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, source.position()))?;
                let key = match ev {
                    Event::EndObject => break,
                    Event::Key(k) => k,
                    _ => return Err(Error::new(ErrorKind::TypeMismatch, source.position())),
                };
                let key_str = key.as_str().ok_or_else(|| {
                    // Until we have a string-decoding API, escaped field names
                    // can't be matched by value. v1 limitation, documented.
                    Error::new(ErrorKind::InvalidEscape, source.position())
                })?;

                match key_str {
                    "id" => {
                        if id.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, source.position()));
                        }
                        id = Some(u64::from_json(source)?);
                    }
                    "name" => {
                        if name.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, source.position()));
                        }
                        name = Some(<&str>::from_json(source)?);
                    }
                    "active" => {
                        if active.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, source.position()));
                        }
                        active = Some(bool::from_json(source)?);
                    }
                    "nickname" => {
                        if nickname.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, source.position()));
                        }
                        nickname = Option::<&str>::from_json(source)?;
                    }
                    _ => {
                        return Err(Error::new(ErrorKind::UnknownField, source.position()));
                    }
                }
            }

            Ok(Self {
                id: id.ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
                name: name.ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
                active: active
                    .ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
                nickname,
            })
        }
    }

    #[test]
    fn primitives_round_through_typed_parse() {
        assert!(parse_str::<bool>("true").unwrap());
        assert!(!parse_str::<bool>("false").unwrap());
        assert_eq!(parse_str::<i32>("-7").unwrap(), -7);
        assert_eq!(parse_str::<u64>("9999999999").unwrap(), 9_999_999_999);
        let f: f64 = parse_str("1.5e2").unwrap();
        assert!((f - 150.0).abs() < f64::EPSILON);
        assert_eq!(parse_str::<&str>("\"hello\"").unwrap(), "hello");
    }

    #[test]
    fn integer_overflow_is_rejected_at_parse_site() {
        let r = parse_str::<u8>("256");
        assert_eq!(r.unwrap_err().kind, ErrorKind::NumberOutOfRange);
    }

    #[test]
    fn type_mismatch_is_rejected_at_parse_site() {
        let r = parse_str::<u32>("\"five\"");
        assert_eq!(r.unwrap_err().kind, ErrorKind::ExpectedNumber);
    }

    #[test]
    fn option_handles_null_and_value() {
        let none: Option<u32> = parse_str("null").unwrap();
        assert_eq!(none, None);
        let some: Option<u32> = parse_str("17").unwrap();
        assert_eq!(some, Some(17));
    }

    #[test]
    fn fixed_array_exact_length() {
        let arr: [i32; 3] = parse_str("[1,2,3]").unwrap();
        assert_eq!(arr, [1, 2, 3]);
    }

    #[test]
    fn fixed_array_too_short_errors() {
        let r = parse_str::<[i32; 3]>("[1,2]");
        assert!(r.is_err());
    }

    #[test]
    fn fixed_array_too_long_errors() {
        let r = parse_str::<[i32; 3]>("[1,2,3,4]");
        assert!(r.is_err());
    }

    #[test]
    fn tuple_heterogeneous() {
        let v: (i32, &str, bool) = parse_str(r#"[1, "hi", true]"#).unwrap();
        assert_eq!(v, (1, "hi", true));
    }

    #[test]
    fn borrowed_str_lifetime() {
        let input = String::from(r#""borrowed""#);
        let s: &str = parse_str(&input).unwrap();
        assert_eq!(s, "borrowed");
    }

    #[test]
    fn rejects_trailing_data_at_typed_layer() {
        let r = parse_str::<u32>("1 2");
        assert!(r.is_err());
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn vec_and_string() {
        let v: Vec<i32> = parse_str("[10, 20, 30]").unwrap();
        assert_eq!(v, vec![10, 20, 30]);
        let s: String = parse_str(r#""owned""#).unwrap();
        assert_eq!(s, "owned");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn nested_vec() {
        let v: Vec<Vec<i32>> = parse_str("[[1,2],[3]]").unwrap();
        assert_eq!(v, vec![vec![1, 2], vec![3]]);
    }

    #[test]
    fn struct_parses_with_all_fields() {
        let json = r#"{"id":42,"name":"alice","active":true,"nickname":"al"}"#;
        let u: User<'_> = parse_str(json).unwrap();
        assert_eq!(
            u,
            User {
                id: 42,
                name: "alice",
                active: true,
                nickname: Some("al"),
            }
        );
    }

    #[test]
    fn struct_parses_with_optional_field_null() {
        let json = r#"{"id":1,"name":"bob","active":false,"nickname":null}"#;
        let u: User<'_> = parse_str(json).unwrap();
        assert_eq!(u.nickname, None);
    }

    #[test]
    fn struct_parses_with_optional_field_omitted() {
        // Field order is irrelevant; missing optional collapses to None.
        let json = r#"{"name":"carol","id":7,"active":true}"#;
        let u: User<'_> = parse_str(json).unwrap();
        assert_eq!(u.nickname, None);
    }

    #[test]
    fn struct_rejects_missing_required_field() {
        let json = r#"{"id":1,"active":true}"#;
        let err = parse_str::<User<'_>>(json).unwrap_err();
        assert_eq!(err.kind, ErrorKind::MissingField);
    }

    #[test]
    fn struct_rejects_wrong_field_type() {
        let json = r#"{"id":"oops","name":"d","active":true}"#;
        let err = parse_str::<User<'_>>(json).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ExpectedNumber);
    }

    #[test]
    fn struct_rejects_unknown_field() {
        let json = r#"{"id":1,"name":"e","active":true,"extra":1}"#;
        let err = parse_str::<User<'_>>(json).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnknownField);
    }

    #[test]
    fn struct_rejects_duplicate_key() {
        let json = r#"{"id":1,"id":2,"name":"f","active":true}"#;
        let err = parse_str::<User<'_>>(json).unwrap_err();
        assert_eq!(err.kind, ErrorKind::DuplicateKey);
    }

    #[test]
    fn struct_borrows_strings_from_input() {
        let input = String::from(r#"{"id":1,"name":"borrowed","active":true}"#);
        let u: User<'_> = parse_str(&input).unwrap();
        // The pointer must lie inside the input buffer — proving zero copy.
        let input_start = input.as_ptr() as usize;
        let input_end = input_start + input.len();
        let name_ptr = u.name.as_ptr() as usize;
        assert!(
            (input_start..input_end).contains(&name_ptr),
            "name was copied, not borrowed",
        );
    }
}
