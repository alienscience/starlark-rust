/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! AST for parsed starlark files.

use crate::{
    errors::Diagnostic,
    syntax::ast::{
        Argument, AstArgument, AstExpr, AstParameter, AstStmt, AstString, Expr, Parameter, Stmt,
    },
};
use codemap::{CodeMap, Spanned};
use gazebo::prelude::*;
use std::{collections::HashSet, sync::Arc};
use thiserror::Error;

#[derive(Error, Debug)]
enum ValidateError {
    #[error("`break` cannot be used outside of a `for` loop")]
    BreakOutsideLoop,
    #[error("`continue` cannot be used outside of a `for` loop")]
    ContinueOutsideLoop,
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
enum ArgsStage {
    Positional,
    Named,
    Args,
    Kwargs,
}

#[derive(Error, Debug)]
enum ArgumentDefinitionOrderError {
    #[error("positional argument after non positional")]
    PositionalThenNonPositional,
    #[error("named argument after *args or **kwargs")]
    NamedArgumentAfterStars,
    #[error("repeated named argument")]
    RepeatedNamed,
    #[error("Args array after another args or kwargs")]
    ArgsArrayAfterArgsOrKwargs,
    #[error("Multiple kwargs dictionary in arguments")]
    MultipleKwargs,
}

impl Expr {
    /// We want to check a function call is well-formed.
    /// Our eventual plan is to follow the Python invariants, but for now, we are closer
    /// to the Starlark invariants.
    ///
    /// Python invariants are no positional arguments after named arguments,
    /// no *args after **kwargs, no repeated argument names.
    ///
    /// Starlark invariants are the above, plus at most one *args and the *args must appear
    /// after all positional and named arguments. The spec is silent on whether you are allowed
    /// multiple **kwargs.
    ///
    /// We allow at most one **kwargs.
    pub fn check_call(
        f: AstExpr,
        args: Vec<AstArgument>,
        codemap: &Arc<CodeMap>,
    ) -> anyhow::Result<Expr> {
        let err = |span, msg| Err(Diagnostic::add_span(msg, span, codemap.dupe()));

        let mut stage = ArgsStage::Positional;
        let mut named_args = HashSet::new();
        for arg in &args {
            match &arg.node {
                Argument::Positional(_) => {
                    if stage != ArgsStage::Positional {
                        return err(
                            arg.span,
                            ArgumentDefinitionOrderError::PositionalThenNonPositional,
                        );
                    }
                }
                Argument::Named(n, _) => {
                    if stage > ArgsStage::Named {
                        return err(
                            arg.span,
                            ArgumentDefinitionOrderError::NamedArgumentAfterStars,
                        );
                    } else if !named_args.insert(&n.node) {
                        // Check the names are distinct
                        return err(n.span, ArgumentDefinitionOrderError::RepeatedNamed);
                    } else {
                        stage = ArgsStage::Named;
                    }
                }
                Argument::ArgsArray(_) => {
                    if stage > ArgsStage::Named {
                        return err(
                            arg.span,
                            ArgumentDefinitionOrderError::ArgsArrayAfterArgsOrKwargs,
                        );
                    } else {
                        stage = ArgsStage::Args;
                    }
                }
                Argument::KWArgsDict(_) => {
                    if stage == ArgsStage::Kwargs {
                        return err(arg.span, ArgumentDefinitionOrderError::MultipleKwargs);
                    } else {
                        stage = ArgsStage::Kwargs;
                    }
                }
            }
        }
        Ok(Expr::Call(box f, args))
    }
}

fn test_param_name<'a, T>(
    argset: &mut HashSet<&'a str>,
    n: &'a Spanned<String>,
    arg: &Spanned<T>,
    codemap: &Arc<CodeMap>,
) -> anyhow::Result<()> {
    if argset.contains(n.node.as_str()) {
        return Err(Diagnostic::add_span(
            ArgumentUseOrderError::DuplicateParameterName,
            arg.span,
            codemap.dupe(),
        ));
    }
    argset.insert(&n.node);
    Ok(())
}

