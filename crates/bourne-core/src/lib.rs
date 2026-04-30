#![cfg_attr(not(feature = "std"), no_std)]

mod error;
mod event;
mod parser;

pub use error::{Error, ErrorKind, Position};
pub use event::{Event, JsonNum, JsonStr};
pub use parser::{DEFAULT_MAX_DEPTH, Parser};

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(input: &str) -> Result<alloc::vec::Vec<Event<'_>>, Error> {
        let mut p: Parser<'_> = Parser::new(input.as_bytes());
        let mut out = alloc::vec::Vec::new();
        while let Some(ev) = p.next_event()? {
            out.push(ev);
        }
        Ok(out)
    }

    #[test]
    fn null_true_false() {
        assert_eq!(collect("null").unwrap(), [Event::Null]);
        assert_eq!(collect("true").unwrap(), [Event::Bool(true)]);
        assert_eq!(collect("false").unwrap(), [Event::Bool(false)]);
    }

    #[test]
    fn numbers() {
        let evs = collect("123").unwrap();
        match &evs[..] {
            [Event::Number(n)] => assert_eq!(n.as_i64().unwrap(), 123),
            _ => panic!("{evs:?}"),
        }
        let evs = collect("-1.5e2").unwrap();
        match &evs[..] {
            [Event::Number(n)] => {
                assert!(n.is_float());
                assert!((n.as_f64().unwrap() - -150.0).abs() < f64::EPSILON);
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn empty_string_and_borrowed() {
        let evs = collect(r#""""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => {
                assert_eq!(s.as_str(), Some(""));
                assert!(!s.has_escapes());
            }
            _ => panic!("{evs:?}"),
        }
        let evs = collect(r#""hello""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => assert_eq!(s.as_str(), Some("hello")),
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn escaped_string_marked() {
        let evs = collect(r#""a\nb""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => {
                assert!(s.has_escapes());
                assert_eq!(s.as_str(), None);
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn empty_array_and_object() {
        assert_eq!(collect("[]").unwrap(), [Event::StartArray, Event::EndArray]);
        assert_eq!(collect("{}").unwrap(), [Event::StartObject, Event::EndObject]);
    }

    #[test]
    fn array_of_scalars() {
        let evs = collect("[1, true, null]").unwrap();
        assert!(matches!(evs[0], Event::StartArray));
        assert!(matches!(evs[1], Event::Number(_)));
        assert_eq!(evs[2], Event::Bool(true));
        assert_eq!(evs[3], Event::Null);
        assert!(matches!(evs[4], Event::EndArray));
    }

    #[test]
    fn nested_object() {
        let evs = collect(r#"{"a":{"b":[1,2]}}"#).unwrap();
        let kinds: alloc::vec::Vec<_> = evs
            .iter()
            .map(|e| match e {
                Event::StartObject => "{",
                Event::EndObject => "}",
                Event::StartArray => "[",
                Event::EndArray => "]",
                Event::Key(_) => "k",
                Event::Number(_) => "n",
                _ => "?",
            })
            .collect();
        assert_eq!(kinds, ["{", "k", "{", "k", "[", "n", "n", "]", "}", "}"]);
    }

    #[test]
    fn rejects_trailing_comma_in_array() {
        assert!(collect("[1,]").is_err());
    }

    #[test]
    fn rejects_trailing_comma_in_object() {
        assert!(collect(r#"{"a":1,}"#).is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(collect("null x").is_err());
    }

    #[test]
    fn rejects_leading_zero() {
        assert!(collect("01").is_err());
    }

    #[test]
    fn rejects_bare_minus() {
        assert!(collect("-").is_err());
    }

    #[test]
    fn rejects_unterminated_string() {
        assert!(collect(r#""abc"#).is_err());
    }

    #[test]
    fn rejects_control_in_string() {
        assert!(collect("\"a\nb\"").is_err());
    }

    #[test]
    fn position_tracks_lines() {
        let mut p: Parser<'_> = Parser::new(b"\n\n  null");
        let _ = p.next_event().unwrap();
        let pos = p.position();
        assert_eq!(pos.line, 3);
    }

    extern crate alloc;
}
