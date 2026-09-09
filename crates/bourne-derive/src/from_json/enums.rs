//! `FromJson` codegen for enums, one strategy per tagging mode.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Variant, punctuated::Punctuated};

use crate::attrs::{ContainerAttrs, EnumMode, VariantAttrs};
use crate::field_plan::FieldPlan;
use crate::from_json::FromJsonDerive;
use crate::from_json::object::{ObjectReader, Unknown};
use crate::naming::Naming;
use crate::shape::VShape;

type Variants = Punctuated<Variant, syn::Token![,]>;

pub(crate) struct EnumReader;

impl EnumReader {
    pub(crate) fn expand(
        name: &Ident,
        variants: &Variants,
        container: &ContainerAttrs,
    ) -> syn::Result<TokenStream> {
        match container.enum_mode()? {
            EnumMode::External => Self::external(name, variants, container),
            EnumMode::Internal(tag) => Self::internal(name, variants, container, &tag),
            EnumMode::Adjacent(tag, content) => {
                Self::adjacent(name, variants, container, &tag, &content)
            }
            EnumMode::Untagged => Self::untagged(name, variants, container),
        }
    }

    /// The variant's JSON tag key and its `match` arm head.
    fn tag_head(
        v: &Variant,
        container: &ContainerAttrs,
        binder: &str,
    ) -> syn::Result<(TokenStream, TokenStream)> {
        let attrs = VariantAttrs::parse(&v.attrs)?;
        let key = Naming::key_expr(&v.ident, &attrs.rename, &container.rename_all);
        Ok(Naming::key_arm_head(&key, &v.ident, binder))
    }

