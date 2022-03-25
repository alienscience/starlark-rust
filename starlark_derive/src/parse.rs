/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use gazebo::prelude::*;
use proc_macro2::Span;
use syn::{
    spanned::Spanned, Attribute, FnArg, GenericArgument, Item, ItemConst, ItemFn, Meta,
    MetaNameValue, NestedMeta, Pat, PatType, PathArguments, ReturnType, Stmt, Type, TypeReference,
};

use crate::{typ::*, util::*};

#[derive(Debug, Copy, Clone, Dupe, PartialEq, Eq)]
pub(crate) enum ModuleKind {
    Globals,
    Methods,
}

impl ModuleKind {
    pub(crate) fn statics_type_name(self) -> &'static str {
        match self {
            ModuleKind::Globals => "GlobalsStatic",
            ModuleKind::Methods => "MethodsStatic",
        }
    }
}

pub(crate) fn parse(mut input: ItemFn) -> syn::Result<StarModule> {
    let module_docstring = parse_module_docstring(&input);
    let visibility = input.vis;
    let sig_span = input.sig.span();
    let name = input.sig.ident;

    if input.sig.inputs.len() != 1 {
        return Err(syn::Error::new(
            sig_span,
            "function must have exactly one argument",
        ));
    }
    let arg = input.sig.inputs.pop().unwrap();
    let arg_span = arg.span();

    let (ty, module_kind) = match arg.into_value() {
        FnArg::Typed(PatType { ty, .. }) if is_mut_globals_builder(&ty) => {
            (ty, ModuleKind::Globals)
        }
        FnArg::Typed(PatType { ty, .. }) if is_mut_methods_builder(&ty) => {
            (ty, ModuleKind::Methods)
        }
        _ => {
            return Err(syn::Error::new(
                arg_span,
                "Expected a mutable globals or methods builder",
            ));
        }
    };
    Ok(StarModule {
        module_kind,
        visibility,
        globals_builder: *ty,
        name,
        docstring: module_docstring,
        stmts: input.block.stmts.into_try_map(parse_stmt)?,
    })
}

fn parse_module_docstring(input: &ItemFn) -> Option<String> {
    let mut doc_attrs = Vec::new();
    for attr in &input.attrs {
        if let Some(ds) = is_attribute_docstring(attr) {
            doc_attrs.push(ds);
        }
    }
    if doc_attrs.is_empty() {
        None
    } else {
        Some(doc_attrs.join("\n"))
    }
}

fn parse_stmt(stmt: Stmt) -> syn::Result<StarStmt> {
    match stmt {
        Stmt::Item(Item::Fn(x)) => parse_fun(x),
        Stmt::Item(Item::Const(x)) => Ok(StarStmt::Const(parse_const(x))),
        s => Err(syn::Error::new(
            s.span(),
            "Can only put constants and functions inside a #[starlark_module]",
        )),
    }
}

fn parse_const(x: ItemConst) -> StarConst {
    StarConst {
        name: x.ident,
        ty: *x.ty,
        value: *x.expr,
    }
}

struct ProcessedAttributes {
    is_attribute: bool,
    type_attribute: Option<NestedMeta>,
    speculative_exec_safe: bool,
    docstring: Option<String>,
    /// Rest attributes
    attrs: Vec<Attribute>,
}

fn is_attribute_docstring(x: &Attribute) -> Option<String> {
    if x.path.is_ident("doc") {
        if let Ok(Meta::NameValue(MetaNameValue {
            lit: syn::Lit::Str(s),
            ..
        })) = x.parse_meta()
        {
            return Some(s.value());
        }
    }
    None
}

/// Parse `#[starlark(...)]` attribute.
fn process_attributes(span: Span, xs: Vec<Attribute>) -> syn::Result<ProcessedAttributes> {
    const ERROR: &str = "Couldn't parse attribute. \
        Expected `#[starlark(type(\"ty\")]`, \
        `#[starlark(attribute)]` or `#[starlark(speculative_exec_safe)]`";

    let mut attrs = Vec::with_capacity(xs.len());
    let mut is_attribute = false;
    let mut type_attribute = None;
    let mut speculative_exec_safe = false;
    let mut doc_attrs = Vec::new();
    for x in xs {
        if x.path.is_ident("starlark") {
            match x.parse_meta()? {
                Meta::List(list) => {
                    assert!(list.path.is_ident("starlark"));
                    for nested in list.nested {
                        match nested {
                            NestedMeta::Lit(lit) => {
                                return Err(syn::Error::new(lit.span(), ERROR));
                            }
                            NestedMeta::Meta(meta) => {
                                if meta.path().is_ident("type") {
                                    match meta {
                                        Meta::List(list) => {
                                            if list.nested.len() != 1 {
                                                return Err(syn::Error::new(list.span(), ERROR));
                                            }
                                            type_attribute =
                                                Some(list.nested.first().unwrap().clone());
                                        }
                                        _ => return Err(syn::Error::new(meta.span(), ERROR)),
                                    }
                                } else if meta.path().is_ident("attribute") {
                                    is_attribute = true;
                                } else if meta.path().is_ident("speculative_exec_safe") {
                                    speculative_exec_safe = true;
                                } else {
                                    return Err(syn::Error::new(meta.span(), ERROR));
                                }
                            }
                        }
                    }
                }
                _ => return Err(syn::Error::new(x.span(), ERROR)),
            }
        } else if let Some(ds) = is_attribute_docstring(&x) {
            doc_attrs.push(ds);
            // Important the attributes remain tagged to the function, so the test annotations
            // are present, and thus the doc test works properly.
            attrs.push(x);
        } else {
            attrs.push(x);
        }
    }
    if is_attribute && type_attribute.is_some() {
        return Err(syn::Error::new(span, "Can't be an attribute with a .type"));
    }
    let docstring = if !doc_attrs.is_empty() {
        Some(doc_attrs.join("\n"))
    } else {
        None
    };
    Ok(ProcessedAttributes {
        is_attribute,
        type_attribute,
        speculative_exec_safe,
        docstring,
        attrs,
    })
}

