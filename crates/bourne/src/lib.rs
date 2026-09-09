#![cfg_attr(not(feature = "std"), no_std)]

//! Type-driven JSON: parse straight into the caller's chosen type and
//! serialize from it, with no generic `Value` middle layer.
//!
//! `json-bourne` skips the dynamic-tree intermediate that crates like
//! `serde_json` use. Each type knows how to deserialize itself from a
//! [`Lexer`] (via [`FromJson`](trait@FromJson)) or write itself to a [`JsonWrite`]
//! sink (via [`ToJson`]). The typed structure already enforces JSON's
//! grammar, so the per-event state machine is pure overhead for typed
//! consumers — skipping it makes the typed path ~2× faster on
//! integer / string-heavy payloads.
//!
//! - **Derive-driven.** `#[derive(FromJson, ToJson)]` (the `derive`
//!   feature, on by default) generates the typed impls. The generated
//!   code is itself `no_std`; only the compile-time derive pulls in the
//!   proc-macro stack.
//! - **`no_std` everywhere.** The streaming [`Lexer`] / [`Parser`]
//!   layer is `no_std` always; with the default `std` feature off
//!   the crate is `no_std + alloc`, and with `alloc` off too it is
//!   pure `no_std`.
//! - **Borrowed strings by default.** `&'input str` and `Cow<'input,
//!   str>` parse zero-copy when the input contains no escapes.
//! - **Bounded by construction.** Container nesting is depth-limited
//!   (default 128, const-generic). The streaming parser is a state
//!   machine with no recursion.
//!
//! # Quick start
//!
//! Parse a primitive directly into a Rust type:
//!
//! ```
//! use json_bourne::parse_str;
//! let n: u32 = parse_str("42").unwrap();
//! assert_eq!(n, 42);
//! ```
//!
//! Parse a struct with `#[derive(FromJson)]`:
//!
//! ```
//! use json_bourne::{FromJson, parse_str};
//!
//! #[derive(Debug, PartialEq, FromJson)]
//! struct User<'input> {
//!     id: u64,
//!     name: &'input str,
//!     active: bool,
//! }
//!
//! let u: User<'_> = parse_str(r#"{"id":1,"name":"alice","active":true}"#).unwrap();
//! assert_eq!(u.name, "alice");
//! ```
//!
//! Serialize back out with `#[derive(ToJson)]`:
//!
//! ```
//! use json_bourne::{ToJson, to_string};
//!
//! #[derive(ToJson)]
//! struct Point { x: i32, y: i32 }
//!
//! let s = to_string(&Point { x: 3, y: -7 }).unwrap();
//! assert_eq!(s, r#"{"x":3,"y":-7}"#);
//! ```
//!
//! # Output destinations
//!
//! | Sink                       | Entry point                 |
//! |----------------------------|-----------------------------|
//! | `String`                   | [`to_string`]               |
//! | `Vec<u8>`                  | [`to_vec`]                  |
//! | `String` (pretty-printed)  | [`to_string_pretty`]        |
//! | any [`std::io::Write`]     | [`to_writer`] (std only)    |
//! | any [`core::fmt::Write`]   | [`to_fmt`]                  |
//!
//! Custom sinks implement [`JsonWrite`] directly.
//!
//! # Features
//!
//! | Feature     | Default | Purpose                                     |
//! |-------------|---------|---------------------------------------------|
//! | `std`       | yes     | `HashMap`, `std::net`, `std::path`, `to_writer` |
//! | `alloc`     | yes     | `String`, `Vec`, `Box`/`Rc`/`Arc`, escape decoding, `to_string`/`to_vec` |
//! | `derive`    | yes     | `#[derive(FromJson, ToJson)]` via `bourne-derive` |
//!
//! `default-features = false` plus `["alloc"]` gives a `no_std + alloc`
//! build. `default-features = false` alone gives pure `no_std`: the
//! streaming [`Lexer`] / [`Parser`], typed APIs that don't need a heap,
//! and float serialization through [`to_fmt`] / [`FmtWriteSink`] are
//! available.
// Targeted uses of `unsafe` inside the streaming layer (lexer.rs / event.rs):
//   1. `from_utf8_unchecked` after the lexer has validated every byte against
//      the RFC 3629 byte ranges inline. The safe alternative re-walks the
//      entire string per call and was 48% of `vec_borrowed_str` runtime.
//   2. `core::arch::x86_64` SSE2 intrinsics in `scan_ascii_string_run_simd`.
//      SSE2 is part of the x86_64 ABI baseline, so the `#[target_feature]`
//      precondition is statically guaranteed on x86_64 — the unsafe is
//      mechanical (intrinsics are unsafe by signature), not a memory-safety
//      escape hatch. Non-x86_64 targets compile to the scalar path.
//
// Workspace lint is `deny` (not `forbid`) for exactly this kind of
// localized, justified exception — granted per-site, never crate-wide.

#[cfg(feature = "alloc")]
extern crate alloc;

// The derive macros emit `::json_bourne::…` paths that must resolve even
// when the derive is used *inside* this crate (tests, doctests). Alias
// the crate to its own name so those absolute paths bind here too. This
// is the same self-aliasing trick `serde_derive` relies on.
#[cfg(feature = "derive")]
extern crate self as json_bourne;

mod casing;
mod de;
#[cfg(feature = "std")]
pub(crate) mod display_scratch;
mod error;
mod escape;
mod event;
mod float;
mod lexer;
mod parser;
mod ser;
#[cfg(all(test, feature = "std"))]
mod teju_gen;

#[doc(hidden)]
pub use casing::{Casing as __Casing, Renamed as __Renamed};
pub use de::{FromJson, parse, parse_str};
#[cfg(feature = "alloc")]
pub use de::{KeyCow, MapKey, key_to_cow};
pub use error::{Error, ErrorKind, LineColumn, Position};
pub use event::{Event, JsonNum, JsonStr, MAX_INPUT_LEN};
pub use lexer::{Checkpoint, DEFAULT_MAX_DEPTH, Lexer, ValueKind};
pub use parser::Parser;
#[cfg(feature = "alloc")]
pub use ser::{
    ByteSink, MapKeyOut, PrettyStringSink, StringSink, to_string, to_string_pretty, to_vec,
};
pub use ser::{FmtWriteSink, to_fmt};
#[cfg(feature = "std")]
pub use ser::{IoWriteSink, to_writer};
pub use ser::{JsonWrite, ToJson};

/// Derive `FromJson` / `ToJson` for your own structs and enums.
///
/// Requires the `derive` feature (on by default). The derives are
/// implemented by the companion `bourne-derive` proc-macro crate and
/// re-exported here so you only ever depend on and import `json_bourne`.
///
/// ```
/// use json_bourne::{FromJson, ToJson, parse_str, to_string};
///
/// #[derive(FromJson, ToJson)]
/// struct Point { x: i32, y: i32 }
///
/// let p: Point = parse_str(r#"{"x":3,"y":-7}"#).unwrap();
/// assert_eq!(to_string(&p).unwrap(), r#"{"x":3,"y":-7}"#);
/// ```
#[cfg(feature = "derive")]
pub use bourne_derive::{FromJson, ToJson};

#[cfg(all(test, feature = "std"))]
mod tests;
