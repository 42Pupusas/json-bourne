//! `syn`-based derive macros for `json-bourne`.
//!
//! This crate is not used directly. Enable the `derive` feature on
//! `json-bourne` and use `#[derive(FromJson, ToJson)]` — the derives are
//! re-exported from the main crate so there is a single import surface.
//!
//! Generated code targets the crate runtime (`Lexer` / `JsonWrite` /
//! `FromJson` / `ToJson`) via `::json_bourne::…` paths that resolve in
//! the consumer's crate graph.
//!
//! Supported shapes: named and tuple structs, and all four enum tagging
//! modes (external, internal `#[bourne(tag)]`, adjacent `#[bourne(tag,
//! content)]`, untagged), both directions. Field attributes: `rename`,
//! `skip`, `default`, `skip_if_none`. Container attributes: `rename_all`,
//! `deny_unknown_fields`, `tag`, `content`, `untagged`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, Fields, GenericParam, Ident, Lifetime, LifetimeParam, Type,
    parse_macro_input, spanned::Spanned,
};

// ---------------------------------------------------------------------------
// Attribute model. One small struct replaces the dozens of `cur_*` slots the
// tt-muncher threads through every recursion.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FieldAttrs {
    rename: Option<String>,
    skip: bool,
    default: bool,
    skip_if_none: bool,
}

#[derive(Default)]
struct ContainerAttrs {
    rename_all: Option<String>,
    /// `true` when `#[bourne(deny_unknown_fields = false)]` was set —
    /// i.e. unknown keys are skipped rather than erroring.
    lenient: bool,
    // Enum-only container attrs.
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
}

/// Which tagging scheme an enum uses, derived from container attrs.
enum EnumMode {
    External,
    Internal(String),
    Adjacent(String, String),
    Untagged,
}

impl ContainerAttrs {
    fn enum_mode(&self) -> EnumMode {
        if self.untagged {
            EnumMode::Untagged
        } else if let (Some(t), Some(c)) = (&self.tag, &self.content) {
            EnumMode::Adjacent(t.clone(), c.clone())
        } else if let Some(t) = &self.tag {
            EnumMode::Internal(t.clone())
        } else {
            EnumMode::External
        }
    }
}

fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for attr in attrs {
        if !attr.path().is_ident("bourne") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let v = meta.value()?;
                let s: syn::LitStr = v.parse()?;
                out.rename = Some(s.value());
            } else if meta.path.is_ident("skip") {
                out.skip = true;
            } else if meta.path.is_ident("default") {
                out.default = true;
            } else if meta.path.is_ident("skip_if_none") {
                out.skip_if_none = true;
            } else {
                return Err(meta.error("unknown bourne field attribute"));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn parse_container_attrs(attrs: &[syn::Attribute]) -> syn::Result<ContainerAttrs> {
    let mut out = ContainerAttrs::default();
    for attr in attrs {
        if !attr.path().is_ident("bourne") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                let v = meta.value()?;
                let s: syn::LitStr = v.parse()?;
                out.rename_all = Some(s.value());
            } else if meta.path.is_ident("deny_unknown_fields") {
                // `= false` toggles lenient; bare presence is strict (default).
                if let Ok(v) = meta.value() {
                    let b: syn::LitBool = v.parse()?;
                    // The macro's flag names the *lenient* toggle: the
                    // `deny_unknown_fields = false` container form. Store
                    // "lenient == !deny".
                    out.lenient = !b.value();
                } else {
                    out.lenient = false;
                }
            } else if meta.path.is_ident("tag") {
                let v = meta.value()?;
                let s: syn::LitStr = v.parse()?;
                out.tag = Some(s.value());
            } else if meta.path.is_ident("content") {
                let v = meta.value()?;
                let s: syn::LitStr = v.parse()?;
                out.content = Some(s.value());
            } else if meta.path.is_ident("untagged") {
                out.untagged = true;
            } else {
                return Err(meta.error("unknown bourne container attribute"));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Helpers shared by both directions.
// ---------------------------------------------------------------------------

/// Is this type spelled `Option<...>`? (Textual, matching the macro's
/// leading-`Option`-ident detection — good enough and identical in effect.)
fn is_option(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident == "Option";
        }
    }
    false
}

/// The JSON key expression for a field: explicit rename wins, else
/// container `rename_all`, else the verbatim name. Yields a `&str`
/// expression (used in a match guard, mirroring the macro).
fn key_expr(
    name: &Ident,
    rename: &Option<String>,
    rename_all: &Option<String>,
) -> proc_macro2::TokenStream {
    if let Some(r) = rename {
        quote! { #r }
    } else if let Some(case) = rename_all {
        let name_str = name.to_string();
        quote! {{
            const __BOURNE_KEY: &str =
                ::json_bourne::__Casing::rename(#name_str, #case).as_str();
            __BOURNE_KEY
        }}
    } else {
        let name_str = name.to_string();
        quote! { #name_str }
    }
}

/// Parse a variant-level `#[bourne(rename = "...")]` (the only variant attr).
fn parse_variant_rename(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    let mut rename = None;
    for attr in attrs {
        if !attr.path().is_ident("bourne") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let v = meta.value()?;
                let s: syn::LitStr = v.parse()?;
                rename = Some(s.value());
                Ok(())
            } else {
                Err(meta.error("unsupported variant attribute in prototype"))
            }
        })?;
    }
    Ok(rename)
}

/// The classified shape of an enum variant.
enum VShape<'a> {
    Unit,
    Newtype(&'a Type),
    Tuple(Vec<&'a Type>),
    Struct(Vec<(&'a Ident, &'a Type)>),
}

fn classify_variant(v: &syn::Variant) -> VShape<'_> {
    match &v.fields {
        Fields::Unit => VShape::Unit,
        Fields::Unnamed(u) if u.unnamed.len() == 1 => VShape::Newtype(&u.unnamed[0].ty),
        Fields::Unnamed(u) => VShape::Tuple(u.unnamed.iter().map(|f| &f.ty).collect()),
        Fields::Named(n) => VShape::Struct(
            n.named
                .iter()
                .map(|f| (f.ident.as_ref().unwrap(), &f.ty))
                .collect(),
        ),
    }
}

/// Reproduce `__from_json_acquire!`: the integer/`&str` fast paths that
/// bypass the generic `FromJson::from_lex` dispatch. Fairness for benches
/// depends on matching these exactly.
fn acquire_expr(ty: &Type) -> proc_macro2::TokenStream {
    let ty_str = quote!(#ty).to_string().replace(' ', "");
    let int_narrow = |t: &str| {
        let t: proc_macro2::TokenStream = t.parse().unwrap();
        quote! {
            <#t>::try_from(__lex.parse_i64_value()?).map_err(|_| {
                ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::NumberOutOfRange,
                    __lex.position(),
                )
            })?
        }
    };
    let uint_narrow = |t: &str| {
        let t: proc_macro2::TokenStream = t.parse().unwrap();
        quote! {
            <#t>::try_from(__lex.parse_u64_value()?).map_err(|_| {
                ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::NumberOutOfRange,
                    __lex.position(),
                )
            })?
        }
    };
    match ty_str.as_str() {
        "&str" | "&'inputstr" => quote! { __lex.parse_str_value()? },
        "i64" => quote! { __lex.parse_i64_value()? },
        "u64" => quote! { __lex.parse_u64_value()? },
        "f64" => quote! { __lex.parse_f64_value()? },
        "f32" => quote! { __lex.parse_f64_value()? as f32 },
        "i8" => int_narrow("i8"),
        "i16" => int_narrow("i16"),
        "i32" => int_narrow("i32"),
        "isize" => int_narrow("isize"),
        "u8" => uint_narrow("u8"),
        "u16" => uint_narrow("u16"),
        "u32" => uint_narrow("u32"),
        "usize" => uint_narrow("usize"),
        "bool" => quote! {
            match __lex.read_value()? {
                ::json_bourne::Event::Bool(b) => b,
                _ => return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::ExpectedBool,
                    __lex.position(),
                )),
            }
        },
        _ => quote! {
            <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?
        },
    }
}

