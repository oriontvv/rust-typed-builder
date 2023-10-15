use std::{collections::HashSet, iter};

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, ToTokens};
use syn::{
    parenthesized,
    parse::{Parse, ParseStream, Parser},
    parse_quote,
    punctuated::Punctuated,
    spanned::Spanned,
    token, Attribute, Error, Expr, FnArg, ItemFn, Pat, PatIdent, ReturnType, Signature, Token, Type,
};

pub fn path_to_single_string(path: &syn::Path) -> Option<String> {
    if path.leading_colon.is_some() {
        return None;
    }
    let mut it = path.segments.iter();
    let segment = it.next()?;
    if it.next().is_some() {
        // Multipart path
        return None;
    }
    if segment.arguments != syn::PathArguments::None {
        return None;
    }
    Some(segment.ident.to_string())
}

pub fn ident_to_type(ident: syn::Ident) -> syn::Type {
    let mut path = syn::Path {
        leading_colon: None,
        segments: Default::default(),
    };
    path.segments.push(syn::PathSegment {
        ident,
        arguments: Default::default(),
    });
    syn::Type::Path(syn::TypePath { qself: None, path })
}

pub fn empty_type() -> syn::Type {
    syn::TypeTuple {
        paren_token: Default::default(),
        elems: Default::default(),
    }
    .into()
}

pub fn type_tuple(elems: impl Iterator<Item = syn::Type>) -> syn::TypeTuple {
    let mut result = syn::TypeTuple {
        paren_token: Default::default(),
        elems: elems.collect(),
    };
    if !result.elems.empty_or_trailing() {
        result.elems.push_punct(Default::default());
    }
    result
}

pub fn empty_type_tuple() -> syn::TypeTuple {
    syn::TypeTuple {
        paren_token: Default::default(),
        elems: Default::default(),
    }
}

pub fn modify_types_generics_hack<F>(ty_generics: &syn::TypeGenerics, mut mutator: F) -> syn::AngleBracketedGenericArguments
where
    F: FnMut(&mut syn::punctuated::Punctuated<syn::GenericArgument, syn::token::Comma>),
{
    let mut abga: syn::AngleBracketedGenericArguments =
        syn::parse2(ty_generics.to_token_stream()).unwrap_or_else(|_| syn::AngleBracketedGenericArguments {
            colon2_token: None,
            lt_token: Default::default(),
            args: Default::default(),
            gt_token: Default::default(),
        });
    mutator(&mut abga.args);
    abga
}

pub fn strip_raw_ident_prefix(mut name: String) -> String {
    if name.starts_with("r#") {
        name.replace_range(0..2, "");
    }
    name
}

pub fn first_visibility(visibilities: &[Option<&syn::Visibility>]) -> proc_macro2::TokenStream {
    let vis = visibilities
        .iter()
        .flatten()
        .next()
        .expect("need at least one visibility in the list");

    vis.to_token_stream()
}

pub fn public_visibility() -> syn::Visibility {
    syn::Visibility::Public(syn::token::Pub::default())
}

pub fn expr_to_lit_string(expr: &syn::Expr) -> Result<String, Error> {
    match expr {
        syn::Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Str(str) => Ok(str.value()),
            _ => Err(Error::new_spanned(expr, "attribute only allows str values")),
        },
        _ => Err(Error::new_spanned(expr, "attribute only allows str values")),
    }
}

pub enum AttrArg {
    Flag(Ident),
    KeyValue(KeyValue),
    Sub(SubAttr),
    Not { not: Token![!], name: Ident },
}

impl AttrArg {
    pub fn name(&self) -> &Ident {
        match self {
            AttrArg::Flag(name) => name,
            AttrArg::KeyValue(KeyValue { name, .. }) => name,
            AttrArg::Sub(SubAttr { name, .. }) => name,
            AttrArg::Not { name, .. } => name,
        }
    }

    pub fn incorrect_type(&self) -> syn::Error {
        let message = match self {
            AttrArg::Flag(name) => format!("{:?} is not supported as a flag", name.to_string()),
            AttrArg::KeyValue(KeyValue { name, .. }) => format!("{:?} is not supported as key-value", name.to_string()),
            AttrArg::Sub(SubAttr { name, .. }) => format!("{:?} is not supported as nested attribute", name.to_string()),
            AttrArg::Not { name, .. } => format!("{:?} cannot be nullified", name.to_string()),
        };
        syn::Error::new_spanned(self, message)
    }

