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

//! The list type, an immutable sequence of values.

use std::{
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt,
    fmt::{Debug, Display, Formatter},
    hash::Hasher,
    slice,
};

use gazebo::{
    any::AnyLifetime,
    coerce::{coerce, coerce_ref, Coerce},
};

use crate::values::{
    comparison::{compare_slice, equals_slice},
    index::{apply_slice, convert_index},
    ARef, AllocValue, FromValue, FrozenValue, Heap, StarlarkValue, Trace, Tracer, UnpackValue,
    Value, ValueError, ValueLike,
};

/// Define the tuple type. See [`Tuple`] and [`FrozenTuple`] as the two aliases.
#[repr(C)]
pub struct TupleGen<V> {
    len: usize,
    /// The data stored by the tuple.
    content: [V; 0],
}

impl<'v, V: ValueLike<'v>> Display for TupleGen<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(")?;
        for (i, v) in self.content().iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            Display::fmt(&v, f)?;
        }

        if self.len() == 1 {
            write!(f, ",")?;
        }
        write!(f, ")")
    }
}

impl<'v, V: ValueLike<'v>> Debug for TupleGen<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TupleGen")
            .field("content", &self.content())
            .finish()
    }
}

impl<V> TupleGen<V> {
    pub const TYPE: &'static str = "tuple";

    pub(crate) const unsafe fn new(len: usize) -> TupleGen<V> {
        TupleGen { len, content: [] }
    }
}

pub type Tuple<'v> = TupleGen<Value<'v>>;
pub type FrozenTuple = TupleGen<FrozenValue>;

unsafe impl<'v> Coerce<Tuple<'v>> for FrozenTuple {}

unsafe impl<'v> AnyLifetime<'v> for TupleGen<Value<'v>> {
    any_lifetime_body!(TupleGen<Value<'static>>);
}

unsafe impl<'v> AnyLifetime<'v> for TupleGen<FrozenValue> {
    any_lifetime_body!(TupleGen<FrozenValue>);
}

unsafe impl<'v> Trace<'v> for Tuple<'v> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        for v in self.content_mut() {
            tracer.trace(v);
        }
    }
}

impl<'v> Tuple<'v> {
    pub fn from_value(value: Value<'v>) -> Option<&'v Self> {
        if value.unpack_frozen().is_some() {
            value.downcast_ref::<FrozenTuple>().map(coerce_ref)
        } else {
            value.downcast_ref::<Tuple<'v>>()
        }
    }
}

impl<'v> FromValue<'v> for TupleGen<Value<'v>> {
    fn from_value(value: Value<'v>) -> Option<ARef<'v, Self>> {
        Tuple::from_value(value).map(ARef::new_ptr)
    }
}

impl<'v, V: ValueLike<'v>> TupleGen<V> {
    /// Get the length of the tuple.
    pub fn len(&self) -> usize {
        self.content().len()
    }

    /// Tuple elements.
    pub fn content(&self) -> &[V] {
        unsafe { slice::from_raw_parts(self.content.as_ptr(), self.len) }
    }

    pub(crate) fn content_mut(&mut self) -> &mut [V] {
        unsafe { slice::from_raw_parts_mut(self.content.as_mut_ptr(), self.len) }
    }

    /// Iterate over the elements of the tuple.
    pub fn iter<'a>(&'a self) -> impl Iterator<Item = Value<'v>> + 'a
    where
        'v: 'a,
    {
        self.content().iter().map(|e| e.to_value())
    }
}

