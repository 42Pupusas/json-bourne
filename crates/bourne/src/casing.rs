//! Compile-time identifier case conversion for `#[bourne(rename_all = "…")]`.
//!
//! This crate forbids proc-macros and external deps, so `rename_all`
//! cannot rewrite identifiers during token expansion — `macro_rules!`
//! has no string manipulation. Instead the *runtime* key comparison
//! (parse side) and the *emitted* key (serialize side) are computed
//! from the field/variant name at compile time via a `const { … }`
//! block.
//!
//! Because the macros invoke this engine in **`const` position**, every
//! entry point is an inherent `const fn` on [`Casing`] / [`Renamed`].
//! A *trait* would be the natural home for this behavior, but const
//! trait methods are still unstable on stable Rust — dispatching
//! through a trait would push conversion to runtime and defeat the
//! zero-cost, compile-time-key design. Inherent const methods give the
//! same cohesion (behavior attached to the type, no free-floating
//! functions) while staying const-evaluable.
//!
//! Rust identifiers reaching here are always `snake_case`-ish for
//! fields and `PascalCase`-ish for variants — but we don't assume
//! either. [`Casing::convert`] first splits the input into words on a
//! set of boundaries that covers both conventions:
//!
//!   * an explicit `_` or `-` separator, and
//!   * a lower→upper transition (`fooBar` / `FooBar` → `foo`, `Bar`).
//!
//! then re-joins the words in the requested style. This matches serde's
//! `RenameRule` word-splitting closely enough for the identifiers a
//! Rust program can actually produce.
//!
//! The output is returned by value in a fixed-capacity buffer
//! ([`Renamed`]); [`Renamed::as_str`] borrows it as `&str`. A field or
//! variant name longer than [`CAP`] bytes after conversion is a
//! compile-time panic (const eval), which is fine — these are source
//! identifiers, not input data.

/// Maximum length, in bytes, of a converted key. Identifiers this long
/// don't occur in practice; overflowing is a const-eval panic.
pub const CAP: usize = 128;

/// The eight `rename_all` styles serde supports, mirrored here.
///
/// The conversion engine hangs off this type: [`Casing::from_name`]
/// parses the attribute string, [`Casing::convert`] rewrites an
/// identifier, and [`Casing::rename`] combines the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Casing {
    /// `lowercase` — words concatenated, all lower, no separators.
    Lower,
    /// `UPPERCASE` — words concatenated, all upper, no separators.
    Upper,
    /// `PascalCase` — each word capitalized, no separators.
    Pascal,
    /// `camelCase` — like Pascal but the first word stays lower.
    Camel,
    /// `snake_case` — lower words joined by `_`.
    Snake,
    /// `SCREAMING_SNAKE_CASE` — upper words joined by `_`.
    ScreamingSnake,
    /// `kebab-case` — lower words joined by `-`.
    Kebab,
    /// `SCREAMING-KEBAB-CASE` — upper words joined by `-`.
    ScreamingKebab,
}

impl Casing {
    /// Map a `rename_all` string to its [`Casing`], at compile time.
    ///
    /// # Panics
    /// An unrecognized value is a const-eval panic naming the accepted
    /// set, so a typo in `#[bourne(rename_all = "…")]` fails the build
    /// with a clear message rather than silently doing nothing.
    #[must_use]
    pub const fn from_name(name: &str) -> Self {
        // `const fn` can't match string literals, so compare byte slices.
        let b = name.as_bytes();
        if byte_eq(b, b"lowercase") {
            Self::Lower
        } else if byte_eq(b, b"UPPERCASE") {
            Self::Upper
        } else if byte_eq(b, b"PascalCase") {
            Self::Pascal
        } else if byte_eq(b, b"camelCase") {
            Self::Camel
        } else if byte_eq(b, b"snake_case") {
            Self::Snake
        } else if byte_eq(b, b"SCREAMING_SNAKE_CASE") {
            Self::ScreamingSnake
        } else if byte_eq(b, b"kebab-case") {
            Self::Kebab
        } else if byte_eq(b, b"SCREAMING-KEBAB-CASE") {
            Self::ScreamingKebab
        } else {
            panic!(
                "bourne: unknown rename_all value. Expected one of: lowercase, UPPERCASE, \
                 PascalCase, camelCase, snake_case, SCREAMING_SNAKE_CASE, kebab-case, \
                 SCREAMING-KEBAB-CASE"
            )
        }
    }