    pub fn flag(self) -> syn::Result<Ident> {
        if let Self::Flag(name) = self {
            Ok(name)
        } else {
            Err(self.incorrect_type())
        }
    }

    pub fn key_value(self) -> syn::Result<KeyValue> {
        if let Self::KeyValue(key_value) = self {
            Ok(key_value)
        } else {
            Err(self.incorrect_type())
        }
    }

    pub fn key_value_or_not(self) -> syn::Result<Option<KeyValue>> {
        match self {
            Self::KeyValue(key_value) => Ok(Some(key_value)),
            Self::Not { .. } => Ok(None),
            _ => Err(self.incorrect_type()),
        }
    }

    pub fn sub_attr(self) -> syn::Result<SubAttr> {
        if let Self::Sub(sub_attr) = self {
            Ok(sub_attr)
        } else {
            Err(self.incorrect_type())
        }
    }

    pub fn apply_flag_to_field(self, field: &mut Option<Span>, caption: &str) -> syn::Result<()> {
        match self {
            AttrArg::Flag(flag) => {
                if field.is_none() {
                    *field = Some(flag.span());
                    Ok(())
                } else {
                    Err(Error::new(
                        flag.span(),
                        format!("Illegal setting - field is already {caption}"),
                    ))
                }
            }
            AttrArg::Not { .. } => {
                *field = None;
                Ok(())
            }
            _ => Err(self.incorrect_type()),
        }
    }
}

pub struct KeyValue {
    pub name: Ident,
    pub eq: Token![=],
    pub value: TokenStream,
}

impl KeyValue {
    pub fn parse_value<T: Parse>(self) -> syn::Result<T> {
        syn::parse2(self.value)
    }
}

impl ToTokens for KeyValue {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.name.to_tokens(tokens);
        self.eq.to_tokens(tokens);
        self.value.to_tokens(tokens);
    }
}

impl Parse for KeyValue {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            name: input.parse()?,
            eq: input.parse()?,
            value: input.parse()?,
        })
    }
}

pub struct SubAttr {
    pub name: Ident,
    pub paren: token::Paren,
    pub args: TokenStream,
}

impl SubAttr {
    pub fn args<T: Parse>(self) -> syn::Result<impl IntoIterator<Item = T>> {
        Punctuated::<T, Token![,]>::parse_terminated.parse2(self.args)
    }
    pub fn undelimited<T: Parse>(self) -> syn::Result<impl IntoIterator<Item = T>> {
        (|p: ParseStream| iter::from_fn(|| (!p.is_empty()).then(|| p.parse())).collect::<syn::Result<Vec<T>>>()).parse2(self.args)
    }
}

impl ToTokens for SubAttr {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.name.to_tokens(tokens);
        self.paren.surround(tokens, |t| self.args.to_tokens(t));
    }
}

impl Parse for AttrArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(Token![!]) {
            Ok(Self::Not {
                not: input.parse()?,
                name: input.parse()?,
            })
        } else {
            let name = input.parse()?;
            if input.peek(Token![,]) || input.is_empty() {
                Ok(Self::Flag(name))
            } else if input.peek(token::Paren) {
                let args;
                Ok(Self::Sub(SubAttr {
                    name,
                    paren: parenthesized!(args in input),
                    args: args.parse()?,
                }))
            } else if input.peek(Token![=]) {
                Ok(Self::KeyValue(KeyValue {
                    name,
                    eq: input.parse()?,
                    value: input.parse()?, // This thing consumes beyond the punctuation separated boundaries?
                }))
            } else {
                Err(input.error("expected !<ident>, <ident>=<value> or <ident>(…)"))
            }
        }
    }
}

impl ToTokens for AttrArg {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        match self {
            AttrArg::Flag(flag) => flag.to_tokens(tokens),
            AttrArg::KeyValue(kv) => kv.to_tokens(tokens),
            AttrArg::Sub(sub) => sub.to_tokens(tokens),
            AttrArg::Not { not, name } => {
                not.to_tokens(tokens);
                name.to_tokens(tokens);
            }
        }
    }
}

pub trait ApplyMeta {
    fn apply_meta(&mut self, expr: AttrArg) -> Result<(), Error>;