impl<'v, V: ValueLike<'v>> StarlarkValue<'v> for TupleGen<V>
where
    Self: AnyLifetime<'v>,
{
    starlark_type!(Tuple::TYPE);

    fn to_bool(&self) -> bool {
        self.len() != 0
    }
    fn get_hash(&self) -> anyhow::Result<u64> {
        let mut s = DefaultHasher::new();
        for v in self.content() {
            s.write_u64(v.get_hash()?)
        }
        Ok(s.finish())
    }

    fn to_json(&self) -> anyhow::Result<String> {
        let mut res = String::new();
        res.push('[');
        for (i, e) in self.content().iter().enumerate() {
            if i != 0 {
                res.push_str(", ");
            }
            res.push_str(&e.to_json()?);
        }
        res.push(']');
        Ok(res)
    }

    fn equals(&self, other: Value<'v>) -> anyhow::Result<bool> {
        match Tuple::from_value(other) {
            None => Ok(false),
            Some(other) => equals_slice(self.content(), other.content(), |x, y| x.equals(*y)),
        }
    }

    fn compare(&self, other: Value<'v>) -> anyhow::Result<Ordering> {
        match Tuple::from_value(other) {
            None => ValueError::unsupported_with(self, "cmp()", other),
            Some(other) => compare_slice(self.content(), other.content(), |x, y| x.compare(*y)),
        }
    }

    fn at(&self, index: Value, _heap: &'v Heap) -> anyhow::Result<Value<'v>> {
        let i = convert_index(index, self.len() as i32)? as usize;
        Ok(self.content()[i].to_value())
    }

    fn length(&self) -> anyhow::Result<i32> {
        Ok(self.len() as i32)
    }

    fn is_in(&self, other: Value<'v>) -> anyhow::Result<bool> {
        for x in self.content() {
            if x.equals(other)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn slice(
        &self,
        start: Option<Value>,
        stop: Option<Value>,
        stride: Option<Value>,
        heap: &'v Heap,
    ) -> anyhow::Result<Value<'v>> {
        Ok(heap.alloc_tuple(&apply_slice(coerce(self.content()), start, stop, stride)?))
    }

    fn iterate<'a>(
        &'a self,
        _heap: &'v Heap,
    ) -> anyhow::Result<Box<dyn Iterator<Item = Value<'v>> + 'a>>
    where
        'v: 'a,
    {
        Ok(box self.iter())
    }

    fn with_iterator(
        &self,
        _heap: &'v Heap,
        f: &mut dyn FnMut(&mut dyn Iterator<Item = Value<'v>>) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        f(&mut self.iter())
    }

    fn add(&self, other: Value<'v>, heap: &'v Heap) -> anyhow::Result<Value<'v>> {
        if let Some(other) = Tuple::from_value(other) {
            let mut result = Vec::with_capacity(self.len() + other.len());
            for x in self.iter() {
                result.push(x);
            }
            for x in other.iter() {
                result.push(x);
            }
            Ok(heap.alloc_tuple(&result))
        } else {
            ValueError::unsupported_with(self, "a", other)
        }
    }

    fn mul(&self, other: Value, heap: &'v Heap) -> anyhow::Result<Value<'v>> {
        match other.unpack_int() {
            Some(l) => {
                let mut result = Vec::new();
                for _i in 0..l {
                    result.extend(self.content().iter().map(|e| e.to_value()));
                }
                Ok(heap.alloc_tuple(&result))
            }
            None => Err(ValueError::IncorrectParameterType.into()),
        }
    }
}

impl<'v, T1: AllocValue<'v>> AllocValue<'v> for (T1,) {
    fn alloc_value(self, heap: &'v Heap) -> Value<'v> {
        heap.alloc_tuple(&[self.0.alloc_value(heap)])
    }
}

impl<'v, T1: AllocValue<'v>, T2: AllocValue<'v>> AllocValue<'v> for (T1, T2) {
    fn alloc_value(self, heap: &'v Heap) -> Value<'v> {
        heap.alloc_tuple(&[self.0.alloc_value(heap), self.1.alloc_value(heap)])
    }
}

impl<'v, T1: AllocValue<'v>, T2: AllocValue<'v>, T3: AllocValue<'v>> AllocValue<'v>
    for (T1, T2, T3)
{
    fn alloc_value(self, heap: &'v Heap) -> Value<'v> {
        heap.alloc_tuple(&[
            self.0.alloc_value(heap),
            self.1.alloc_value(heap),
            self.2.alloc_value(heap),
        ])
    }
}

impl<'v, T1: UnpackValue<'v>, T2: UnpackValue<'v>> UnpackValue<'v> for (T1, T2) {
    fn unpack_value(value: Value<'v>) -> Option<Self> {
        let t = Tuple::from_value(value)?;
        if t.len() != 2 {
            return None;
        }
        Some((
            T1::unpack_value(t.content()[0])?,
            T2::unpack_value(t.content()[1])?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_to_str() {
        assert::all_true(
            r#"
str((1, 2, 3)) == "(1, 2, 3)"
str((1, (2, 3))) == "(1, (2, 3))"
str((1,)) == "(1,)"
"#,
        );
    }
}
