//! Runtime `Option` detection for derive-generated code.
//!
//! `#[bourne(skip_if_none)]` and the "an absent key means `None`" reader
//! rule both need to know whether a field is an `Option`. A proc macro
//! sees only tokens, so a syntactic test reads `type MaybeName =
//! Option<String>` as a required non-`Option` field: the writer emits
//! `"name":null` instead of omitting the member, and — worse — the
//! reader rejects valid JSON that simply omits the key (audit A3).
//!
//! The macro does not need to resolve the alias, because the *type
//! checker* already has. Rust prefers an inherent method over a trait
//! method, so an inherent impl on the `Option`-shaped wrapper wins
//! whenever the field really is an `Option` however it is spelled, and
//! the blanket trait impl catches everything else. That is a form of
//! specialization that works on stable, resolved at the concrete call
//! site the derive expands to.
//!
//! The one place this does not discriminate is a fully generic field
//! (`field: T` where `T` is a type parameter): inherent impls are not
//! selectable there, so such a field takes the fallback and behaves as
//! non-`Option`. `Option<T>` with `T` generic *is* detected, since the
//! outer constructor is concrete.

/// Borrowed field value, for the writer's skip decision.
#[derive(Debug)]
pub struct OptionShape<'a, T: ?Sized>(pub &'a T);

impl<T> OptionShape<'_, Option<T>> {
    /// The field is an `Option` and holds `None`, so `skip_if_none`
    /// omits the member. Inherent, so it outranks
    /// [`OptionShapeFallback::bourne_is_none`].
    #[inline]
    #[must_use]
    pub const fn bourne_is_none(&self) -> bool {
        self.0.is_none()
    }
}

/// Fallback for field types that are not `Option`.
pub trait OptionShapeFallback {
    /// Never skip: a non-`Option` field has no "none" state.
    fn bourne_is_none(&self) -> bool;
}

impl<T: ?Sized> OptionShapeFallback for OptionShape<'_, T> {
    #[inline]
    fn bourne_is_none(&self) -> bool {
        false
    }
}

/// A reader's per-field slot: `Some` once the key has been seen.
#[derive(Debug)]
pub struct OptionSlot<T>(pub Option<T>);

impl<T> OptionSlot<Option<T>> {
    /// An absent key yields `None` rather than a missing-field error.
    /// Inherent, so it outranks [`OptionSlotFallback::bourne_or_absent`].
    #[inline]
    #[must_use]
    pub fn bourne_or_absent(self) -> Option<Option<T>> {
        Some(self.0.unwrap_or(None))
    }
}

/// Fallback for field types that are not `Option`.
pub trait OptionSlotFallback {
    /// The field's own type, handed back unchanged.
    type Out;
    /// An absent key stays absent, so the caller raises `MissingField`.
    fn bourne_or_absent(self) -> Option<Self::Out>;
}

impl<T> OptionSlotFallback for OptionSlot<T> {
    type Out = T;
    #[inline]
    fn bourne_or_absent(self) -> Option<T> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type MaybeName = Option<i32>;
    type Maybe<T> = Option<T>;

    #[test]
    fn writer_detects_direct_option() {
        let none: Option<i32> = None;
        let some: Option<i32> = Some(1);
        assert!(OptionShape(&none).bourne_is_none());
        assert!(!OptionShape(&some).bourne_is_none());
    }

    #[test]
    fn writer_sees_through_aliases() {
        let none: MaybeName = None;
        let some: MaybeName = Some(1);
        assert!(OptionShape(&none).bourne_is_none());
        assert!(!OptionShape(&some).bourne_is_none());

        let generic: Maybe<u8> = None;
        assert!(OptionShape(&generic).bourne_is_none());
    }

    #[test]
    fn writer_never_skips_non_options() {
        assert!(!OptionShape(&0u32).bourne_is_none());
        assert!(!OptionShape(&"").bourne_is_none());
        let empty: [u8; 0] = [];
        assert!(!OptionShape(&empty[..]).bourne_is_none());
    }

    #[test]
    fn writer_treats_nested_some_none_as_present() {
        let nested: Option<Option<u8>> = Some(None);
        assert!(!OptionShape(&nested).bourne_is_none());
    }

    #[test]
    fn reader_absent_option_defaults_to_none() {
        let absent: Option<Option<i32>> = None;
        assert_eq!(OptionSlot(absent).bourne_or_absent(), Some(None));
    }

    #[test]
    fn reader_absent_alias_defaults_to_none() {
        let absent: Option<MaybeName> = None;
        assert_eq!(OptionSlot(absent).bourne_or_absent(), Some(None));

        let absent: Option<Maybe<u8>> = None;
        assert_eq!(OptionSlot(absent).bourne_or_absent(), Some(None));
    }

    #[test]
    fn reader_absent_required_stays_absent() {
        let absent: Option<u32> = None;
        assert_eq!(OptionSlot(absent).bourne_or_absent(), None);
    }

    #[test]
    fn reader_preserves_present_values() {
        assert_eq!(OptionSlot(Some(7u32)).bourne_or_absent(), Some(7));

        let present: Option<MaybeName> = Some(Some(3));
        assert_eq!(OptionSlot(present).bourne_or_absent(), Some(Some(3)));

        let explicit_null: Option<MaybeName> = Some(None);
        assert_eq!(OptionSlot(explicit_null).bourne_or_absent(), Some(None));
    }
}