/// Split generics into the pieces both impls need, synthesizing the
/// `'input` lifetime the runtime trait is parameterized over. If the type
/// already carries a lifetime, reuse it (bourne's convention); otherwise
/// introduce a fresh `'input`.
///
/// This one function replaces the entire 4×-arm generics duplication in
/// `macros.rs` (none / lifetime / lifetime+type / type).
struct GenericsPlan {
    input_lt: Lifetime,
    /// Generics for the `impl<...>` — includes a fresh `'input` iff needed.
    impl_generics: proc_macro2::TokenStream,
    /// The `TypeName<...>` reference (original params only).
    ty_generics: proc_macro2::TokenStream,
    where_clause: proc_macro2::TokenStream,
}

fn plan_generics(input: &DeriveInput) -> GenericsPlan {
    let generics = &input.generics;
    let name = &input.ident;

    let existing_lt = generics.params.iter().find_map(|p| match p {
        GenericParam::Lifetime(lt) => Some(lt.lifetime.clone()),
        _ => None,
    });

    let (input_lt, needs_fresh) = match existing_lt {
        Some(lt) => (lt, false),
        None => (Lifetime::new("'input", name.span()), true),
    };

    // impl-side param list.
    let mut impl_params: Vec<proc_macro2::TokenStream> = Vec::new();
    if needs_fresh {
        let lp = LifetimeParam::new(input_lt.clone());
        impl_params.push(quote!(#lp));
    }
    for p in &generics.params {
        impl_params.push(quote!(#p));
    }

    // type-side param list (original params, names only).
    let ty_params: Vec<proc_macro2::TokenStream> = generics
        .params
        .iter()
        .map(|p| match p {
            GenericParam::Lifetime(lt) => {
                let l = &lt.lifetime;
                quote!(#l)
            }
            GenericParam::Type(t) => {
                let id = &t.ident;
                quote!(#id)
            }
            GenericParam::Const(c) => {
                let id = &c.ident;
                quote!(#id)
            }
        })
        .collect();

    let ty_generics = if ty_params.is_empty() {
        quote!()
    } else {
        quote!(< #(#ty_params),* >)
    };

    // where-clause: every type param must be FromJson<'input> (parse side).
    // ToJson doesn't need the bound; we emit a superset and let the ToJson
    // path build its own. Here we build the FromJson bounds.
    let type_param_bounds: Vec<proc_macro2::TokenStream> = generics
        .params
        .iter()
        .filter_map(|p| match p {
            GenericParam::Type(t) => {
                let id = &t.ident;
                Some(quote!(#id: ::json_bourne::FromJson<#input_lt>))
            }
            _ => None,
        })
        .collect();

    let where_clause = if type_param_bounds.is_empty() {
        quote!()
    } else {
        quote!(where #(#type_param_bounds),*)
    };

    GenericsPlan {
        input_lt,
        impl_generics: quote!(< #(#impl_params),* >),
        ty_generics,
        where_clause,
    }
}

// ===========================================================================
// FromJson derive
// ===========================================================================

#[proc_macro_derive(FromJson, attributes(bourne))]
pub fn derive_from_json(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match from_json_impl(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn from_json_impl(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let container = parse_container_attrs(&input.attrs)?;
    let plan = plan_generics(input);
    let input_lt = &plan.input_lt;
    let impl_generics = &plan.impl_generics;
    let ty_generics = &plan.ty_generics;
    let where_clause = &plan.where_clause;

    let body = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => from_json_named(&named.named, &container)?,
            Fields::Unnamed(unnamed) => from_json_tuple(&unnamed.unnamed)?,
            Fields::Unit => {
                return Err(syn::Error::new(
                    input.span(),
                    "unit structs have no JSON representation",
                ));
            }
        },
        Data::Enum(e) => from_json_enum(name, &e.variants, &container)?,
        Data::Union(_) => {
            return Err(syn::Error::new(input.span(), "unions are unsupported"));
        }
    };

    Ok(quote! {
        impl #impl_generics ::json_bourne::FromJson<#input_lt> for #name #ty_generics
            #where_clause
        {
            fn from_lex(
                __lex: &mut ::json_bourne::Lexer<#input_lt>,
            ) -> ::core::result::Result<Self, ::json_bourne::Error> {
                #body
            }
        }
    })
}

fn from_json_named(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    container: &ContainerAttrs,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut decls = Vec::new();
    let mut arms = Vec::new();
    let mut assigns = Vec::new();

    for field in fields {
        let name = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        let fa = parse_field_attrs(&field.attrs)?;

        if fa.skip {
            assigns.push(quote! { #name: ::core::default::Default::default(), });
            continue;
        }

        decls.push(quote! {
            let mut #name: ::core::option::Option<#ty> = ::core::option::Option::None;
        });

        let key = key_expr(name, &fa.rename, &container.rename_all);
        let acquire = acquire_expr(ty);
        arms.push(quote! {
            __bourne_k if __bourne_k == #key => {
                if #name.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::DuplicateKey,
                        __lex.position(),
                    ));
                }
                #name = ::core::option::Option::Some(#acquire);
            }
        });

        // finalize: skip already handled; then default / Option / required.
        let fin = if fa.default {
            quote! { #name.unwrap_or_else(::core::default::Default::default) }
        } else if is_option(ty) {
            quote! { #name.unwrap_or(::core::option::Option::None) }
        } else {
            quote! {
                #name.ok_or_else(|| ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField,
                    __lex.position(),
                ))?
            }
        };
        assigns.push(quote! { #name: #fin, });
    }

    let unknown_arm = if container.lenient {
        // `deny_unknown_fields = false` -> skip unknown keys. Default is strict.
        quote! { _ => { __lex.skip_value()?; } }
    } else {
        quote! {
            _ => {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::UnknownField,
                    __lex.position(),
                ));
            }
        }
    };

    Ok(quote! {
        __lex.object_start()?;
        #(#decls)*
        let mut __maybe_key = __lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
            match __key_cow.as_ref() {
                #(#arms)*
                #unknown_arm
            }
            __maybe_key = __lex.object_next_key_lex()?;
        }
        ::core::result::Result::Ok(Self { #(#assigns)* })
    })
}

fn from_json_tuple(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
) -> syn::Result<proc_macro2::TokenStream> {
    let n = fields.len();
    if n == 1 {
        let ty = &fields[0].ty;
        return Ok(quote! {
            ::core::result::Result::Ok(Self(
                <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?,
            ))
        });
    }

    // Multi-field: JSON array of exactly N elements.
    let mut reads = Vec::new();
    let mut idents = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let ty = &field.ty;
        let id = Ident::new(&format!("__elem_{i}"), field.span());
        if i == 0 {
            reads.push(quote! {
                let #id = <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
            });
        } else {
            reads.push(quote! {
                if __lex.array_continue(b']')? {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::TypeMismatch,
                        __lex.position(),
                    ));
                }
                let #id = <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
            });
        }
        idents.push(id);
    }

    Ok(quote! {
        if __lex.array_start()? {
            return ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::TypeMismatch,
                __lex.position(),
            ));
        }
        #(#reads)*
        if !__lex.array_continue(b']')? {
            return ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::TypeMismatch,
                __lex.position(),
            ));
        }
        ::core::result::Result::Ok(Self( #(#idents),* ))
    })
}

// ---------------------------------------------------------------------------
// Enum parse. One function, dispatching by tagging mode, replaces the four
// separate dispatch macros + their walkers (~700 lines of macros.rs).
// ---------------------------------------------------------------------------

fn from_json_enum(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    container: &ContainerAttrs,
) -> syn::Result<proc_macro2::TokenStream> {
    match container.enum_mode() {
        EnumMode::External => from_json_enum_external(name, variants, container),
        EnumMode::Internal(tag) => from_json_enum_internal(name, variants, container, &tag),
        EnumMode::Adjacent(tag, content) => {
            from_json_enum_adjacent(name, variants, container, &tag, &content)
        }
        EnumMode::Untagged => from_json_enum_untagged(name, variants),
    }
}

/// Read the array body for a tuple variant of arity `>= 2`, after the caller
/// has consumed `[` and read element 0. Shared by every tagging mode.
fn tuple_variant_read(ctor: &proc_macro2::TokenStream, tys: &[&Type]) -> proc_macro2::TokenStream {
    let mut reads = Vec::new();
    let mut idents = Vec::new();
    for (i, ty) in tys.iter().enumerate() {
        let id = Ident::new(&format!("__elem_{i}"), name_span());
        if i == 0 {
            reads.push(quote! {
                let #id = <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
            });
        } else {
            reads.push(quote! {
                if __lex.array_continue(b']')? {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::TypeMismatch, __lex.position()));
                }
                let #id = <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
            });
        }
        idents.push(id);
    }
    quote! {
        if __lex.array_start()? {
            return ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::TypeMismatch, __lex.position()));
        }
        #(#reads)*
        if !__lex.array_continue(b']')? {
            return ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::TypeMismatch, __lex.position()));
        }
        #ctor( #(#idents),* )
    }
}

