//! Reads a JSON object into a set of named fields.
//!
//! The parse-side twin of `to_json::object`: one implementation for
//! named structs and enum struct variants, so `rename`, `skip`,
//! `default` and `deny_unknown_fields` cannot drift apart between the
//! two (audit §3.6).

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::acquire::Acquire;
use crate::field_plan::FieldPlan;
use crate::naming::Naming;

/// What to do with a key that matches no field.
pub(crate) enum Unknown {
    Reject,
    Skip,
}

impl Unknown {
    fn arm(&self) -> TokenStream {
        match self {
            Self::Skip => quote! { _ => { __lex.skip_value()?; } },
            Self::Reject => quote! {
                _ => {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::UnknownField,
                        __lex.position(),
                    ));
                }
            },
        }
    }
}

pub(crate) struct ObjectReader;

impl ObjectReader {
    /// Read an object body into `ctor { .. }`, having already consumed
    /// nothing — this emits the `object_start` too.
    ///
    /// `tag_arm` extends the key match when the variant shares its
    /// object with the enum's tag (internal mode); the caller's arm is
    /// responsible for the tag's own duplicate check, and this declares
    /// the `__bourne_tag_seen` flag it uses.
    pub(crate) fn read(
        ctor: &TokenStream,
        fields: &[FieldPlan<'_>],
        unknown: &Unknown,
        tag_arm: Option<TokenStream>,
    ) -> TokenStream {
        let mut decls = Vec::new();
        let mut key_consts = Vec::new();
        let mut arms = Vec::new();
        let mut assigns = Vec::new();

        for f in fields {
            let name = f.ident;
            let ty = f.ty;
            if f.attrs.skip {
                assigns.push(quote! { #name: ::core::default::Default::default(), });
                continue;
            }

            decls.push(quote! {
                let mut #name: ::core::option::Option<#ty> = ::core::option::Option::None;
            });

            let acquire = Acquire::expr(ty);
            let (kc, head) = Naming::key_arm_head(&f.key, name, "__bourne_k");
            key_consts.push(kc);
            arms.push(quote! {
                #head => {
                    if #name.is_some() {
                        return ::core::result::Result::Err(::json_bourne::Error::new(
                            ::json_bourne::ErrorKind::DuplicateKey,
                            __lex.position(),
                        ));
                    }
                    #name = ::core::option::Option::Some(#acquire);
                }
            });

            assigns.push({
                let fin = Self::finalize(name, f);
                quote! { #name: #fin, }
            });
        }

        let tag_seen_decl = if tag_arm.is_some() {
            quote! { let mut __bourne_tag_seen = false; }
        } else {
            quote!()
        };
        let mut walk_arms = arms;
        if let Some(arm) = tag_arm {
            walk_arms.push(arm);
        }
        let walk = Self::key_walk(&walk_arms, &unknown.arm());

        quote! {{
            __lex.object_start()?;
            #(#decls)*
            #(#key_consts)*
            #tag_seen_decl
            #walk
            #ctor { #(#assigns)* }
        }}
    }

    /// Turn the collected `Option<T>` slot into the field's final value:
    /// `default` falls back to `Default`, an `Option` field defaults to
    /// `None`, anything else is required.
    fn finalize(name: &Ident, f: &FieldPlan<'_>) -> TokenStream {
        if f.attrs.default {
            quote! { #name.unwrap_or_else(::core::default::Default::default) }
        } else if Naming::is_option(f.ty) {
            quote! { #name.unwrap_or(::core::option::Option::None) }
        } else {
            quote! {
                #name.ok_or_else(|| ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField,
                    __lex.position(),
                ))?
            }
        }
    }

    /// Object-walk loop for derive-generated `FromJson`: borrowed-first key
    /// acquisition. Escape-free keys come back as `KeyCow::Borrowed` straight
    /// from the borrowing read (`object_first_key_str` /
    /// `object_next_key_str`); an escape-bearing key rejects with
    /// `InvalidEscape` and the cursor rewound, which the advance loop turns
    /// into a decoding retry via `object_key_cow`. Equivalent to the `_lex` +
    /// `key_to_cow` sequence for every input, without the span round-trip on
    /// the common path (audit 4.3.2).
    pub(crate) fn key_walk(arms: &[TokenStream], unknown_arm: &TokenStream) -> TokenStream {
        quote! {
            let mut __maybe_key = __lex.object_first_key_str()?;
            '__bourne_walk: loop {
                let __key_cow: ::json_bourne::KeyCow<'_> = match __maybe_key {
                    ::core::option::Option::Some(__k) => {
                        ::json_bourne::KeyCow::Borrowed(__k)
                    }
                    ::core::option::Option::None => break,
                };
                let __key: &str = __key_cow.as_ref();
                match __key {
                    #(#arms)*
                    #unknown_arm
                }
                loop {
                    match __lex.object_next_key_str() {
                        ::core::result::Result::Ok(__k) => {
                            __maybe_key = __k;
                            continue '__bourne_walk;
                        }
                        ::core::result::Result::Err(__e)
                            if __e.kind == ::json_bourne::ErrorKind::InvalidEscape =>
                        {
                            let __decoded = __lex.object_key_cow()?;
                            let __key: &str = __decoded.as_ref();
                            match __key {
                                #(#arms)*
                                #unknown_arm
                            }
                        }
                        ::core::result::Result::Err(__e) => {
                            return ::core::result::Result::Err(__e);
                        }
                    }
                }
            }
        }
    }
}