/// Check if given type is `anyhow::Result<T>`, and if it is, return `T`.
fn is_anyhow_result(t: &Type) -> Option<Type> {
    let path = match t {
        Type::Path(p) => p,
        _ => return None,
    };
    if path.qself.is_some() {
        return None;
    }
    let mut segments = path.path.segments.iter();
    match segments.next() {
        None => return None,
        Some(s) if s.ident != "anyhow" => return None,
        _ => {}
    };
    let result = match segments.next() {
        None => return None,
        Some(s) if s.ident != "Result" => return None,
        Some(result) => result,
    };
    if segments.next().is_some() {
        return None;
    }
    let result_arguments = match &result.arguments {
        PathArguments::AngleBracketed(args) => args,
        _ => return None,
    };
    let mut result_arguments = result_arguments.args.iter();
    let t = match result_arguments.next() {
        None => return None,
        Some(t) => t,
    };
    match t {
        GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    }
}

// Add a function to the `GlobalsModule` named `globals_builder`.
fn parse_fun(func: ItemFn) -> syn::Result<StarStmt> {
    let span = func.span();
    let sig_span = func.sig.span();

    let ProcessedAttributes {
        is_attribute,
        type_attribute,
        speculative_exec_safe,
        docstring,
        attrs,
    } = process_attributes(func.span(), func.attrs)?;

    let (return_type, return_type_arg) = match func.sig.output {
        ReturnType::Default => {
            return Err(syn::Error::new(span, "Function must have a return type"));
        }
        ReturnType::Type(_, x) => match is_anyhow_result(&x) {
            Some(return_arg_type) => (x, return_arg_type),
            None => {
                return Err(syn::Error::new(
                    span,
                    "Function return type must be precisely `anyhow::Result<...>`",
                ));
            }
        },
    };
    let mut args: Vec<_> = func
        .sig
        .inputs
        .into_iter()
        .map(parse_arg)
        .collect::<Result<_, _>>()?;

    if is_attribute {
        if args.len() != 1 {
            return Err(syn::Error::new(
                sig_span,
                "Attribute function must have single parameter",
            ));
        }
        let arg = args.pop().unwrap();
        if !arg.is_this() {
            return Err(syn::Error::new(
                sig_span,
                "Attribute function must have `this` as the only parameter",
            ));
        }
        if arg.default.is_some() {
            return Err(syn::Error::new(
                sig_span,
                "Attribute function `this` parameter have no default value",
            ));
        }
        Ok(StarStmt::Attr(StarAttr {
            name: func.sig.ident,
            arg: arg.ty,
            attrs,
            return_type: *return_type,
            return_type_arg,
            speculative_exec_safe,
            body: *func.block,
            docstring,
        }))
    } else {
        Ok(StarStmt::Fun(StarFun {
            name: func.sig.ident,
            type_attribute,
            attrs,
            args,
            return_type: *return_type,
            return_type_arg,
            speculative_exec_safe,
            body: *func.block,
            source: StarFunSource::Unknown,
            docstring,
        }))
    }
}

fn parse_arg(x: FnArg) -> syn::Result<StarArg> {
    let span = x.span();
    if let FnArg::Typed(PatType { attrs, pat, ty, .. }) = x {
        if let Pat::Ident(ident) = *pat {
            let ty = *ty;
            Ok(StarArg {
                span,
                attrs,
                mutable: ident.mutability.is_some(),
                name: ident.ident,
                by_ref: ident.by_ref.is_some(),
                ty,
                default: ident.subpat.map(|x| *x.1),
                source: StarArgSource::Unknown,
            })
        } else {
            panic!("Unexpected pattern, {:?} in span {:?}", *pat, span);
        }
    } else {
        panic!("Unexpected argument, {:?}", x);
    }
}

fn is_mut_something(x: &Type, smth: &str) -> bool {
    match x {
        Type::Reference(TypeReference {
            mutability: Some(_),
            elem: x,
            ..
        }) => is_type_name(x, smth),
        _ => false,
    }
}

// Is the type `&mut GlobalsBuilder`
fn is_mut_globals_builder(x: &Type) -> bool {
    is_mut_something(x, "GlobalsBuilder")
}

// Is the type `&mut MethodsBuilder`
fn is_mut_methods_builder(x: &Type) -> bool {
    is_mut_something(x, "MethodsBuilder")
}