fn name_span() -> proc_macro2::Span {
    proc_macro2::Span::call_site()
}

/// Emit the named-object body parse for a struct variant, constructing
/// `ctor { .. }`. Reuses the same object-walk shape as plain structs.
fn struct_variant_read(
    ctor: &proc_macro2::TokenStream,
    fields: &[(&Ident, &Type)],
) -> proc_macro2::TokenStream {
    let mut decls = Vec::new();
    let mut arms = Vec::new();
    let mut assigns = Vec::new();
    for (fname, ty) in fields {
        let key = fname.to_string();
        let acquire = acquire_expr(ty);
        decls.push(quote! {
            let mut #fname: ::core::option::Option<#ty> = ::core::option::Option::None;
        });
        arms.push(quote! {
            __bourne_k if __bourne_k == #key => {
                if #fname.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                }
                #fname = ::core::option::Option::Some(#acquire);
            }
        });
        let fin = if is_option(ty) {
            quote! { #fname.unwrap_or(::core::option::Option::None) }
        } else {
            quote! {
                #fname.ok_or_else(|| ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField, __lex.position()))?
            }
        };
        assigns.push(quote! { #fname: #fin, });
    }
    quote! {{
        __lex.object_start()?;
        #(#decls)*
        let mut __maybe_key = __lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
            match __key_cow.as_ref() {
                #(#arms)*
                _ => {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::UnknownField, __lex.position()));
                }
            }
            __maybe_key = __lex.object_next_key_lex()?;
        }
        #ctor { #(#assigns)* }
    }}
}

