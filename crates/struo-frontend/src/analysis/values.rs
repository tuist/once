use std::collections::BTreeMap;

use anyhow::anyhow;
use serde_json::Value as JsonValue;
use starlark::eval::Evaluator;
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::ListRef;
use starlark::values::{Value, ValueLike};

use super::store::with_store;
use super::transitive_set::{self, TransitiveSet};
use crate::target::AttrValue;

pub(super) fn attr_value_to_starlark<'v>(
    eval: &Evaluator<'v, '_, '_>,
    value: &AttrValue,
) -> Value<'v> {
    let heap = eval.heap();
    match value {
        AttrValue::String(string) => heap.alloc(string.clone()),
        AttrValue::Integer(integer) => heap.alloc(*integer),
        AttrValue::Float(float) => heap.alloc(*float),
        AttrValue::Bool(boolean) => Value::new_bool(*boolean),
        AttrValue::List(items) => {
            let values: Vec<Value<'v>> = items
                .iter()
                .map(|item| attr_value_to_starlark(eval, item))
                .collect();
            heap.alloc(values)
        }
        AttrValue::Map(entries) => {
            let pairs: Vec<(String, Value<'v>)> = entries
                .iter()
                .map(|(key, value)| (key.clone(), attr_value_to_starlark(eval, value)))
                .collect();
            heap.alloc(AllocDict(pairs))
        }
    }
}

pub(super) fn json_to_value<'v>(eval: &Evaluator<'v, '_, '_>, json: &JsonValue) -> Value<'v> {
    match convert_json(eval, json, &mut |_| {
        None::<Result<_, std::convert::Infallible>>
    }) {
        Ok(value) => value,
        Err(never) => match never {},
    }
}

/// Like [`json_to_value`] for a dependency's provider, turning each set
/// reference back into the set the dependency exported.
pub(super) fn provider_json_to_value<'v>(
    eval: &Evaluator<'v, '_, '_>,
    json: &JsonValue,
) -> anyhow::Result<Value<'v>> {
    let heap = eval.heap();
    convert_json(eval, json, &mut |entries| {
        let hex = transitive_set::reference(entries)?;
        Some(
            with_store(|store| {
                let store =
                    store.ok_or_else(|| anyhow!("transitive sets resolve only during analysis"))?;
                store
                    .host_cache
                    .transitive_sets
                    .resolve(&store.workspace_root, hex)
            })
            .map(|node| heap.alloc(TransitiveSet(node))),
        )
    })
}

fn convert_json<'v, E>(
    eval: &Evaluator<'v, '_, '_>,
    json: &JsonValue,
    object: &mut impl FnMut(&serde_json::Map<String, JsonValue>) -> Option<Result<Value<'v>, E>>,
) -> Result<Value<'v>, E> {
    let heap = eval.heap();
    Ok(match json {
        JsonValue::Null => Value::new_none(),
        JsonValue::Bool(boolean) => Value::new_bool(*boolean),
        JsonValue::Number(number) => {
            if let Some(integer) = number.as_i64() {
                heap.alloc(integer)
            } else if let Some(float) = number.as_f64() {
                heap.alloc(float)
            } else {
                Value::new_none()
            }
        }
        JsonValue::String(string) => heap.alloc(string.clone()),
        JsonValue::Array(items) => {
            let values = items
                .iter()
                .map(|item| convert_json(eval, item, object))
                .collect::<Result<Vec<_>, E>>()?;
            heap.alloc(values)
        }
        JsonValue::Object(entries) => {
            if let Some(value) = object(entries) {
                return value;
            }
            let pairs = entries
                .iter()
                .map(|(key, value)| Ok((key.clone(), convert_json(eval, value, object)?)))
                .collect::<Result<Vec<(String, Value<'v>)>, E>>()?;
            heap.alloc(AllocDict(pairs))
        }
    })
}

/// Deepest Starlark nesting `value_to_json` will follow before it stops
/// recursing. A Starlark value can be nested arbitrarily deep with a
/// trivial loop (`x = []; for _ in range(1_000_000): x = [x]`); following
/// that with native recursion overflows the stack and aborts the
/// process. The limit is far above any legitimate provider or attribute
/// shape, so real graphs are unaffected.
const MAX_JSON_DEPTH: usize = 256;

