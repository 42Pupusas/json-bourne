//! JSON string escapes, both directions.
//!
//! `writer` turns a `&str` into escaped JSON (used by every sink);
//! `decoder` turns an escaped body back into text (used by the typed
//! `String` / `Cow` / `char` readers); `validator` checks an escaped
//! body without producing output (used by the lexer's stream contract).
//!
//! `Hex4` is the one `\uXXXX` digit reader — it was previously copied
//! into both `lexer.rs` and `de.rs` (audit 3.15, 6).

// `escape` is itself private, so `pub` here is crate-visibility.
//
// Decoding is `alloc`-gated: its only callers are the owned-string
// readers (`String`, `Cow`, `char`), which are themselves `alloc`-only.
// Validation and writing ship in every build.
#[cfg(feature = "alloc")]
pub mod decoder;
pub mod hex;
pub mod validator;
mod writer;

pub use hex::Hex4;
pub use writer::write_escaped;
#[cfg(feature = "alloc")]
pub use writer::write_escaped_body;
