//! Writes a set of named fields as JSON object members.
//!
//! One implementation serves both named structs and enum struct
//! variants; they differ only in how a field's value is reached, which
//! `FieldPlan` already abstracts. Emitting the members is otherwise the
//! same problem, and keeping it one problem is what makes attributes
//! behave identically in both (audit §3.6).
//!
//! The caller opens and closes the object; this writes only the members,
//! so the same code serves a bare struct body, an externally-tagged
//! variant's inner object, and an internally-tagged variant's body where
//! the tag is already on the wire.

use proc_macro2::TokenStream;
use quote::quote;

use crate::field_plan::FieldPlan;

/// Whether a leading comma is needed before the next member, as known at
/// expansion time.
#[derive(Clone, Copy, PartialEq)]
enum Comma {
    /// Nothing committed yet; the next member elides its comma.
    Never,
    /// Something definitely committed; the next member's comma is static.
    Always,
    /// A preceding conditional field makes it runtime-dependent.
    Runtime,
}

pub(crate) struct ObjectWriter;

impl ObjectWriter {
    /// Emit the members of an object. `already_emitted` is true when the
    /// caller has already written a member (the internally-tagged
    /// enum's tag), making the first field's comma mandatory.
    pub(crate) fn write(fields: &[FieldPlan<'_>], already_emitted: bool) -> TokenStream {
        let mut stmts = Vec::new();
        let first_init = !already_emitted;
        stmts.push(quote! {
            #[allow(unused_assignments, unused_mut, unused_variables)]
            let mut __first: bool = #first_init;
        });
        // Derived writers fuse `,"key":` into one literal for speed. That
        // fusion is only valid when the sink renders raw bytes verbatim;
        // the pretty sink needs the comma/key/colon as separate structural
        // events. The associated const is a compile-time constant per sink
        // instantiation, so the branch folds away and neither path costs
        // anything at runtime.
        stmts.push(quote! {
            #[allow(non_snake_case, unused_variables)]
            let __FUSED: bool = <__W as ::json_bourne::JsonWrite>::FUSES_STRUCTURAL_BYTES;
        });

        let mut comma = if already_emitted {
            Comma::Always
        } else {
            Comma::Never
        };

        for f in fields {
            if f.attrs.skip {
                continue;
            }
            let value = f.value_ref();

            if f.is_conditional() {
                let key = &f.key;
                let scrutinee = f.option_scrutinee();
                let lead = Self::runtime_comma(comma);
                stmts.push(quote! {
                    if let ::core::option::Option::Some(ref __v) = #scrutinee {
                        #lead
                        if __FUSED {
                            __w.write_escaped_str(#key)?;
                            __w.write_raw_bytes(b":")?;
                        } else {
                            __w.object_key(#key)?;
                        }
                        ::json_bourne::ToJson::write_json(__v, __w)?;
                        __first = false;
                    }
                });
                comma = Comma::Runtime;
                continue;
            }

            if f.plain && comma != Comma::Runtime {
                // Fold (comma?)+key+colon into one compile-time literal,
                // no runtime branch.
                let name_str = f.ident.to_string();
                let lit = match comma {
                    Comma::Never => format!("\"{name_str}\":"),
                    _ => format!(",\"{name_str}\":"),
                };
                // Record that output has been committed. A later
                // conditional field gates its leading comma on
                // `!__first`; without this, plain fields would leave
                // `__first` set and that comma would be wrongly
                // suppressed.
                stmts.push(quote! {
                    if __FUSED {
                        __w.write_raw_bytes(#lit.as_bytes())?;
                    } else {
                        if !__first {
                            __w.separator()?;
                        }
                        __w.object_key(#name_str)?;
                    }
                    ::json_bourne::ToJson::write_json(#value, __w)?;
                    __first = false;
                });
                comma = Comma::Always;
            } else {
                let key = &f.key;
                let lead = Self::runtime_comma(comma);
                stmts.push(quote! {
                    #lead
                    if __FUSED {
                        __w.write_escaped_str(#key)?;
                        __w.write_raw_bytes(b":")?;
                    } else {
                        __w.object_key(#key)?;
                    }
                    ::json_bourne::ToJson::write_json(#value, __w)?;
                    __first = false;
                });
                comma = if comma == Comma::Runtime {
                    Comma::Runtime
                } else {
                    Comma::Always
                };
            }
        }

        quote! { #(#stmts)* }
    }

    /// The separator before a member whose key cannot be fused into a
    /// literal (renamed, conditional, or following a conditional).
    /// Comma emission is structural: every sink renders it, `__FUSED`
    /// only decides how the following key+colon travel.
    fn runtime_comma(comma: Comma) -> TokenStream {
        match comma {
            Comma::Never => quote! {},
            Comma::Always => quote! { __w.separator()?; },
            Comma::Runtime => quote! { if !__first { __w.separator()?; } },
        }
    }

    /// The destructuring pattern `field: __fN, ...` for a struct variant.
    pub(crate) fn variant_pattern(fields: &[FieldPlan<'_>]) -> TokenStream {
        let pat = fields.iter().filter_map(FieldPlan::pattern_entry);
        quote! { #(#pat),* }
    }
}