pub(super) fn value_to_json(value: Value<'_>) -> JsonValue {
    value_to_json_at(value, 0)
}

fn value_to_json_at(value: Value<'_>, depth: usize) -> JsonValue {
    if depth >= MAX_JSON_DEPTH {
        return JsonValue::String("<struo: value nested too deeply to serialize>".to_string());
    }
    if value.is_none() {
        return JsonValue::Null;
    }
    if let Some(boolean) = value.unpack_bool() {
        return JsonValue::Bool(boolean);
    }
    if let Some(integer) = value.unpack_i32() {
        return JsonValue::Number(serde_json::Number::from(integer));
    }
    if let Some(string) = value.unpack_str() {
        return JsonValue::String(string.to_string());
    }
    if let Some(set) = value.downcast_ref::<TransitiveSet>() {
        with_store(|store| {
            if let Some(store) = store {
                store
                    .host_cache
                    .transitive_sets
                    .export(&store.workspace_root, &set.0);
            }
        });
        return serde_json::json!({ transitive_set::REFERENCE_KEY: set.0.hex() });
    }
    if let Some(list) = ListRef::from_value(value) {
        return JsonValue::Array(
            list.iter()
                .map(|item| value_to_json_at(item, depth + 1))
                .collect(),
        );
    }
    if let Some(dict) = DictRef::from_value(value) {
        let mut map = serde_json::Map::new();
        for (key, child) in dict.iter() {
            let Some(key_str) = key.unpack_str() else {
                continue;
            };
            map.insert(key_str.to_string(), value_to_json_at(child, depth + 1));
        }
        return JsonValue::Object(map);
    }
    JsonValue::String(value.to_string())
}

pub(super) fn toml_value_to_starlark<'v>(
    eval: &Evaluator<'v, '_, '_>,
    value: toml::Value,
) -> Value<'v> {
    let heap = eval.heap();
    match value {
        toml::Value::String(value) => heap.alloc(value),
        toml::Value::Integer(value) => heap.alloc(value),
        toml::Value::Float(value) => heap.alloc(value),
        toml::Value::Boolean(value) => Value::new_bool(value),
        toml::Value::Array(values) => heap.alloc(
            values
                .into_iter()
                .map(|value| toml_value_to_starlark(eval, value))
                .collect::<Vec<_>>(),
        ),
        toml::Value::Table(values) => heap.alloc(AllocDict(
            values
                .into_iter()
                .map(|(key, value)| (key, toml_value_to_starlark(eval, value))),
        )),
        toml::Value::Datetime(value) => heap.alloc(value.to_string()),
    }
}

pub(super) fn unpack_string_list(value: Value<'_>, field: &str) -> anyhow::Result<Vec<String>> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a list of strings, got `{}`",
            value.get_type()
        )
    })?;
    list.iter()
        .map(|item| {
            item.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
                anyhow!(
                    "expected `{field}` entries to be strings, got `{}`",
                    item.get_type()
                )
            })
        })
        .collect()
}

pub(super) fn unpack_byte_list(value: Value<'_>, field: &str) -> anyhow::Result<Vec<u8>> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a list of integers in 0..=255, got `{}`",
            value.get_type()
        )
    })?;
    list.iter()
        .map(|item| {
            let int = item.unpack_i32().ok_or_else(|| {
                anyhow!(
                    "expected `{field}` entries to be integers, got `{}`",
                    item.get_type()
                )
            })?;
            u8::try_from(int)
                .map_err(|_| anyhow!("expected `{field}` entries to be in 0..=255, got `{int}`"))
        })
        .collect()
}

pub(super) fn unpack_string_dict(
    value: Value<'_>,
    field: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let dict = DictRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a dict<string, string>, got `{}`",
            value.get_type()
        )
    })?;
    let mut out = BTreeMap::new();
    for (key, value) in dict.iter() {
        let key = key
            .unpack_str()
            .ok_or_else(|| {
                anyhow!(
                    "expected `{field}` keys to be strings, got `{}`",
                    key.get_type()
                )
            })?
            .to_owned();
        let value = value
            .unpack_str()
            .ok_or_else(|| {
                anyhow!(
                    "expected `{field}` values to be strings, got `{}`",
                    value.get_type()
                )
            })?
            .to_owned();
        out.insert(key, value);
    }
    Ok(out)
}
