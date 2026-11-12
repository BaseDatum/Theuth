//! Owner-local discovery metadata beside the unchanged jsonrpsee declaration.
//!
//! Put `#[rpc_descriptors(owner = "owner", descriptors = rpc_descriptors)]` before
//! `#[rpc(server, client)]`. The named function returns `rpc_core::Descriptors`.
//! Unsupported annotations/signatures are errors, never incomplete metadata.
//!
//! Method/trait `cfg` is supported; `cfg_attr`, generic traits, custom result
//! aliases and unknown annotations fail closed. Namespaces apply to canonical
//! method/notification/unsubscribe names, not aliases. Argument renames and
//! jsonrpsee's snake/camel named-input aliases are retained. Optionality follows
//! jsonrpsee's syntactic `Option` rule, not JSON Schema nullability.
//! Mixed-audience traits may override a method/subscription with
//! `#[rpc_audience("mesh")]` or `#[rpc_audience("owner")]`; the attribute is
//! consumed before jsonrpsee expands the unchanged RPC declaration.

use std::collections::BTreeSet;

use heck::{ToLowerCamelCase, ToSnakeCase};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::quote;
use syn::{Attribute, FnArg, GenericArgument, Ident, ItemTrait, LitStr, Pat, PathArguments, ReturnType, Token, TraitItem, Type, parse::Parser, spanned::Spanned};

/// Generate a named `fn() -> rpc_core::Descriptors` for an owner RPC trait.
/// Options: required `owner = "..."`, `descriptors = function_name`, and optional
/// `audience = "owner"` (default) or `"mesh"`. Place before jsonrpsee's `rpc`.
#[proc_macro_attribute]
pub fn rpc_descriptors(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args.into(), syn::parse_macro_input!(input as ItemTrait))
        .unwrap_or_else(syn::Error::into_compile_error).into()
}

struct Config {
    owner: LitStr,
    descriptors: Ident,
    audience: Tokens,
}

impl Config {
    fn parse(args: Tokens) -> syn::Result<Self> {
        let mut owner = None;
        let mut descriptors = None;
        let mut audience = quote!(::rpc_core::Audience::Owner);
        let mut seen = BTreeSet::new();
        syn::meta::parser(|meta| {
            unique(&mut seen, &meta.path)?;
            if meta.path.is_ident("owner") { owner = Some(meta.value()?.parse()?); }
            else if meta.path.is_ident("descriptors") { descriptors = Some(meta.value()?.parse()?); }
            else if meta.path.is_ident("audience") {
                let value: LitStr = meta.value()?.parse()?;
                audience = match value.value().as_str() {
                    "owner" => quote!(::rpc_core::Audience::Owner),
                    "mesh" => quote!(::rpc_core::Audience::Mesh),
                    _ => return Err(syn::Error::new_spanned(value, "audience must be owner or mesh")),
                };
            } else { return Err(meta.error("unsupported rpc_descriptors option")); }
            Ok(())
        }).parse2(args.clone())?;
        Ok(Self {
            owner: owner.ok_or_else(|| syn::Error::new_spanned(&args, "owner is required"))?,
            descriptors: descriptors.ok_or_else(|| syn::Error::new_spanned(&args, "descriptors function name is required"))?,
            audience,
        })
    }
}

