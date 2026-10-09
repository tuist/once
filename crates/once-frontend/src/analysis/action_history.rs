use once_presentation::ActionHistoryKey;
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::{Heap, Value};

pub(super) fn unpack_history(value: Option<Value<'_>>) -> Option<ActionHistoryKey> {
    let dict = DictRef::from_value(value?)?;
    let namespace = dict.get_str("namespace")?.unpack_str()?;
    let key = dict.get_str("key")?.unpack_str()?;
    if namespace.len() > 64 || key.len() > 128 {
        return None;
    }
    ActionHistoryKey {
        namespace: namespace.to_string(),
        key: key.to_string(),
    }
    .normalize()
}

pub(super) fn make_history<'v>(
    namespace: Value<'v>,
    components: Value<'v>,
    heap: Heap<'v>,
) -> Value<'v> {
    let key = build_history(namespace, components);
    match key {
        Some(key) => heap.alloc(AllocDict([
            ("namespace", heap.alloc(key.namespace)),
            ("key", heap.alloc(key.key)),
        ])),
        None => heap.alloc(NoneType),
    }
}

fn build_history(namespace: Value<'_>, components: Value<'_>) -> Option<ActionHistoryKey> {
    let namespace = namespace.unpack_str()?;
    if namespace.is_empty()
        || namespace.len() > 64
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return None;
    }
    let parts = ListRef::from_value(components)?;
    if !(1..=8).contains(&parts.len()) {
        return None;
    }
    let mut bytes = 0;
    let mut strings = Vec::with_capacity(parts.len());
    for part in parts.iter() {
        let part = part.unpack_str()?;
        bytes += part.len();
        if bytes > 2048 || part.chars().any(char::is_control) {
            return None;
        }
        strings.push(part);
    }
    let encoded = serde_json::to_vec(&strings).ok()?;
    ActionHistoryKey {
        namespace: namespace.to_string(),
        key: super::globals::sha256_hex(&encoded),
    }
    .normalize()
}
