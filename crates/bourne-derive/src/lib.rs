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

/// Is this type spelled `Option<...>`? Textual on the last path segment,
/// like the macro's detection generally. A type alias
/// (`type Maybe<T> = Option<T>`) therefore reads as required — same
/// limitation as serde's derive, so behavioral compatibility wins over
/// deeper type resolution here.
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

/// Match-arm head for a JSON key, literal when possible so rustc builds a
/// length-switched byte compare instead of evaluating guards one by one
/// (audit 4.3.1).
///
/// Plain and explicitly renamed keys are string-literal patterns
/// directly. `rename_all` keys are not known textually at expansion time,
/// but `Casing::convert` is a `const fn`: the renamed key is hoisted into
/// a named `const` (unique per owner so several cased keys can share one
/// match) and the const name is the pattern. Returns the const
/// declaration to emit beside the match — empty for literal keys — and
/// the arm head. Anything that cannot be constant falls back to a guard
/// arm, which stays correct.
fn key_arm_head(
    key: &proc_macro2::TokenStream,
    owner: &Ident,
    binder: &str,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let text = key.to_string();
    let is_literal = syn::parse_str::<syn::Expr>(&text)
        .ok()
        .is_some_and(|e| matches!(e, syn::Expr::Lit(_)));
    if is_literal {
        return (quote!(), quote! { #key });
    }

    // key_expr's rename_all shape: `{ const __BOURNE_KEY: &str = <expr>;
    // __BOURNE_KEY }`. Recover `<expr>` and re-emit it as a uniquely
    // named const item beside the match.
    let cased = syn::parse_str::<syn::Block>(&text).ok().and_then(|b| {
        let Some(syn::Stmt::Item(syn::Item::Const(item))) = b.stmts.into_iter().next() else {
            return None;
        };
        let syn::ItemConst { expr, .. } = &item;
        let cname = Ident::new(
            &format!("__BOURNE_KEY_{}", owner.to_string().to_ascii_uppercase()),
            proc_macro2::Span::call_site(),
        );
        Some((quote! { const #cname: &str = #expr; }, cname))
    });
    if let Some((decl, cname)) = cased {
        return (decl, quote! { #cname });
    }

    let b = Ident::new(binder, proc_macro2::Span::call_site());
    (quote!(), quote! { #b if #b == #key })
}

#[cfg(test)]
mod key_arm_head_tests {
    use super::*;
    use proc_macro2::Span;

    fn block(text: &str) -> proc_macro2::TokenStream {
        text.parse().unwrap()
    }

    #[test]
    fn string_literal_yields_pattern_no_decl() {
        let key = quote! { "userId" };
        let (decl, head) = key_arm_head(&key, &Ident::new("user_id", Span::call_site()), "__b");
        assert!(decl.is_empty());
        assert_eq!(head.to_string(), "\"userId\"");
    }

    #[test]
    fn rename_all_const_is_hoisted_with_owner_name() {
        let key = block(
            "{ const __BOURNE_KEY: &str = ::json_bourne::__Casing::rename(\"user_id\", \"camelCase\").as_str(); __BOURNE_KEY }",
        );
        let (decl, head) = key_arm_head(&key, &Ident::new("user_id", Span::call_site()), "__b");
        assert_eq!(
            decl.to_string(),
            "const __BOURNE_KEY_USER_ID : & str = :: json_bourne :: __Casing :: rename (\"user_id\" , \"camelCase\") . as_str () ;"
        );
        assert_eq!(head.to_string(), "__BOURNE_KEY_USER_ID");
    }

    #[test]
    fn different_owners_get_different_consts() {
        let key = block("{ const __BOURNE_KEY: &str = \"x\"; __BOURNE_KEY }");
        let (_, h1) = key_arm_head(&key, &Ident::new("ab", Span::call_site()), "__b");
        let (_, h2) = key_arm_head(&key, &Ident::new("cd", Span::call_site()), "__b");
        assert_ne!(h1.to_string(), h2.to_string());
        assert_eq!(h1.to_string(), "__BOURNE_KEY_AB");
        assert_eq!(h2.to_string(), "__BOURNE_KEY_CD");
    }

    #[test]
    fn unexpected_shape_falls_back_to_guard() {
        let key = quote! { make_key("x") };
        let (decl, head) = key_arm_head(&key, &Ident::new("f", Span::call_site()), "__b");
        assert!(decl.is_empty());
        assert_eq!(head.to_string(), "__b if __b == make_key (\"x\")");
    }

    fn acquire(ty: &str) -> String {
        let ty: syn::Type = syn::parse_str(ty).unwrap();
        super::acquire_expr(&ty).to_string()
    }

    #[test]
    fn borrowed_str_fast_path_accepts_any_lifetime() {
        for ty in ["&str", "&'input str", "&'a str", "&'listener str"] {
            assert_eq!(
                acquire(ty),
                "__lex . parse_str_value () ?",
                "fast path for {ty}"
            );
        }
    }

    #[test]
    fn non_str_references_fall_through_to_generic() {
        assert_ne!(acquire("&'a String"), "__lex . parse_str_value () ?");
        assert_ne!(acquire("&'a mut str"), "__lex . parse_str_value () ?");
        assert_ne!(acquire("&'a [u8]"), "__lex . parse_str_value () ?");
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
    Struct(Vec<VariantField<'a>>),
}

/// A struct-variant field with its parsed attributes and resolved JSON
/// key expression (explicit `rename` wins, else the enum's `rename_all`,
/// else the verbatim name).
struct VariantField<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    /// Fresh binding for the writer's destructuring pattern.
    bind: Ident,
    key: proc_macro2::TokenStream,
    /// No rename and no container `rename_all`: the JSON key equals the
    /// Rust field name, so the writer can fuse it into byte literals.
    plain: bool,
    attrs: FieldAttrs,
}

fn classify_variant<'a>(
    v: &'a syn::Variant,
    rename_all: &Option<String>,
) -> syn::Result<VShape<'a>> {
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
        Fields::Unit => VShape::Unit,
        Fields::Unnamed(u) if u.unnamed.len() == 1 => VShape::Newtype(&u.unnamed[0].ty),
        Fields::Unnamed(u) => VShape::Tuple(u.unnamed.iter().map(|f| &f.ty).collect()),
        Fields::Named(n) => {
            let mut fields = Vec::new();
            for (i, f) in n.named.iter().enumerate() {
                let ident = f.ident.as_ref().unwrap();
                let attrs = parse_field_attrs(&f.attrs)?;
                let plain = attrs.rename.is_none() && rename_all.is_none();
                let key = key_expr(ident, &attrs.rename, rename_all);
                let bind = Ident::new(&format!("__f{i}"), ident.span());
                fields.push(VariantField {
                    ident,
                    ty: &f.ty,
                    bind,
                    key,
                    plain,
                    attrs,
                });
            }
            VShape::Struct(fields)
        }
    })
}

/// Reproduce `__from_json_acquire!`: the integer/`&str` fast paths that
/// bypass the generic `FromJson::from_lex` dispatch. Fairness for benches
/// depends on matching these exactly.
/// Whether `ty` is exactly `str` — a last path segment named `str` with no
/// further qualification.
fn is_bare_str(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.qself.is_none() && tp.path.get_ident().is_some_and(|i| i == "str"))
}

fn acquire_expr(ty: &Type) -> proc_macro2::TokenStream {
    // `&str` and `&'a str` for any lifetime: match on type structure, not
    // the stringified type, so a lifetime name other than `'input` still
    // takes the direct string path (audit 4.3.4).
    if let Type::Reference(tr) = ty {
        if tr.mutability.is_none() && is_bare_str(&tr.elem) {
            return quote! { __lex.parse_str_value()? };
        }
    }
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

/// Object-walk loop for derive-generated `FromJson`: borrowed-first key
/// acquisition. Escape-free keys come back as `KeyCow::Borrowed` straight
/// from the borrowing read (`object_first_key_str` /
/// `object_next_key_str`); an escape-bearing key rejects with
/// `InvalidEscape` and the cursor rewound, which the advance loop turns
/// into a decoding retry via `object_key_cow`. Equivalent to the `_lex` +
/// `key_to_cow` sequence for every input, without the span round-trip on
/// the common path (audit 4.3.2).
fn object_key_walk(
    arms: &[proc_macro2::TokenStream],
    unknown_arm: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
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
            Fields::Unnamed(unnamed) if !unnamed.unnamed.is_empty() => {
                from_json_tuple(&unnamed.unnamed)?
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new(
                    input.span(),
                    "empty tuple structs are unsupported: to_json emits `[]` but \
                     the parser rejects an empty array for a zero-field tuple",
                ));
            }
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
    let mut key_consts = Vec::new();
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
        let (kc, head) = key_arm_head(&key, name, "__bourne_k");
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

    let walk = object_key_walk(&arms, &unknown_arm);
    Ok(quote! {
        __lex.object_start()?;
        #(#decls)*
        #(#key_consts)*
        #walk
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
        EnumMode::Untagged => from_json_enum_untagged(name, variants, container),
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
/// Read a struct-variant's object body into `ctor { .. }`. Field handling
/// mirrors `from_json_named`: `rename`d keys match by their JSON key,
/// `#[bourne(skip)]` fields take `Default::default()` and never match a
/// key, `default` fields fall back to `Default` when absent, everything
/// else is required. `tag_arm` extends the key match when the variant
/// shares its object with the enum's tag (internal mode); the walk
/// declares `__bourne_tag_seen` for that arm's duplicate check.
fn struct_variant_read(
    ctor: &proc_macro2::TokenStream,
    fields: &[VariantField<'_>],
    tag_arm: Option<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let mut decls = Vec::new();
    let mut key_consts = Vec::new();
    let mut arms = Vec::new();
    let mut assigns = Vec::new();
    for f in fields {
        let fname = f.ident;
        let ty = f.ty;
        if f.attrs.skip {
            assigns.push(quote! { #fname: ::core::default::Default::default(), });
            continue;
        }
        let acquire = acquire_expr(ty);
        let key = &f.key;
        decls.push(quote! {
            let mut #fname: ::core::option::Option<#ty> = ::core::option::Option::None;
        });
        let (kc, head) = key_arm_head(key, fname, "__bourne_k");
        key_consts.push(kc);
        arms.push(quote! {
            #head => {
                if #fname.is_some() {
                    return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                }
                #fname = ::core::option::Option::Some(#acquire);
            }
        });
        let fin = if f.attrs.default {
            quote! { #fname.unwrap_or_else(::core::default::Default::default) }
        } else if is_option(ty) {
            quote! { #fname.unwrap_or(::core::option::Option::None) }
        } else {
            quote! {
                #fname.ok_or_else(|| ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField, __lex.position()))?
            }
        };
        assigns.push(quote! { #fname: #fin, });
    }
    let tag_seen_decl = if tag_arm.is_some() {
        quote! { let mut __bourne_tag_seen = false; }
    } else {
        quote!()
    };
    let fallback_arm = quote! {
        _ => {
            return ::core::result::Result::Err(::json_bourne::Error::new(
                ::json_bourne::ErrorKind::UnknownField, __lex.position()));
        }
    };
    let mut walk_arms = arms.to_vec();
    if let Some(arm) = tag_arm {
        walk_arms.push(arm);
    }
    let walk = object_key_walk(&walk_arms, &fallback_arm);
    quote! {{
        __lex.object_start()?;
        #(#decls)*
        #(#key_consts)*
        #tag_seen_decl
        #walk
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
    let mut key_consts = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let key = key_expr(vname, &rename, &container.rename_all);
        let ctor = quote!( #name::#vname );
        let (kc, head) = key_arm_head(&key, vname, "__bourne_k");
        key_consts.push(kc);
        match classify_variant(v, &container.rename_all)? {
            VShape::Unit => unit_arms.push(quote! {
                #head => ::core::result::Result::Ok(#ctor),
            }),
            VShape::Newtype(ty) => tagged_arms.push(quote! {
                #head =>
                    #ctor(<#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?),
            }),
            VShape::Tuple(tys) => {
                let body = tuple_variant_read(&ctor, &tys);
                tagged_arms.push(quote! {
                    #head => { #body },
                });
            }
            VShape::Struct(fields) => {
                let body = struct_variant_read(&ctor, &fields, None);
                tagged_arms.push(quote! {
                    #head => #body,
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
                #(#key_consts)*
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
                // Read the tag via the borrow-or-decode path so an escape-
                // bearing `rename` (which the serializer now escapes) still
                // matches; `parse_str_value` would reject the escapes.
                let __key_js = __lex.read_string_no_validate()?;
                let __tag_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
                #(#key_consts)*
                match __tag_cow.as_ref() {
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
    let mut key_consts = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let key = key_expr(vname, &rename, &container.rename_all);
        let ctor = quote!( #name::#vname );
        let (kc, head) = key_arm_head(&key, vname, "__bourne_t");
        key_consts.push(kc);
        match classify_variant(v, &container.rename_all)? {
            VShape::Unit => arms.push(quote! {
                #head => {
                    // Unit variant: the tag key may appear once; a
                    // repeat is a duplicate, anything else is unknown.
                    __lex.object_start()?;;
                    let mut __seen_tag = false;
                    let mut __mk = __lex.object_first_key_lex()?;
                    while let ::core::option::Option::Some(__kjs) = __mk {
                        let __kc = ::json_bourne::key_to_cow(__kjs, __lex)?;
                        if __kc.as_ref() == #tag {
                            if __seen_tag {
                                return ::core::result::Result::Err(::json_bourne::Error::new(
                                    ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                            }
                            __seen_tag = true;
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
                arms.push(quote! { #head => #body, });
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
        let __tag_value: ::json_bourne::KeyCow<'_> = loop {
            let ::core::option::Option::Some(__key_js) = __maybe_key else {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::MissingField, __lex.position()));
            };
            let __key_cow = ::json_bourne::key_to_cow(__key_js, __lex)?;
            if __key_cow.as_ref() == #tag {
                break ::json_bourne::key_to_cow(__lex.read_string_no_validate()?, __lex)?;
            }
            __lex.skip_value()?;
            __maybe_key = __lex.object_next_key_lex()?;
        };
        __lex.restore(__cp);
        #(#key_consts)*
        match __tag_value.as_ref() {
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
    fields: &[VariantField<'_>],
    tag: &str,
) -> proc_macro2::TokenStream {
    // The tag key shares the variant's object; it must appear exactly
    // once — the walk's first occurrence is skipped, a second is a
    // duplicate key, matching named structs.
    let tag_arm = quote! {
        #tag => {
            if __bourne_tag_seen {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
            }
            __bourne_tag_seen = true;
            __lex.skip_value()?;
        }
    };
    let body = struct_variant_read(ctor, fields, Some(tag_arm));
    quote! { ::core::result::Result::Ok({ #body }) }
}

fn from_json_enum_adjacent(
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    container: &ContainerAttrs,
    tag: &str,
    content: &str,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut arms = Vec::new();
    let mut key_consts = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let rename = parse_variant_rename(&v.attrs)?;
        let key = key_expr(vname, &rename, &container.rename_all);
        let ctor = quote!( #name::#vname );
        let (kc, head) = key_arm_head(&key, vname, "__bourne_t");
        key_consts.push(kc);
        match classify_variant(v, &container.rename_all)? {
            VShape::Unit => arms.push(quote! {
                #head => {
                    if __content_cp.is_some() {
                        ::core::result::Result::Err(::json_bourne::Error::new(
                            ::json_bourne::ErrorKind::UnknownField, __lex.position()))
                    } else {
                        ::core::result::Result::Ok(#ctor)
                    }
                }
            }),
            VShape::Newtype(ty) => arms.push(quote! {
                #head => match __content_cp {
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
                    #head => match __content_cp {
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
                let body = struct_variant_read(&ctor, &fields, None);
                arms.push(quote! {
                    #head => match __content_cp {
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
        let mut __tag_value: ::core::option::Option<::json_bourne::KeyCow<'_>> =
            ::core::option::Option::None;
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
                __tag_value = ::core::option::Option::Some(
                    ::json_bourne::key_to_cow(__lex.read_string_no_validate()?, __lex)?);
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
        let __tag = __tag_value
            .ok_or_else(|| ::json_bourne::Error::new(
                ::json_bourne::ErrorKind::MissingField, __lex.position()))?;
        let __post_cp = __lex.checkpoint();
        #(#key_consts)*
        let __value = match __tag.as_ref() {
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
    container: &ContainerAttrs,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut attempts = Vec::new();
    for v in variants {
        let vname = &v.ident;
        let ctor = quote!( #name::#vname );
        let try_body = match classify_variant(v, &container.rename_all)? {
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
                let body = struct_variant_read(&ctor, &fields, None);
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
            Fields::Unnamed(unnamed) if !unnamed.unnamed.is_empty() => {
                to_json_tuple(unnamed.unnamed.len())
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new(
                    input.span(),
                    "empty tuple structs are unsupported: to_json emits `[]` but \
                     the parser rejects an empty array for a zero-field tuple",
                ));
            }
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
    stmts.push(quote! { __w.begin_object()?; });
    stmts.push(quote! {
        #[allow(unused_assignments, unused_mut, unused_variables)]
        let mut __first: bool = true;
    });
    // Derived writers fuse `,"key":` into one literal for speed. That
    // fusion is only valid when the sink renders raw bytes verbatim;
    // the pretty sink needs the comma/key/colon as separate structural
    // events. The associated const is a compile-time constant per sink
    // instantiation, so the branch folds away and neither path costs
    // anything at runtime.
    stmts.push(quote! {
        let __FUSED: bool = <__W as ::json_bourne::JsonWrite>::FUSES_STRUCTURAL_BYTES;
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
                StaticFirst::No => quote! { if __FUSED { __w.separator()?; } },
                StaticFirst::Maybe => {
                    quote! { if !__first && __FUSED { __w.separator()?; } }
                }
            };
            stmts.push(quote! {
                if let ::core::option::Option::Some(ref __v) = self.#name {
                    #comma
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
                if __FUSED {
                    __w.write_raw_bytes(#lit.as_bytes())?;
                } else {
                    if !__first {
                        __w.separator()?;
                    }
                    __w.object_key(#name_str)?;
                }
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
                StaticFirst::No => quote! { if __FUSED { __w.separator()?; } },
                StaticFirst::Maybe => {
                    quote! { if !__first && __FUSED { __w.separator()?; } }
                }
            };
            stmts.push(quote! {
                #comma
                if __FUSED {
                    __w.write_escaped_str(#key)?;
                    __w.write_raw_bytes(b":")?;
                } else {
                    __w.object_key(#key)?;
                }
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

    stmts.push(quote! { __w.end_object()?; });
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
    stmts.push(quote! { __w.begin_array()?; });
    for i in 0..n {
        let idx = syn::Index::from(i);
        if i > 0 {
            stmts.push(quote! { __w.separator()?; });
        }
        stmts.push(quote! { ::json_bourne::ToJson::write_json(&self.#idx, __w)?; });
    }
    stmts.push(quote! { __w.end_array()?; });
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
        arms.push(to_json_variant_arm(
            name,
            v,
            vname,
            &tagkey,
            &mode,
            &container.rename_all,
        )?);
    }
    Ok(quote! {
        match self {
            #(#arms)*
        }
    })
}

/// Write a struct-variant's fields as `"k":v,...` (no surrounding braces).
/// Plain fields fuse `(comma)?key:` into compile-time literals; renamed
/// fields write their key through the escaping writer. `skip` fields are
/// never emitted. `emitted`: whether something (the internal-mode tag) is
/// already on the wire, making the first field's comma mandatory.
fn write_variant_fields(fields: &[VariantField<'_>], emitted: bool) -> proc_macro2::TokenStream {
    let mut stmts = Vec::new();
    stmts.push(quote! {
        let __FUSED: bool = <__W as ::json_bourne::JsonWrite>::FUSES_STRUCTURAL_BYTES;
        let mut __first = !(#emitted);
    });
    for f in fields {
        if f.attrs.skip {
            continue;
        }
        let bind = &f.bind;
        if f.plain {
            let name_str = f.ident.to_string();
            let lit = format!("\"{name_str}\":");
            let lit_comma = format!(",\"{name_str}\":");
            stmts.push(quote! {
                if __first {
                    if __FUSED {
                        __w.write_raw_bytes(#lit.as_bytes())?;
                    } else {
                        __w.object_key(#name_str)?;
                    }
                    __first = false;
                } else {
                    if __FUSED {
                        __w.write_raw_bytes(#lit_comma.as_bytes())?;
                    } else {
                        __w.separator()?;
                        __w.object_key(#name_str)?;
                    }
                }
                ::json_bourne::ToJson::write_json(#bind, __w)?;
            });
        } else {
            let key = &f.key;
            stmts.push(quote! {
                if !__first {
                    __w.separator()?;
                }
                __first = false;
                __w.object_key(#key)?;
                ::json_bourne::ToJson::write_json(#bind, __w)?;
            });
        }
    }
    quote! { #(#stmts)* }
}

/// Destructuring pattern `field: __fN, ...` for a struct variant.
fn variant_field_pat(fields: &[VariantField<'_>]) -> proc_macro2::TokenStream {
    let pat = fields.iter().map(|f| {
        let fname = f.ident;
        let bind = &f.bind;
        quote! { #fname: #bind }
    });
    quote! { #(#pat),* }
}

fn to_json_variant_arm(
    name: &Ident,
    v: &syn::Variant,
    vname: &Ident,
    tagkey: &proc_macro2::TokenStream,
    mode: &EnumMode,
    rename_all: &Option<String>,
) -> syn::Result<proc_macro2::TokenStream> {
    let shape = classify_variant(v, rename_all)?;
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
            let pat = variant_field_pat(fields);
            let body = write_variant_fields(fields, false);
            quote! {
                #name::#vname { #pat } => {
                    __w.begin_object()?;
                    #body
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            }
        }

        // ---- External ----
        (EnumMode::External, VShape::Unit) => quote! {
            #name::#vname => {
                __w.write_escaped_str(#tagkey)?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::External, VShape::Newtype(_)) => quote! {
            #name::#vname(__inner) => {
                __w.begin_object()?;
                __w.object_key(#tagkey)?;
                ::json_bourne::ToJson::write_json(__inner, __w)?;
                __w.end_object()?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::External, VShape::Tuple(tys)) => {
            let binds = variant_field_idents(tys.len());
            let writes = tuple_payload_writes(&binds);
            quote! {
                #name::#vname( #(#binds),* ) => {
                    __w.begin_object()?;
                    __w.object_key(#tagkey)?;
                    #writes
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            }
        }
        (EnumMode::External, VShape::Struct(fields)) => {
            let pat = variant_field_pat(fields);
            let body = write_variant_fields(fields, false);
            quote! {
                #name::#vname { #pat } => {
                    __w.begin_object()?;
                    __w.object_key(#tagkey)?;
                    __w.begin_object()?;
                    #body
                    __w.end_object()?;
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            }
        }

        // ---- Internal ----
        (EnumMode::Internal(tag), VShape::Unit) => quote! {
            #name::#vname => {
                __w.begin_object()?;
                __w.object_key(#tag)?;
                __w.write_escaped_str(#tagkey)?;
                __w.end_object()?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Internal(tag), VShape::Struct(fields)) => {
            let pat = variant_field_pat(fields);
            // Internal mode always has the tag first, so fields use the
            // comma-leading form unconditionally.
            let mut stmts = Vec::new();
            for f in fields {
                if f.attrs.skip {
                    continue;
                }
                let bind = &f.bind;
                if f.plain {
                    let key = f.ident.to_string();
                    let lit = format!(",\"{key}\":");
                    stmts.push(quote! {
                        if <__W as ::json_bourne::JsonWrite>::FUSES_STRUCTURAL_BYTES {
                            __w.write_raw_bytes(#lit.as_bytes())?;
                        } else {
                            __w.separator()?;
                            __w.object_key(#key)?;
                        }
                        ::json_bourne::ToJson::write_json(#bind, __w)?;
                    });
                } else {
                    let key = &f.key;
                    stmts.push(quote! {
                        __w.separator()?;
                        __w.object_key(#key)?;
                        ::json_bourne::ToJson::write_json(#bind, __w)?;
                    });
                }
            }
            quote! {
                #name::#vname { #pat } => {
                    __w.begin_object()?;
                    __w.object_key(#tag)?;
                    __w.write_escaped_str(#tagkey)?;
                    #(#stmts)*
                    __w.end_object()?;
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
                __w.begin_object()?;
                __w.object_key(#tag)?;
                __w.write_escaped_str(#tagkey)?;
                __w.end_object()?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Adjacent(tag, content), VShape::Newtype(_)) => quote! {
            #name::#vname(__inner) => {
                __w.begin_object()?;
                __w.object_key(#tag)?;
                __w.write_escaped_str(#tagkey)?;
                __w.separator()?;
                __w.object_key(#content)?;
                ::json_bourne::ToJson::write_json(__inner, __w)?;
                __w.end_object()?;
                ::core::result::Result::Ok(())
            }
        },
        (EnumMode::Adjacent(tag, content), VShape::Tuple(tys)) => {
            let binds = variant_field_idents(tys.len());
            let writes = tuple_payload_writes(&binds);
            quote! {
                #name::#vname( #(#binds),* ) => {
                    __w.begin_object()?;
                    __w.object_key(#tag)?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.separator()?;
                    __w.object_key(#content)?;
                    #writes
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            }
        }
        (EnumMode::Adjacent(tag, content), VShape::Struct(fields)) => {
            let pat = variant_field_pat(fields);
            let body = write_variant_fields(fields, false);
            quote! {
                #name::#vname { #pat } => {
                    __w.begin_object()?;
                    __w.object_key(#tag)?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.separator()?;
                    __w.object_key(#content)?;
                    __w.begin_object()?;
                    #body
                    __w.end_object()?;
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            }
        }
    })
}

/// For a tuple payload write bindings `__f0..` as a JSON array.
fn tuple_payload_writes(binds: &[Ident]) -> proc_macro2::TokenStream {
    let mut stmts = Vec::new();
    stmts.push(quote! { __w.begin_array()?; });
    for (i, b) in binds.iter().enumerate() {
        if i > 0 {
            stmts.push(quote! { __w.separator()?; });
        }
        stmts.push(quote! { ::json_bourne::ToJson::write_json(#b, __w)?; });
    }
    stmts.push(quote! { __w.end_array()?; });
    quote! { #(#stmts)* }
}

#[cfg(test)]
mod empty_tuple_rejection_tests {
    use super::{from_json_impl, to_json_impl};
    use syn::DeriveInput;

    fn derive(src: &str) -> syn::Result<()> {
        let input = syn::parse_str::<DeriveInput>(src).unwrap();
        from_json_impl(&input)?;
        to_json_impl(&input)?;
        Ok(())
    }

    #[test]
    fn empty_tuple_struct_is_rejected() {
        let err = derive("struct Empty();").unwrap_err();
        assert!(
            err.to_string()
                .contains("empty tuple structs are unsupported"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn empty_tuple_variant_is_rejected_in_every_mode() {
        for src in [
            "enum E { V() }",
            "#[bourne(untagged)] enum E { V() }",
            "#[bourne(tag = \"t\")] enum E { V() }",
            "#[bourne(tag = \"t\", content = \"c\")] enum E { V() }",
        ] {
            let err = derive(src).unwrap_err();
            assert!(
                err.to_string()
                    .contains("empty tuple variants are unsupported"),
                "{src}: unexpected error: {err}"
            );
        }
    }

    #[test]
    fn non_empty_tuples_still_derive() {
        derive("struct T(u8);").unwrap();
        derive("struct T(u8, u16);").unwrap();
        derive("enum E { V(u8, u16), U }").unwrap();
    }
}