    fn apply_sub_attr(&mut self, attr_arg: AttrArg) -> syn::Result<()> {
        for arg in attr_arg.sub_attr()?.args()? {
            self.apply_meta(arg)?;
        }
        Ok(())
    }

    fn apply_subsections(&mut self, list: &syn::MetaList) -> syn::Result<()> {
        if list.tokens.is_empty() {
            return Err(syn::Error::new_spanned(list, "Expected builder(…)"));
        }

        let parser = syn::punctuated::Punctuated::<_, syn::token::Comma>::parse_terminated;
        let exprs = parser.parse2(list.tokens.clone())?;
        for expr in exprs {
            self.apply_meta(expr)?;
        }

        Ok(())
    }

    fn apply_attr(&mut self, attr: &Attribute) -> syn::Result<()> {
        match &attr.meta {
            syn::Meta::List(list) => self.apply_subsections(list),
            meta => Err(Error::new_spanned(meta, "Expected builder(…)")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Mutator {
    pub fun: ItemFn,
    pub required_fields: HashSet<Ident>,
}

#[derive(Default)]
struct MutatorAttribute {
    requires: HashSet<Ident>,
}

impl ApplyMeta for MutatorAttribute {
    fn apply_meta(&mut self, expr: AttrArg) -> Result<(), Error> {
        if expr.name() != "requires" {
            return Err(Error::new_spanned(expr.name(), "Only `requires` is supported"));
        }

        match expr.key_value()?.parse_value()? {
            Expr::Array(syn::ExprArray { elems, .. }) => self.requires.extend(
                elems
                    .into_iter()
                    .map(|expr| match expr {
                        Expr::Path(path) if path.path.get_ident().is_some() => {
                            Ok(path.path.get_ident().cloned().expect("should be ident"))
                        }
                        expr => Err(Error::new_spanned(expr, "Expected field name")),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            expr => {
                return Err(Error::new_spanned(
                    expr,
                    "Only list of field names [field1, field2, …] supported",
                ))
            }
        }
        Ok(())
    }
}

impl Parse for Mutator {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut fun: ItemFn = input.parse()?;

        let mut attribute = MutatorAttribute::default();

        let mut i = 0;
        while i < fun.attrs.len() {
            let attr = &fun.attrs[i];
            if attr.path().is_ident("mutator") {
                attribute.apply_attr(attr)?;
                fun.attrs.remove(i);
            } else {
                i += 1;
            }
        }

        // Ensure `&mut self` receiver
        if let Some(FnArg::Receiver(receiver)) = fun.sig.inputs.first_mut() {
            *receiver = parse_quote!(&mut self);
        } else {
            // Error either on first argument or `()`
            return Err(syn::Error::new(
                fun.sig
                    .inputs
                    .first()
                    .map(Spanned::span)
                    .unwrap_or(fun.sig.paren_token.span.span()),
                "mutator needs to take a reference to `self`",
            ));
        };

        Ok(Self {
            fun,
            required_fields: attribute.requires,
        })
    }
}

fn pat_to_ident(i: usize, pat: &Pat) -> Ident {
    if let Pat::Ident(PatIdent { ident, .. }) = pat {
        ident.clone()
    } else {
        format_ident!("__{i}", span = pat.span())
    }
}

impl Mutator {
    /// Signature for Builder::<mutator> function
    pub fn outer_sig(&self, output: Type) -> Signature {
        let mut sig = self.fun.sig.clone();
        sig.output = ReturnType::Type(Default::default(), output.into());

        sig.inputs = sig
            .inputs
            .into_iter()
            .enumerate()
            .map(|(i, input)| match input {
                FnArg::Receiver(_) => parse_quote!(self),
                FnArg::Typed(mut input) => {
                    input.pat = Box::new(
                        PatIdent {
                            attrs: Vec::new(),
                            by_ref: None,
                            mutability: None,
                            ident: pat_to_ident(i, &input.pat),
                            subpat: None,
                        }
                        .into(),
                    );
                    FnArg::Typed(input)
                }
            })
            .collect();
        sig
    }

    /// Arguments to call inner mutator function
    pub fn arguments(&self) -> Punctuated<Ident, Token![,]> {
        self.fun
            .sig
            .inputs
            .iter()
            .enumerate()
            .filter_map(|(i, input)| match &input {
                FnArg::Receiver(_) => None,
                FnArg::Typed(input) => Some(pat_to_ident(i, &input.pat)),
            })
            .collect()
    }
}
