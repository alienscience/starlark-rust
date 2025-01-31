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

//! Compile def.

use gazebo::prelude::*;

use crate::eval::{
    bc::{
        instr_arg::ArgPopsStack,
        instr_impl::{InstrDef, InstrDefData},
        writer::BcWriter,
    },
    fragment::{def::DefCompiled, span::IrSpanned},
    runtime::call_stack::FrozenFileSpan,
};

impl DefCompiled {
    pub(crate) fn write_bc(&self, span: FrozenFileSpan, bc: &mut BcWriter) {
        let DefCompiled {
            ref function_name,
            ref params,
            ref return_type,
            info,
        } = *self;
        let function_name = function_name.clone();
        let mut value_count = 0;
        let params = params.map(|p| {
            p.map(|p| {
                p.map_expr(|e| {
                    e.write_bc(bc);
                    value_count += 1;
                    value_count - 1
                })
            })
        });
        let return_type = return_type.as_ref().map(|t| {
            t.write_bc(bc);
            value_count += 1;
            IrSpanned {
                node: value_count - 1,
                span: t.span,
            }
        });

        let instr_def_data = InstrDefData {
            function_name,
            params,
            return_type,
            info,
        };

        bc.write_instr::<InstrDef>(span, (ArgPopsStack(value_count), instr_def_data));
    }
}