    fn external(
        name: &Ident,
        variants: &Variants,
        container: &ContainerAttrs,
    ) -> syn::Result<TokenStream> {
        let mut unit_arms = Vec::new();
        let mut tagged_arms = Vec::new();
        let mut key_consts = Vec::new();
        for v in variants {
            let vname = &v.ident;
            let ctor = quote!( #name::#vname );
            let (kc, head) = Self::tag_head(v, container, "__bourne_k")?;
            key_consts.push(kc);
            match VShape::classify(v, &container.rename_all)? {
                VShape::Unit => unit_arms.push(quote! {
                    #head => ::core::result::Result::Ok(#ctor),
                }),
                VShape::Newtype(ty) => tagged_arms.push(quote! {
                    #head =>
                        #ctor(<#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?),
                }),
                VShape::Tuple(tys) => {
                    let body = FromJsonDerive::tuple_body(&ctor, &tys);
                    tagged_arms.push(quote! { #head => #body, });
                }
                VShape::Struct(fields) => {
                    let body = ObjectReader::read(&ctor, &fields, &container.unknown(), None);
                    tagged_arms.push(quote! { #head => #body, });
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

    fn internal(
        name: &Ident,
        variants: &Variants,
        container: &ContainerAttrs,
        tag: &str,
    ) -> syn::Result<TokenStream> {
        let mut arms = Vec::new();
        let mut key_consts = Vec::new();
        for v in variants {
            let vname = &v.ident;
            let ctor = quote!( #name::#vname );
            let (kc, head) = Self::tag_head(v, container, "__bourne_t")?;
            key_consts.push(kc);
            match VShape::classify(v, &container.rename_all)? {
                VShape::Unit => arms.push(quote! {
                    #head => {
                        // Unit variant: the tag key may appear once; a
                        // repeat is a duplicate, anything else is unknown.
                        __lex.object_start()?;
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
                    let body = Self::internal_struct_body(&ctor, &fields, container, tag);
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

    /// Struct-variant body for internally-tagged mode: the tag key is a
    /// sibling of the fields and must be skipped, not treated as unknown.
    fn internal_struct_body(
        ctor: &TokenStream,
        fields: &[FieldPlan<'_>],
        container: &ContainerAttrs,
        tag: &str,
    ) -> TokenStream {
        // The tag shares the variant's object; it must appear exactly
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
        let body = ObjectReader::read(ctor, fields, &container.unknown(), Some(tag_arm));
        quote! { ::core::result::Result::Ok(#body) }
    }

    fn adjacent(
        name: &Ident,
        variants: &Variants,
        container: &ContainerAttrs,
        tag: &str,
        content: &str,
    ) -> syn::Result<TokenStream> {
        let mut arms = Vec::new();
        let mut key_consts = Vec::new();
        for v in variants {
            let vname = &v.ident;
            let ctor = quote!( #name::#vname );
            let (kc, head) = Self::tag_head(v, container, "__bourne_t")?;
            key_consts.push(kc);
            match VShape::classify(v, &container.rename_all)? {
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
                    let body = FromJsonDerive::tuple_body(&ctor, &tys);
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
                VShape::Struct(fields) => {
                    let body =
                        ObjectReader::read(&ctor, &fields, &container.unknown(), None);
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
            // Payload snapshot: taken at the content value's first byte and
            // consumed by the arm directly (audit 4.5.4). Set only when the
            // tag is already known, so a known-tag payload is never skipped.
            let mut __content_cp: ::core::option::Option<::json_bourne::Checkpoint> =
                ::core::option::Option::None;
            let mut __early = false;
            let mut __maybe_key = __lex.object_first_key_lex()?;
            'keys: loop {
                let ::core::option::Option::Some(__key_js) = __maybe_key else {
                    break;
                };
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
                    if __tag_value.is_some() {
                        __early = true;
                        break 'keys;
                    }
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
            #(#key_consts)*
            let __post_cp = __lex.checkpoint();
            let __value = match __tag.as_ref() {
                #(#arms)*
                _ => ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::UnknownField, __lex.position())),
            }?;
            if __early {
                // The arm read the payload forward and stopped at `,` or
                // `}`. The tail walk rejects leftover keys — a leftover #tag
                // is a duplicate (it was consumed before the content).
                // `object_next_key_lex` consumes the closing brace on its
                // `Ok(None)`, so the walk ends with the object closed.
                loop {
                    match __lex.object_next_key_lex()? {
                        ::core::option::Option::Some(__kjs) => {
                            let __kc = ::json_bourne::key_to_cow(__kjs, __lex)?;
                            if __kc.as_ref() == #tag {
                                return ::core::result::Result::Err(::json_bourne::Error::new(
                                    ::json_bourne::ErrorKind::DuplicateKey, __lex.position()));
                            }
                            return ::core::result::Result::Err(::json_bourne::Error::new(
                                ::json_bourne::ErrorKind::UnknownField, __lex.position()));
                        }
                        ::core::option::Option::None => break,
                    }
                }
            } else {
                // The arm restored and re-read the skipped payload; rewind
                // to the post-object cursor.
                __lex.restore(__post_cp);
            }
            ::core::result::Result::Ok(__value)
        }})
    }

    fn untagged(
        name: &Ident,
        variants: &Variants,
        container: &ContainerAttrs,
    ) -> syn::Result<TokenStream> {
        let mut attempts = Vec::new();
        for v in variants {
            let vname = &v.ident;
            let ctor = quote!( #name::#vname );
            let try_body = match VShape::classify(v, &container.rename_all)? {
                VShape::Unit => quote! {
                    <() as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
                    ::core::result::Result::Ok(#ctor)
                },
                VShape::Newtype(ty) => quote! {
                    ::core::result::Result::Ok(#ctor(
                        <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?))
                },
                VShape::Tuple(tys) => {
                    let body = FromJsonDerive::tuple_body(&ctor, &tys);
                    quote! { ::core::result::Result::Ok(#body) }
                }
                VShape::Struct(fields) => {
                    // Untagged discrimination is by trial: a lenient
                    // reader would make the first struct variant accept
                    // any object and shadow the rest, so unknown keys
                    // stay rejected here regardless of the container's
                    // `deny_unknown_fields`.
                    let body = ObjectReader::read(&ctor, &fields, &Unknown::Reject, None);
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
}