fn from_json_enum_external(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    container: &ContainerAttrs,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut unit_arms = Vec::new();
    let mut tagged_arms = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let key = key_expr(vname, &rename, &container.rename_all);
        let ctor = quote!( #name::#vname );
        match classify_variant(v) {
            VShape::Unit => unit_arms.push(quote! {
                __bourne_t if __bourne_t == #key => ::core::result::Result::Ok(#ctor),
            }),
            VShape::Newtype(ty) => tagged_arms.push(quote! {
                __bourne_t if __bourne_t == #key =>
                    #ctor(<#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?),
            }),
            VShape::Tuple(tys) => {
                let body = tuple_variant_read(&ctor, &tys);
                tagged_arms.push(quote! {
                    __bourne_t if __bourne_t == #key => { #body },
                });
            }
            VShape::Struct(fields) => {
                let body = struct_variant_read(&ctor, &fields);
                tagged_arms.push(quote! {
                    __bourne_t if __bourne_t == #key => #body,
                });
            }
        }
    }

    let object_dispatch = if tagged_arms.is_empty() {
        quote!()
    } else {
        quote! {
            ::json_bourne::ValueKind::Object => {
                __lex.object_start()?;
                let __key_js = __lex.object_first_key_lex()?.ok_or_else(|| {
                    ::json_bourne::Error::new(::json_bourne::ErrorKind::UnknownField, __lex.position())
                })?;
                let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
                let __value = match __key_cow.as_ref() {
                    #(#tagged_arms)*
                    _ => return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::UnknownField, __lex.position())),
                };
                if __lex.object_next_key_lex()?.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::UnknownField, __lex.position()));
                }
                ::core::result::Result::Ok(__value)
            }
        }
    };

    Ok(quote! {
        match __lex.peek_value_kind()? {
            ::json_bourne::ValueKind::String => {
                let __tag = __lex.parse_str_value()?;
                match __tag {
                    #(#unit_arms)*
                    _ => ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::UnknownField, __lex.position())),
                }
            }
            #object_dispatch
            _ => ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::TypeMismatch, __lex.position())),
        }
    })
}

fn from_json_enum_internal(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    container: &ContainerAttrs,
    tag: &str,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut arms = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let key = key_expr(vname, &rename, &container.rename_all);
        let ctor = quote!( #name::#vname );
        match classify_variant(v) {
            VShape::Unit => arms.push(quote! {
                __t if __t == #key => {
                    // Drain remaining keys; only the tag key is allowed.
                    __lex.object_start()?;
                    let mut __mk = __lex.object_first_key_lex()?;
                    while let ::core::option::Option::Some(__kjs) = __mk {
                        let __kc = ::json_bourne::key_to_cow(__kjs, __lex)?;
                        if __kc.as_ref() == #tag {
                            __lex.skip_value()?;
                        } else {
                            return ::core::result::Result::Err(::json_bourne::Error::new(
                                ::json_bourne::ErrorKind::UnknownField, __lex.position()));
                        }
                        __mk = __lex.object_next_key_lex()?;
                    }
                    ::core::result::Result::Ok(#ctor)
                }
            }),
            VShape::Struct(fields) => {
                let body = internal_struct_variant_read(&ctor, &fields, tag);
                arms.push(quote! { __t if __t == #key => #body, });
            }
            _ => {
                return Err(syn::Error::new(
                    v.ident.span(),
                    "internally-tagged enums support only unit and struct variants",
                ));
            }
        }
    }

    Ok(quote! {{
        let __cp = __lex.checkpoint();
        __lex.object_start()?;
        let mut __maybe_key = __lex.object_first_key_lex()?;
        let __tag_value: &str = loop {
            let ::core::option::Option::Some(__key_js) = __maybe_key else {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField, __lex.position()));
            };
            let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
            if __key_cow.as_ref() == #tag {
                break __lex.parse_str_value()?;
            }
            __lex.skip_value()?;
            __maybe_key = __lex.object_next_key_lex()?;
        };
        __lex.restore(__cp);
        match __tag_value {
            #(#arms)*
            _ => ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::UnknownField, __lex.position())),
        }
    }})
}