fn unique(seen: &mut BTreeSet<String>, path: &syn::Path) -> syn::Result<()> {
    if !seen.insert(quote!(#path).to_string()) {
        return Err(syn::Error::new_spanned(path, "duplicate annotation option"));
    }
    Ok(())
}

fn check_attrs(attrs: &[Attribute], allowed: &[&str]) -> syn::Result<()> {
    for attr in attrs {
        let name = attr.path().segments.last().unwrap().ident.to_string();
        if !allowed.contains(&name.as_str()) {
            return Err(syn::Error::new_spanned(attr, "unsupported discovery annotation (cfg_attr and transforming attributes must not silently change the contract)"));
        }
    }
    Ok(())
}

#[derive(Default)]
struct RpcOptions {
    name: Option<LitStr>,
    notification: Option<LitStr>,
    unsubscribe: Option<LitStr>,
    aliases: Vec<LitStr>,
    unsubscribe_aliases: Vec<LitStr>,
    item: Option<Type>,
}

impl RpcOptions {
    fn parse(attr: &Attribute, subscription: bool) -> syn::Result<Self> {
        let mut options = Self::default();
        let mut seen = BTreeSet::new();
        attr.parse_nested_meta(|meta| {
            unique(&mut seen, &meta.path)?;
            if meta.path.is_ident("name") {
                let value = meta.value()?;
                options.name = Some(value.parse()?);
                if subscription && value.peek(Token![=>]) {
                    value.parse::<Token![=>]>()?;
                    options.notification = Some(value.parse()?);
                }
            } else if meta.path.is_ident("aliases") || (subscription && meta.path.is_ident("unsubscribe_aliases")) {
                let content;
                let value = meta.value()?;
                syn::bracketed!(content in value);
                let values = content.parse_terminated(|input| input.parse::<LitStr>(), Token![,])?.into_iter().collect();
                if meta.path.is_ident("aliases") { options.aliases = values; }
                else { options.unsubscribe_aliases = values; }
            } else if subscription && meta.path.is_ident("item") { options.item = Some(meta.value()?.parse()?); }
            else if subscription && meta.path.is_ident("unsubscribe") { options.unsubscribe = Some(meta.value()?.parse()?); }
            else if meta.path.is_ident("param_kind") {
                let kind: Ident = meta.value()?.parse()?;
                if kind != "array" && kind != "map" { return Err(meta.error("param_kind must be array or map")); }
                // This controls generated clients only; the server accepts both forms.
            } else if meta.path.is_ident("with_extensions") || (!subscription && meta.path.is_ident("blocking")) {
                // jsonrpsee injects Extensions/sinks into the generated server trait,
                // not the owner's user-parameter declaration.
            } else { return Err(meta.error("unsupported RPC annotation option")); }
            Ok(())
        })?;
        if options.name.is_none() { return Err(syn::Error::new_spanned(attr, "RPC name is required")); }
        Ok(options)
    }
}

fn expand(args: Tokens, mut item: ItemTrait) -> syn::Result<Tokens> {
    let config = Config::parse(args.clone())?;
    check_attrs(&item.attrs, &["rpc", "cfg", "doc", "allow", "warn", "deny", "forbid", "deprecated"])?;
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() || !item.supertraits.is_empty() || item.unsafety.is_some() || item.auto_token.is_some() {
        return Err(syn::Error::new_spanned(&item, "discovery requires a concrete, safe RPC trait without supertraits"));
    }
    let rpc: Vec<_> = item.attrs.iter().filter(|a| a.path().segments.last().unwrap().ident == "rpc").collect();
    if rpc.len() != 1 { return Err(syn::Error::new_spanned(&item, "put rpc_descriptors before exactly one #[rpc(server, client)]")); }
    let mut namespace: Option<LitStr> = None;
    let mut separator = "_".to_owned();
    let mut server = false;
    let mut seen = BTreeSet::new();
    rpc[0].parse_nested_meta(|meta| {
        unique(&mut seen, &meta.path)?;
        if meta.path.is_ident("server") { server = true; }
        else if meta.path.is_ident("client") {}
        else if meta.path.is_ident("namespace") { namespace = Some(meta.value()?.parse()?); }
        else if meta.path.is_ident("namespace_separator") { separator = meta.value()?.parse::<LitStr>()?.value(); }
        else { return Err(meta.error("unsupported #[rpc] discovery option")); }
        Ok(())
    })?;
    if !server { return Err(syn::Error::new_spanned(rpc[0], "discovery requires #[rpc(server)]")); }
    let cfg: Vec<_> = item.attrs.iter().filter(|a| a.path().is_ident("cfg")).collect();
    // jsonrpsee 0.26 drops method cfg attrs. Let rustc select a declaration
    // before jsonrpsee sees it, rather than advertising an unregistered method.
    for (index, entry) in item.items.iter().enumerate() {
        let TraitItem::Fn(method) = entry else { return Err(syn::Error::new_spanned(entry, "only RPC methods are supported")); };
        check_attrs(&method.attrs, &["method", "subscription", "rpc_audience", "cfg", "doc", "allow", "warn", "deny", "forbid", "deprecated"])?;
        if let Some((attr_index, attr)) = method.attrs.iter().enumerate().find(|(_, a)| a.path().is_ident("cfg")) {
            let predicate = &attr.meta.require_list()?.tokens;
            let mut enabled = item.clone();
            let TraitItem::Fn(method) = &mut enabled.items[index] else { unreachable!() };
            method.attrs.remove(attr_index);
            let mut disabled = item.clone();
            disabled.items.remove(index);
            return Ok(quote! {
                #(#cfg)*
                #[cfg(#predicate)]
                #[::rpc_core::rpc_descriptors(#args)]
                #enabled
                #(#cfg)*
                #[cfg(not(#predicate))]
                #[::rpc_core::rpc_descriptors(#args)]
                #disabled
            });
        }
    }
    let qualify = |name: &LitStr| -> LitStr {
        match &namespace {
            Some(ns) => LitStr::new(&format!("{}{separator}{}", ns.value(), name.value()), name.span()),
            None => name.clone(),
        }
    };
    let owner = &config.owner;
    let mut methods = Vec::new();
    let mut subscriptions = Vec::new();
    for entry in &item.items {
        let TraitItem::Fn(method) = entry else { unreachable!() };
        let overrides: Vec<_> = method.attrs.iter().filter(|a| a.path().is_ident("rpc_audience")).collect();
        if overrides.len() > 1 {
            return Err(syn::Error::new_spanned(method, "duplicate rpc_audience annotation"));
        }
        let audience = match overrides.first() {
            Some(attr) => {
                let value: LitStr = attr.parse_args()?;
                match value.value().as_str() {
                    "owner" => quote!(::rpc_core::Audience::Owner),
                    "mesh" => quote!(::rpc_core::Audience::Mesh),
                    _ => return Err(syn::Error::new_spanned(value, "audience must be owner or mesh")),
                }
            }
            None => config.audience.clone(),
        };
        let sig = &method.sig;
        if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() || sig.variadic.is_some() || sig.unsafety.is_some() || sig.constness.is_some() || sig.abi.is_some() || method.default.is_some() {
            return Err(syn::Error::new_spanned(sig, "unsupported RPC signature"));
        }
        let receiver = sig.receiver().ok_or_else(|| syn::Error::new_spanned(sig, "RPC methods require &self"))?;
        if receiver.reference.is_none() || receiver.mutability.is_some() || receiver.colon_token.is_some() || !receiver.attrs.is_empty() {
            return Err(syn::Error::new_spanned(receiver, "RPC methods require plain &self"));
        }
        let annotations: Vec<_> = method.attrs.iter().filter(|a| a.path().is_ident("method") || a.path().is_ident("subscription")).collect();
        if annotations.len() != 1 { return Err(syn::Error::new_spanned(method, "exactly one method or subscription annotation is required")); }
        let subscription = annotations[0].path().is_ident("subscription");
        let options = RpcOptions::parse(annotations[0], subscription)?;
        let name = options.name.as_ref().unwrap();
        let qualified = qualify(name);
        let aliases = &options.aliases;
        let params = params(sig)?;
        if subscription {
            let item_type = options.item.ok_or_else(|| syn::Error::new_spanned(method, "subscription item is required"))?;
            let unsubscribe = match options.unsubscribe {
                Some(name) => name,
                None => LitStr::new(&format!("unsubscribe{}", name.value().strip_prefix("subscribe").ok_or_else(|| syn::Error::new_spanned(name, "explicit unsubscribe name is required"))?), name.span()),
            };
            let unsubscribe = qualify(&unsubscribe);
            let notification = qualify(options.notification.as_ref().unwrap_or(name));
            let unsubscribe_aliases = &options.unsubscribe_aliases;
            subscriptions.push(quote!(::rpc_core::SubscriptionDescriptor {
                owner:#owner, subscribe:#qualified, unsubscribe:#unsubscribe, notification:#notification,
                aliases:vec![#(#aliases),*], unsubscribe_aliases:vec![#(#unsubscribe_aliases),*],
                params:vec![#(#params),*], item: ::rpc_core::schema_for::<#item_type>, audience:#audience,
            }));
        } else {
            let ReturnType::Type(_, result) = &sig.output else { return Err(syn::Error::new_spanned(sig, "unary results must be RpcResult<T>")); };
            let result = rpc_result(result)?;
            methods.push(quote!(::rpc_core::MethodDescriptor {
                owner:#owner, name:#qualified, aliases:vec![#(#aliases),*], params:vec![#(#params),*],
                result: ::rpc_core::schema_for::<#result>, audience:#audience,
            }));
        }
    }
    for entry in &mut item.items {
        if let TraitItem::Fn(method) = entry {
            method.attrs.retain(|a| !a.path().is_ident("rpc_audience"));
        }
    }
    let visibility = &item.vis;
    let function = &config.descriptors;
    Ok(quote! {
        #(#cfg)*
        #item
        #(#cfg)*
        #[doc = "Lazy RPC contracts generated from the adjacent owner trait."]
        #visibility fn #function() -> ::rpc_core::Descriptors {
            ::rpc_core::Descriptors { methods:vec![#(#methods),*], subscriptions:vec![#(#subscriptions),*] }
        }
    })
}

fn params(sig: &syn::Signature) -> syn::Result<Vec<Tokens>> {
    sig.inputs.iter().filter_map(|arg| match arg { FnArg::Typed(arg) => Some(arg), _ => None }).map(|arg| {
        check_attrs(&arg.attrs, &["argument"])?;
        let Pat::Ident(pattern) = &*arg.pat else { return Err(syn::Error::new_spanned(&arg.pat, "RPC arguments require identifier names")); };
        if pattern.by_ref.is_some() || pattern.subpat.is_some() { return Err(syn::Error::new_spanned(pattern, "unsupported argument pattern")); }
        let mut name = LitStr::new(&pattern.ident.to_string(), pattern.span());
        if arg.attrs.len() > 1 { return Err(syn::Error::new_spanned(arg, "duplicate argument annotation")); }
        for attr in &arg.attrs {
            let mut seen = BTreeSet::new();
            attr.parse_nested_meta(|meta| {
                unique(&mut seen, &meta.path)?;
                if !meta.path.is_ident("rename") { return Err(meta.error("unsupported argument annotation")); }
                name = meta.value()?.parse()?;
                Ok(())
            })?;
        }
        let aliases: BTreeSet<_> = [name.value().to_snake_case(), name.value().to_lower_camel_case()].into_iter().collect();
        let aliases: Vec<_> = aliases.into_iter().filter(|s| s != &name.value()).map(|s| LitStr::new(&s, name.span())).collect();
        let ty = &arg.ty;
        // Match jsonrpsee's syntax-based optional_next selection, not schema nullability.
        let optional = matches!(&**ty, Type::Path(path) if path.path.segments.last().is_some_and(|s| s.ident == "Option"));
        Ok(quote!(::rpc_core::ParamDescriptor { name:#name, aliases:vec![#(#aliases),*], optional:#optional, schema: ::rpc_core::schema_for::<#ty> }))
    }).collect()
}

fn rpc_result(ty: &Type) -> syn::Result<&Type> {
    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "RpcResult"
        && let PathArguments::AngleBracketed(args) = &segment.arguments
        && args.args.len() == 1
        && let GenericArgument::Type(ty) = &args.args[0]
    {
        return Ok(ty);
    }
    Err(syn::Error::new_spanned(ty, "unary results must be RpcResult<T>; custom result aliases are unsupported"))
}

#[cfg(test)]
mod tests;