    /// Convert `src` into this style, at compile time.
    ///
    /// Word boundaries are separators (`_`/`-`) and lower→upper
    /// transitions.
    ///
    /// # Panics
    /// Const-eval panic if the result exceeds [`CAP`] bytes.
    #[must_use]
    pub const fn convert(self, src: &str) -> Renamed {
        let s = src.as_bytes();
        let mut buf = [0u8; CAP];
        let mut len = 0usize;

        // Word index: 0 for the first emitted word, incremented at each
        // boundary. Needed so camelCase can keep word 0 lowercase.
        let mut word = 0usize;
        // Whether the current output word already has at least one char
        // (controls capitalization of the *first* letter of a word).
        let mut word_started = false;

        let mut i = 0usize;
        while i < s.len() {
            let b = s[i];

            // Separator: finish the current word (if any). Emit a joiner
            // lazily at the *next* real char so we never trail a sep.
            if b == b'_' || b == b'-' {
                if word_started {
                    word += 1;
                    word_started = false;
                }
                i += 1;
                continue;
            }

            // Implicit boundary: prev char lower/digit, this char upper.
            if i > 0 && is_upper(b) {
                let prev = s[i - 1];
                if (is_lower(prev) || prev.is_ascii_digit()) && word_started {
                    word += 1;
                    word_started = false;
                }
            }

            // At the start of a new word (word_started == false) we may
            // need to emit a separator joiner before the first char.
            if !word_started {
                match self {
                    Self::Snake | Self::ScreamingSnake if word > 0 => {
                        len = Renamed::push(&mut buf, len, b'_');
                    }
                    Self::Kebab | Self::ScreamingKebab if word > 0 => {
                        len = Renamed::push(&mut buf, len, b'-');
                    }
                    _ => {}
                }
            }

            // Decide the case of this output byte.
            let first_in_word = !word_started;
            let out = match self {
                Self::Lower | Self::Snake | Self::Kebab => b.to_ascii_lowercase(),
                Self::Upper | Self::ScreamingSnake | Self::ScreamingKebab => {
                    b.to_ascii_uppercase()
                }
                Self::Pascal => {
                    if first_in_word {
                        b.to_ascii_uppercase()
                    } else {
                        b.to_ascii_lowercase()
                    }
                }
                Self::Camel => {
                    if first_in_word && word > 0 {
                        b.to_ascii_uppercase()
                    } else {
                        b.to_ascii_lowercase()
                    }
                }
            };
            len = Renamed::push(&mut buf, len, out);
            word_started = true;
            i += 1;
        }

        Renamed { buf, len }
    }

    /// Convenience: parse the `rename_all` string `name` and convert
    /// `src` in one step, at compile time. This is what the macros call
    /// at the leaf with the raw attribute literal threaded down.
    #[must_use]
    pub const fn rename(src: &str, name: &str) -> Renamed {
        Self::from_name(name).convert(src)
    }
}

/// A converted key: a fixed-capacity byte buffer plus its used length.
///
/// Held by value so it can be produced inside a `const { … }` block and
/// borrowed as `&str` for the surrounding comparison / write.
#[derive(Debug, Clone, Copy)]
pub struct Renamed {
    buf: [u8; CAP],
    len: usize,
}

impl Renamed {
    /// Borrow the converted key as a string slice.
    ///
    /// # Panics
    /// Never in practice: the buffer is filled only with ASCII bytes
    /// derived from a valid `&str`, so it is always valid UTF-8.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        // Use the checked path, which const-eval folds away. The input
        // was ASCII by construction.
        match core::str::from_utf8(self.buf.split_at(self.len).0) {
            Ok(s) => s,
            // Unreachable: we only ever push ASCII. A const-eval panic
            // here would mean a converter bug, caught at compile time.
            Err(_) => panic!("bourne: rename_all produced non-UTF-8 (converter bug)"),
        }
    }

    /// Push a byte, panicking (at const eval) on capacity overflow.
    const fn push(buf: &mut [u8; CAP], len: usize, b: u8) -> usize {
        assert!(len < CAP, "bourne: rename_all key exceeds capacity");
        buf[len] = b;
        len + 1
    }
}

/// True for an ASCII uppercase letter.
const fn is_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

/// True for an ASCII lowercase letter.
const fn is_lower(b: u8) -> bool {
    b.is_ascii_lowercase()
}

/// Byte-slice equality usable in `const fn`.
const fn byte_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}