/// Struct-variant body for internally-tagged mode: the tag key is a sibling
/// and must be skipped (not treated as unknown).
fn internal_struct_variant_read(
    ctor: &proc_macro2::TokenStream,
    fields: &[(&Ident, &Type)],
    tag: &str,
) -> proc_macro2::TokenStream {
    let mut decls = Vec::new();
    let mut arms = Vec::new();
    let mut assigns = Vec::new();
    for (fname, ty) in fields {
        let key = fname.to_string();
        let acquire = acquire_expr(ty);
        decls.push(quote! {
            let mut #fname: ::core::option::Option<#ty> = ::core::option::Option::None;
        });
        arms.push(quote! {
            __bourne_k if __bourne_k == #key => {
                if #fname.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                }
                #fname = ::core::option::Option::Some(#acquire);
            }
        });
        let fin = if is_option(ty) {
            quote! { #fname.unwrap_or(::core::option::Option::None) }
        } else {
            quote! {
                #fname.ok_or_else(|| ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField, __lex.position()))?
            }
        };
        assigns.push(quote! { #fname: #fin, });
    }
    quote! {{
        __lex.object_start()?;
        #(#decls)*
        let mut __maybe_key = __lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
            match __key_cow.as_ref() {
                #(#arms)*
                __bourne_k if __bourne_k == #tag => { __lex.skip_value()?; }
                _ => {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::UnknownField, __lex.position()));
                }
            }
            __maybe_key = __lex.object_next_key_lex()?;
        }
        ::core::result::Result::Ok(#ctor { #(#assigns)* })
    }}
}

fn from_json_enum_adjacent(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    container: &ContainerAttrs,
    tag: &str,
    content: &str,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut arms = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let key = key_expr(vname, &rename, &container.rename_all);
        let ctor = quote!( #name::#vname );
        match classify_variant(v) {
            VShape::Unit => arms.push(quote! {
                __t if __t == #key => {
                    if __content_cp.is_some() {
                        ::core::result::Result::Err(::json_bourne::Error::new(
                            ::json_bourne::ErrorKind::UnknownField, __lex.position()))
                    } else {
                        ::core::result::Result::Ok(#ctor)
                    }
                }
            }),
            VShape::Newtype(ty) => arms.push(quote! {
                __t if __t == #key => match __content_cp {
                    ::core::option::Option::Some(__c) => {
                        __lex.restore(__c);
                        ::core::result::Result::Ok(#ctor(
                            <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?))
                    }
                    ::core::option::Option::None => ::core::result::Result::Err(
                        ::json_bourne::Error::new(::json_bourne::ErrorKind::MissingField, __lex.position())),
                }
            }),
            VShape::Tuple(tys) => {
                let body = tuple_variant_read(&ctor, &tys);
                arms.push(quote! {
                    __t if __t == #key => match __content_cp {
                        ::core::option::Option::Some(__c) => {
                            __lex.restore(__c);
                            ::core::result::Result::Ok({ #body })
                        }
                        ::core::option::Option::None => ::core::result::Result::Err(
                            ::json_bourne::Error::new(::json_bourne::ErrorKind::MissingField, __lex.position())),
                    }
                });
            }
            VShape::Struct(fields) => {
                let body = struct_variant_read(&ctor, &fields);
                arms.push(quote! {
                    __t if __t == #key => match __content_cp {
                        ::core::option::Option::Some(__c) => {
                            __lex.restore(__c);
                            ::core::result::Result::Ok(#body)
                        }
                        ::core::option::Option::None => ::core::result::Result::Err(
                            ::json_bourne::Error::new(::json_bourne::ErrorKind::MissingField, __lex.position())),
                    }
                });
            }
        }
    }

    Ok(quote! {{
        __lex.object_start()?;
        let mut __tag_value: ::core::option::Option<&str> = ::core::option::Option::None;
        let mut __content_cp: ::core::option::Option<::json_bourne::Checkpoint> =
            ::core::option::Option::None;
        let mut __maybe_key = __lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
            if __key_cow.as_ref() == #tag {
                if __tag_value.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                }
                __tag_value = ::core::option::Option::Some(__lex.parse_str_value()?);
            } else if __key_cow.as_ref() == #content {
                if __content_cp.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                }
                __content_cp = ::core::option::Option::Some(__lex.checkpoint());
                __lex.skip_value()?;
            } else {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::UnknownField, __lex.position()));
            }
            __maybe_key = __lex.object_next_key_lex()?;
        }
        let __tag = __tag_value.ok_or_else(|| ::json_bourne::Error::new(
            ::json_bourne::ErrorKind::MissingField, __lex.position()))?;
        let __post_cp = __lex.checkpoint();
        let __value = match __tag {
            #(#arms)*
            _ => ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::UnknownField, __lex.position())),
        }?;
        __lex.restore(__post_cp);
        ::core::result::Result::Ok(__value)
    }})
}