#[derive(Error, Debug)]
enum ArgumentUseOrderError {
    #[error("duplicated parameter name")]
    DuplicateParameterName,
    #[error("positional parameter after non positional")]
    PositionalThenNonPositional,
    #[error("Default parameter after args array or kwargs dictionary")]
    DefaultParameterAfterStars,
    #[error("Args parameter after another args or kwargs parameter")]
    ArgsParameterAfterStars,
    #[error("Multiple kwargs dictionary in parameters")]
    MultipleKwargs,
}

impl Stmt {
    pub fn check_def(
        name: AstString,
        parameters: Vec<AstParameter>,
        return_type: Option<Box<AstExpr>>,
        stmts: AstStmt,
        codemap: &Arc<CodeMap>,
    ) -> anyhow::Result<Stmt> {
        let err = |span, msg| Err(Diagnostic::add_span(msg, span, codemap.dupe()));

        // you can't repeat argument names
        let mut argset = HashSet::new();
        // You can't have more than one *args/*, **kwargs
        // **kwargs must be last
        // You can't have a required `x` after an optional `y=1`
        let mut seen_args = false;
        let mut seen_kwargs = false;
        let mut seen_optional = false;

        for arg in parameters.iter() {
            match &arg.node {
                Parameter::Normal(n, ..) => {
                    if seen_kwargs || seen_optional {
                        return err(arg.span, ArgumentUseOrderError::PositionalThenNonPositional);
                    }
                    test_param_name(&mut argset, n, arg, codemap)?;
                }
                Parameter::WithDefaultValue(n, ..) => {
                    if seen_kwargs {
                        return err(arg.span, ArgumentUseOrderError::DefaultParameterAfterStars);
                    }
                    seen_optional = true;
                    test_param_name(&mut argset, n, arg, codemap)?;
                }
                Parameter::NoArgs => {
                    if seen_args || seen_kwargs {
                        return err(arg.span, ArgumentUseOrderError::ArgsParameterAfterStars);
                    }
                    seen_args = true;
                }
                Parameter::Args(n, ..) => {
                    if seen_args || seen_kwargs {
                        return err(arg.span, ArgumentUseOrderError::ArgsParameterAfterStars);
                    }
                    seen_args = true;
                    test_param_name(&mut argset, n, arg, codemap)?;
                }
                Parameter::KWArgs(n, ..) => {
                    if seen_kwargs {
                        return err(arg.span, ArgumentUseOrderError::MultipleKwargs);
                    }
                    seen_kwargs = true;
                    test_param_name(&mut argset, n, arg, codemap)?;
                }
            }
        }
        Ok(Stmt::Def(name, parameters, return_type, box stmts))
    }

    /// Validate `break` and `continue` is only used inside loops
    pub fn validate_break_continue(codemap: &Arc<CodeMap>, stmt: &AstStmt) -> anyhow::Result<()> {
        // Inside a for, the only thing that might disallow break/continue is def
        fn inside_for(codemap: &Arc<CodeMap>, stmt: &AstStmt) -> anyhow::Result<()> {
            match &stmt.node {
                Stmt::Def(_, _, _, body) => outside_for(codemap, body),
                _ => stmt.node.visit_stmt_result(|x| inside_for(codemap, x)),
            }
        }

        // Outside a for, a continue/break is an error
        fn outside_for(codemap: &Arc<CodeMap>, stmt: &AstStmt) -> anyhow::Result<()> {
            match &stmt.node {
                Stmt::For(box (_, _, body)) => inside_for(codemap, body),
                Stmt::Break => Err(Diagnostic::add_span(
                    ValidateError::BreakOutsideLoop,
                    stmt.span,
                    codemap.dupe(),
                )),
                Stmt::Continue => Err(Diagnostic::add_span(
                    ValidateError::ContinueOutsideLoop,
                    stmt.span,
                    codemap.dupe(),
                )),
                _ => stmt.node.visit_stmt_result(|x| outside_for(codemap, x)),
            }
        }

        outside_for(codemap, stmt)
    }
}
