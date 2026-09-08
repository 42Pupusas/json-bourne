//! The per-field model shared by named structs and enum struct variants.
//!
//! Both shapes are "a set of named fields with attributes, read from and
//! written to a JSON object". They previously had two independent
//! implementations, and the variant one kept omitting things the struct
//! one handled — audit §3.6 (`rename`/`rename_all`/`skip`/`default`
//! ignored), then `skip_if_none` and `deny_unknown_fields` after the
//! first fix. Building one `FieldPlan` per field and having every code
//! path consume it is what stops that recurring: a new attribute is
//! added here and both shapes get it.
//!
//! The only genuine difference between the two is how the value is
//! reached — `self.name` for a struct, a destructuring binding for a
//! variant — which `Access` captures.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Type};

use crate::attrs::FieldAttrs;
use crate::naming::Naming;

/// How the writer reaches a field's value.
pub(crate) enum Access {
    /// `self.ident` — a named struct's field.
    SelfField,
    /// A binding introduced by a `match` pattern — a variant's field.
    Binding(Ident),
}

pub(crate) struct FieldPlan<'a> {
    pub(crate) ident: &'a Ident,
    pub(crate) ty: &'a Type,
    /// The JSON key as an expression (a literal, or a hoisted `const`
    /// for `rename_all`).
    pub(crate) key: TokenStream,
    /// No `rename` and no container `rename_all`: the JSON key is
    /// exactly the Rust field name, so the writer may fuse it into a
    /// compile-time byte literal.
    pub(crate) plain: bool,
    pub(crate) attrs: FieldAttrs,
    access: Access,
}

impl<'a> FieldPlan<'a> {
    pub(crate) fn new(
        ident: &'a Ident,
        ty: &'a Type,
        attrs: FieldAttrs,
        rename_all: &Option<String>,
        access: Access,
    ) -> Self {
        let plain = attrs.rename.is_none() && rename_all.is_none();
        let key = Naming::key_expr(ident, &attrs.rename, rename_all);
        Self {
            ident,
            ty,
            key,
            plain,
            attrs,
            access,
        }
    }

    /// Plan every named field of a struct.
    pub(crate) fn for_struct(
        fields: &'a syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
        rename_all: &Option<String>,
    ) -> syn::Result<Vec<Self>> {
        fields
            .iter()
            .map(|f| {
                let ident = f.ident.as_ref().unwrap();
                let attrs = FieldAttrs::parse(&f.attrs)?;
                Ok(Self::new(
                    ident,
                    &f.ty,
                    attrs,
                    rename_all,
                    Access::SelfField,
                ))
            })
            .collect()
    }

    /// Plan every named field of an enum struct variant. Each field gets
    /// a fresh `__fN` binding for the writer's destructuring pattern.
    pub(crate) fn for_variant(
        fields: &'a syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
        rename_all: &Option<String>,
    ) -> syn::Result<Vec<Self>> {
        fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let ident = f.ident.as_ref().unwrap();
                let attrs = FieldAttrs::parse(&f.attrs)?;
                let bind = Ident::new(&format!("__f{i}"), ident.span());
                Ok(Self::new(
                    ident,
                    &f.ty,
                    attrs,
                    rename_all,
                    Access::Binding(bind),
                ))
            })
            .collect()
    }

    /// Expression yielding a reference to this field's value.
    pub(crate) fn value_ref(&self) -> TokenStream {
        match &self.access {
            Access::SelfField => {
                let ident = self.ident;
                quote! { &self.#ident }
            }
            Access::Binding(bind) => quote! { #bind },
        }
    }

    /// Pattern for matching `Some(inner)` out of an `Option` field, and
    /// the identifier the inner value is bound to.
    pub(crate) fn option_scrutinee(&self) -> TokenStream {
        match &self.access {
            Access::SelfField => {
                let ident = self.ident;
                quote! { self.#ident }
            }
            Access::Binding(bind) => quote! { *#bind },
        }
    }

    /// The destructuring pattern entry `name: __fN`, for variants only.
    pub(crate) fn pattern_entry(&self) -> Option<TokenStream> {
        match &self.access {
            Access::SelfField => None,
            Access::Binding(bind) => {
                let ident = self.ident;
                Some(quote! { #ident: #bind })
            }
        }
    }

    /// Is this field written conditionally (`skip_if_none` on an
    /// `Option`)? A `skip_if_none` on a non-`Option` field is meaningless
    /// and is treated as unconditional, matching the named-struct path.
    pub(crate) fn is_conditional(&self) -> bool {
        self.attrs.skip_if_none && Naming::is_option(self.ty)
    }
}