fn from_json_enum_untagged(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut attempts = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let ctor = quote!( #name::#vname );
        let try_body = match classify_variant(v) {
            VShape::Unit => quote! {
                <() as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
                ::core::result::Result::Ok(#ctor)
            },
            VShape::Newtype(ty) => quote! {
                ::core::result::Result::Ok(#ctor(
                    <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?))
            },
            VShape::Tuple(tys) => {
                let body = tuple_variant_read(&ctor, &tys);
                quote! { ::core::result::Result::Ok({ #body }) }
            }
            VShape::Struct(fields) => {
                let body = struct_variant_read(&ctor, &fields);
                quote! { ::core::result::Result::Ok(#body) }
            }
        };
        attempts.push(quote! {
            __lex.restore(__cp);
            let __try: ::core::result::Result<Self, ::json_bourne::Error> = (|| { #try_body })();
            if let ::core::result::Result::Ok(__v) = __try {
                return ::core::result::Result::Ok(__v);
            }
        });
    }
    Ok(quote! {{
        let __cp = __lex.checkpoint();
        #(#attempts)*
        ::core::result::Result::Err(::json_bourne::Error::new(
            ::json_bourne::ErrorKind::TypeMismatch, __lex.position()))
    }})
}

// ===========================================================================
// ToJson derive
// ===========================================================================

#[proc_macro_derive(ToJson, attributes(bourne))]
pub fn derive_to_json(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match to_json_impl(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn to_json_impl(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let container = parse_container_attrs(&input.attrs)?;

    // ToJson has no `'input` bound; use the type's own generics verbatim,
    // plus a `T: ToJson` bound per type param.
    let generics = &input.generics;
    let (impl_g, ty_g, _where_g) = generics.split_for_impl();
    let type_param_bounds: Vec<proc_macro2::TokenStream> = generics
        .params
        .iter()
        .filter_map(|p| match p {
            GenericParam::Type(t) => {
                let id = &t.ident;
                Some(quote!(#id: ::json_bourne::ToJson))
            }
            _ => None,
        })
        .collect();
    let where_clause = if type_param_bounds.is_empty() {
        quote!()
    } else {
        quote!(where #(#type_param_bounds),*)
    };

    let body = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => to_json_named(&named.named, &container)?,
            Fields::Unnamed(unnamed) => to_json_tuple(unnamed.unnamed.len()),
            Fields::Unit => {
                return Err(syn::Error::new(input.span(), "unit structs unsupported"));
            }
        },
        Data::Enum(e) => to_json_enum(name, &e.variants, &container)?,
        Data::Union(_) => {
            return Err(syn::Error::new(input.span(), "unions are unsupported"));
        }
    };

    Ok(quote! {
        impl #impl_g ::json_bourne::ToJson for #name #ty_g #where_clause {
            fn write_json<__W: ::json_bourne::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                #body
            }
        }
    })
}

fn to_json_named(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    container: &ContainerAttrs,
) -> syn::Result<proc_macro2::TokenStream> {
    // Split into "static" plain fields (comma placement known at compile
    // time -> fused `,"key":` literals) vs skip_if_none (runtime comma).
    // The macro does this with a `static_first` state token; here it's just
    // a running boolean over the field list.
    let mut stmts = Vec::new();
    stmts.push(quote! { __w.write_raw_bytes(b"{")?; });
    stmts.push(quote! {
        #[allow(unused_assignments, unused_mut, unused_variables)]
        let mut __first: bool = true;
    });

    // Static-comma tracking mirrors the macro's `static_first` token:
    //   `Yes`   -> nothing committed yet at compile time (first plain field
    //             elides its leading comma).
    //   `No`    -> a plain field definitely committed; leading comma is static.
    //   `Maybe` -> a prior skip_if_none makes comma posture runtime-dependent.
    #[derive(Clone, Copy, PartialEq)]
    enum StaticFirst {
        Yes,
        No,
        Maybe,
    }
    let mut sf = StaticFirst::Yes;

    for field in fields {
        let name = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        let fa = parse_field_attrs(&field.attrs)?;
        if fa.skip {
            continue;
        }

        let plain = fa.rename.is_none() && container.rename_all.is_none();

        if fa.skip_if_none && is_option(ty) {
            // Runtime-conditional field. Comma depends on prior emits.
            let key = key_expr(name, &fa.rename, &container.rename_all);
            let comma = match sf {
                StaticFirst::Yes => quote! {},
                StaticFirst::No => quote! { __w.write_raw_bytes(b",")?; },
                StaticFirst::Maybe => quote! { if !__first { __w.write_raw_bytes(b",")?; } },
            };
            stmts.push(quote! {
                if let ::core::option::Option::Some(ref __v) = self.#name {
                    #comma
                    __w.write_escaped_str(#key)?;
                    __w.write_raw_bytes(b":")?;
                    ::json_bourne::ToJson::write_json(__v, __w)?;
                    __first = false;
                }
            });
            sf = StaticFirst::Maybe;
            continue;
        }

        if plain && sf != StaticFirst::Maybe {
            // Fast path: fold (comma?)+key+colon into one compile-time literal,
            // no runtime branch. Matches the macro's headline emit.
            let name_str = name.to_string();
            let lit = match sf {
                StaticFirst::Yes => format!("\"{name_str}\":"),
                _ => format!(",\"{name_str}\":"),
            };
            // Record that output has been committed. A later skip_if_none
            // field (which flips posture to `Maybe`) gates its leading comma
            // on `!__first`; without this, plain fields would leave `__first`
            // set and that comma would be wrongly suppressed.
            stmts.push(quote! {
                __w.write_raw_bytes(#lit.as_bytes())?;
                ::json_bourne::ToJson::write_json(&self.#name, __w)?;
                __first = false;
            });
            sf = StaticFirst::No;
        } else {
            // Dynamic comma (rename/rename_all key may need escaping, or a
            // prior skip_if_none made posture runtime-dependent).
            let key = key_expr(name, &fa.rename, &container.rename_all);
            let comma = match sf {
                StaticFirst::Yes => quote! {},
                StaticFirst::No => quote! { __w.write_raw_bytes(b",")?; },
                StaticFirst::Maybe => quote! { if !__first { __w.write_raw_bytes(b",")?; } },
            };
            stmts.push(quote! {
                #comma
                __w.write_escaped_str(#key)?;
                __w.write_raw_bytes(b":")?;
                ::json_bourne::ToJson::write_json(&self.#name, __w)?;
                __first = false;
            });
            sf = if sf == StaticFirst::Maybe {
                StaticFirst::Maybe
            } else {
                StaticFirst::No
            };
        }
    }

    stmts.push(quote! { __w.write_raw_bytes(b"}")?; });
    stmts.push(quote! { ::core::result::Result::Ok(()) });
    Ok(quote! { #(#stmts)* })
}

fn to_json_tuple(n: usize) -> proc_macro2::TokenStream {
    if n == 1 {
        return quote! {
            ::json_bourne::ToJson::write_json(&self.0, __w)
        };
    }
    let mut stmts = Vec::new();
    stmts.push(quote! { __w.write_raw_bytes(b"[")?; });
    for i in 0..n {
        let idx = syn::Index::from(i);
        if i > 0 {
            stmts.push(quote! { __w.write_raw_bytes(b",")?; });
        }
        stmts.push(quote! { ::json_bourne::ToJson::write_json(&self.#idx, __w)?; });
    }
    stmts.push(quote! { __w.write_raw_bytes(b"]")?; });
    stmts.push(quote! { ::core::result::Result::Ok(()) });
    quote! { #(#stmts)* }
}

// ---------------------------------------------------------------------------
// Enum serialize.
// ---------------------------------------------------------------------------

/// Bindings + emit block for a variant's fields, shared across modes.
/// Returns (pattern_after_ident, body_that_writes_payload). The payload
/// writer for struct/tuple assumes it is writing the *bare* payload; each
/// mode wraps it with the appropriate framing.
fn variant_field_idents(n: usize) -> Vec<Ident> {
    (0..n)
        .map(|i| Ident::new(&format!("__f{i}"), name_span()))
        .collect()
}

fn to_json_enum(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    container: &ContainerAttrs,
) -> syn::Result<proc_macro2::TokenStream> {
    let mode = container.enum_mode();
    let mut arms = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let tagkey = key_expr(vname, &rename, &container.rename_all);
        arms.push(to_json_variant_arm(name, v, vname, &tagkey, &mode)?);
    }
    Ok(quote! {
        match self {
            #(#arms)*
        }
    })
}

/// Write a struct-variant's fields as `"k":v,...` (no surrounding braces).
fn write_named_fields(fields: &[(&Ident, Ident)]) -> proc_macro2::TokenStream {
    // fields: (json_key_ident_for_stringify, binding_ident)
    let mut stmts = Vec::new();
    stmts.push(quote! { let mut __first = true; });
    for (key_ident, bind) in fields {
        let key = key_ident.to_string();
        let lit = format!("\"{key}\":");
        let lit_comma = format!(",\"{key}\":");
        stmts.push(quote! {
            if __first {
                __w.write_raw_bytes(#lit.as_bytes())?;
                __first = false;
            } else {
                __w.write_raw_bytes(#lit_comma.as_bytes())?;
            }
            ::json_bourne::ToJson::write_json(#bind, __w)?;
        });
    }
    quote! { #(#stmts)* }
}

fn to_json_variant_arm(
    name: &Ident,
    v: &syn::Variant,
    vname: &Ident,
    tagkey: &proc_macro2::TokenStream,
    mode: &EnumMode,
) -> syn::Result<proc_macro2::TokenStream> {
    let shape = classify_variant(v);
    // Build pattern + payload writer generically, then frame per mode.
    Ok(match (mode, &shape) {
        // ---- Untagged ----
        (EnumMode::Untagged, VShape::Unit) => quote! {
            #name::#vname => { __w.write_raw_bytes(b"null")?; ::core::result::Result::Ok(()) }
        },
        (EnumMode::Untagged, VShape::Newtype(_)) => quote! {
            #name::#vname(__inner) => {
                ::json_bourne::ToJson::write_json(__inner, __w)?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Untagged, VShape::Tuple(tys)) => {
            let binds = variant_field_idents(tys.len());
            let writes = tuple_payload_writes(&binds);
            quote! {
                #name::#vname( #(#binds),* ) => { #writes ::core::result::Result::Ok(()) }
            }
        }
        (EnumMode::Untagged, VShape::Struct(fields)) => {
            let (pat, pairs) = struct_pat_and_pairs(fields);
            let body = write_named_fields(&pairs);
            quote! {
                #name::#vname { #pat } => {
                    __w.write_raw_bytes(b"{")?;
                    #body
                    __w.write_raw_bytes(b"}")?;
                    ::core::result::Result::Ok(())
                }
            }
        }

        // ---- External ----
        (EnumMode::External, VShape::Unit) => quote! {
            #name::#vname => {
                __w.write_raw_bytes(b"\"")?;
                __w.write_str_raw(#tagkey)?;
                __w.write_raw_bytes(b"\"")?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::External, VShape::Newtype(_)) => quote! {
            #name::#vname(__inner) => {
                __w.write_raw_bytes(b"{")?;
                __w.write_escaped_str(#tagkey)?;
                __w.write_raw_bytes(b":")?;
                ::json_bourne::ToJson::write_json(__inner, __w)?;
                __w.write_raw_bytes(b"}")?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::External, VShape::Tuple(tys)) => {
            let binds = variant_field_idents(tys.len());
            let writes = tuple_payload_writes(&binds);
            quote! {
                #name::#vname( #(#binds),* ) => {
                    __w.write_raw_bytes(b"{")?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.write_raw_bytes(b":")?;
                    #writes
                    __w.write_raw_bytes(b"}")?;
                    ::core::result::Result::Ok(())
                }
            }
        }
        (EnumMode::External, VShape::Struct(fields)) => {
            let (pat, pairs) = struct_pat_and_pairs(fields);
            let body = write_named_fields(&pairs);
            quote! {
                #name::#vname { #pat } => {
                    __w.write_raw_bytes(b"{")?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.write_raw_bytes(b":{")?;
                    #body
                    __w.write_raw_bytes(b"}}")?;
                    ::core::result::Result::Ok(())
                }
            }
        }

        // ---- Internal ----
        (EnumMode::Internal(tag), VShape::Unit) => quote! {
            #name::#vname => {
                __w.write_raw_bytes(b"{")?;
                __w.write_escaped_str(#tag)?;
                __w.write_raw_bytes(b":\"")?;
                __w.write_str_raw(#tagkey)?;
                __w.write_raw_bytes(b"\"}")?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Internal(tag), VShape::Struct(fields)) => {
            let (pat, pairs) = struct_pat_and_pairs(fields);
            // Internal mode always has the tag first, so fields use the
            // comma-leading form unconditionally.
            let mut stmts = Vec::new();
            for (key_ident, bind) in &pairs {
                let key = key_ident.to_string();
                let lit = format!(",\"{key}\":");
                stmts.push(quote! {
                    __w.write_raw_bytes(#lit.as_bytes())?;
                    ::json_bourne::ToJson::write_json(#bind, __w)?;
                });
            }
            quote! {
                #name::#vname { #pat } => {
                    __w.write_raw_bytes(b"{")?;
                    __w.write_escaped_str(#tag)?;
                    __w.write_raw_bytes(b":\"")?;
                    __w.write_str_raw(#tagkey)?;
                    __w.write_raw_bytes(b"\"")?;
                    #(#stmts)*
                    __w.write_raw_bytes(b"}")?;
                    ::core::result::Result::Ok(())
                }
            }
        }
        (EnumMode::Internal(_), _) => {
            return Err(syn::Error::new(
                v.ident.span(),
                "internally-tagged enums support only unit and struct variants",
            ));
        }

        // ---- Adjacent ----
        (EnumMode::Adjacent(tag, _content), VShape::Unit) => quote! {
            #name::#vname => {
                __w.write_raw_bytes(b"{")?;
                __w.write_escaped_str(#tag)?;
                __w.write_raw_bytes(b":\"")?;
                __w.write_str_raw(#tagkey)?;
                __w.write_raw_bytes(b"\"}")?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Adjacent(tag, content), VShape::Newtype(_)) => quote! {
            #name::#vname(__inner) => {
                __w.write_raw_bytes(b"{")?;
                __w.write_escaped_str(#tag)?;
                __w.write_raw_bytes(b":\"")?;
                __w.write_str_raw(#tagkey)?;
                __w.write_raw_bytes(b"\",")?;
                __w.write_escaped_str(#content)?;
                __w.write_raw_bytes(b":")?;
                ::json_bourne::ToJson::write_json(__inner, __w)?;
                __w.write_raw_bytes(b"}")?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Adjacent(tag, content), VShape::Tuple(tys)) => {
            let binds = variant_field_idents(tys.len());
            let writes = tuple_payload_writes(&binds);
            quote! {
                #name::#vname( #(#binds),* ) => {
                    __w.write_raw_bytes(b"{")?;
                    __w.write_escaped_str(#tag)?;
                    __w.write_raw_bytes(b":\"")?;
                    __w.write_str_raw(#tagkey)?;
                    __w.write_raw_bytes(b"\",")?;
                    __w.write_escaped_str(#content)?;
                    __w.write_raw_bytes(b":")?;
                    #writes
                    __w.write_raw_bytes(b"}")?;
                    ::core::result::Result::Ok(())
                }
            }
        }
        (EnumMode::Adjacent(tag, content), VShape::Struct(fields)) => {
            let (pat, pairs) = struct_pat_and_pairs(fields);
            let body = write_named_fields(&pairs);
            quote! {
                #name::#vname { #pat } => {
                    __w.write_raw_bytes(b"{")?;
                    __w.write_escaped_str(#tag)?;
                    __w.write_raw_bytes(b":\"")?;
                    __w.write_str_raw(#tagkey)?;
                    __w.write_raw_bytes(b"\",")?;
                    __w.write_escaped_str(#content)?;
                    __w.write_raw_bytes(b":{")?;
                    #body
                    __w.write_raw_bytes(b"}}")?;
                    ::core::result::Result::Ok(())
                }
            }
        }
    })
}

/// For a tuple payload write bindings `__f0..` as a JSON array.
fn tuple_payload_writes(binds: &[Ident]) -> proc_macro2::TokenStream {
    let mut stmts = Vec::new();
    stmts.push(quote! { __w.write_raw_bytes(b"[")?; });
    for (i, b) in binds.iter().enumerate() {
        if i > 0 {
            stmts.push(quote! { __w.write_raw_bytes(b",")?; });
        }
        stmts.push(quote! { ::json_bourne::ToJson::write_json(#b, __w)?; });
    }
    stmts.push(quote! { __w.write_raw_bytes(b"]")?; });
    quote! { #(#stmts)* }
}

/// Build a struct-variant binding pattern `field: __fN, ...` and the pairs
/// `(field_ident_for_key, binding_ident)`.
fn struct_pat_and_pairs<'a>(
    fields: &'a [(&'a Ident, &'a Type)],
) -> (proc_macro2::TokenStream, Vec<(&'a Ident, Ident)>) {
    let mut pat = Vec::new();
    let mut pairs = Vec::new();
    for (i, (fname, _ty)) in fields.iter().enumerate() {
        let bind = Ident::new(&format!("__f{i}"), name_span());
        pat.push(quote! { #fname: #bind });
        pairs.push((*fname, bind));
    }
    (quote! { #(#pat),* }, pairs)
}
