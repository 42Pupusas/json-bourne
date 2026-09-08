//! Variant shape classification.

use syn::{Fields, Type, Variant};

use crate::field_plan::FieldPlan;

/// The classified shape of an enum variant.
pub(crate) enum VShape<'a> {
    Unit,
    Newtype(&'a Type),
    Tuple(Vec<&'a Type>),
    Struct(Vec<FieldPlan<'a>>),
}

impl<'a> VShape<'a> {
    pub(crate) fn classify(v: &'a Variant, rename_all: &Option<String>) -> syn::Result<Self> {
        if let Fields::Unnamed(u) = &v.fields {
            if u.unnamed.is_empty() {
                return Err(syn::Error::new(
                    v.ident.span(),
                    "empty tuple variants are unsupported: they serialize as `[]` but \
                     the tuple reader rejects an empty array",
                ));
            }
        }
        Ok(match &v.fields {
            Fields::Unit => Self::Unit,
            Fields::Unnamed(u) if u.unnamed.len() == 1 => Self::Newtype(&u.unnamed[0].ty),
            Fields::Unnamed(u) => Self::Tuple(u.unnamed.iter().map(|f| &f.ty).collect()),
            Fields::Named(n) => Self::Struct(FieldPlan::for_variant(&n.named, rename_all)?),
        })
    }
}
