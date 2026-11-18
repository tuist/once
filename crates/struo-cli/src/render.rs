use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;

pub fn structured<T: Serialize>(format: Format, value: &T) -> Result<String> {
    let body = match format {
        Format::Human => unreachable!("human rendering is handled by the caller"),
        Format::Json => serde_json::to_string(value)?,
        Format::Toon => toon_rust::encode_default(value)?,
    };
    Ok(format!("{body}\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn toon_preserves_uniform_objects_with_nested_values() {
        let value = json!({
            "commands": [
                {"name": "build", "args": [{"name": "target"}]},
                {"name": "test", "args": [{"name": "target"}]},
            ]
        });

        let rendered = structured(Format::Toon, &value).unwrap();
        let decoded: serde_json::Value = toon_rust::decode_default(rendered.trim()).unwrap();

        assert_eq!(decoded, value);
    }
}
