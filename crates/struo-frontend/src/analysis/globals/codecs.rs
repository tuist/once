//! Data-format codecs for TOML, property lists, and JSON.

use serde_json::Value as JsonValue;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Value;

use crate::analysis::values::{json_to_value, toml_value_to_starlark};

#[starlark_module]
pub(super) fn codec_globals(builder: &mut GlobalsBuilder) {
    /// Decode TOML into Starlark dictionaries/lists/scalars. This is a
    /// generic data-format primitive used by dependency resolvers; the
    /// ecosystem-specific interpretation stays in Starlark.
    fn toml_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value: toml::Value = toml::from_str(src)?;
        Ok(toml_value_to_starlark(eval, value))
    }

    /// Decode an XML property list into Starlark dictionaries/lists/scalars.
    /// Dates become RFC 3339 strings and data becomes base64 strings.
    fn plist_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value = crate::analysis::property_list::plist_to_json(src)?;
        Ok(json_to_value(eval, &value))
    }

    /// Encode Starlark dictionaries/lists/scalars as an XML property list,
    /// failing on values a property list cannot represent.
    fn plist_encode(value: Value<'_>) -> anyhow::Result<String> {
        crate::analysis::property_list::starlark_to_plist_xml(value)
    }

    /// Decode JSON into Starlark dictionaries/lists/scalars. Dependency
    /// resolvers use this for machine output from ecosystem-native
    /// resolution commands.
    fn json_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value: JsonValue = serde_json::from_str(src)?;
        Ok(json_to_value(eval, &value))
    }
}
