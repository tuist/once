use serde_json::{json, Value};

pub(crate) fn declaration() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": { "type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9_-]*$" },
            "on": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "event": { "type": "string", "const": "vcs.ref_changed" },
                    "refs": {
                        "type": "array", "minItems": 1, "uniqueItems": true,
                        "items": {
                            "type": "string", "minLength": 1,
                            "description": "Exact fully qualified ref, such as refs/heads/main. Patterns are not supported."
                        }
                    }
                },
                "required": ["event", "refs"]
            },
            "invoke": {
                "type": "array", "minItems": 1,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "target": {
                            "type": "string", "minLength": 1,
                            "description": "Exact canonical workspace-relative target id."
                        },
                        "capability": { "type": "string", "enum": ["build", "test", "run"] },
                        "configuration": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "os": { "type": "string", "minLength": 1 },
                                "arch": { "type": "string", "minLength": 1 },
                                "tokens": {
                                    "type": "array", "uniqueItems": true,
                                    "items": { "type": "string", "minLength": 1 }
                                }
                            },
                            "required": ["os", "arch", "tokens"]
                        }
                    },
                    "required": ["target", "capability", "configuration"]
                }
            }
        },
        "required": ["id", "on", "invoke"]
    })
}

pub(crate) fn observation() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "event": { "type": "string", "const": "vcs.ref_changed" },
            "ref": { "type": "string", "minLength": 1, "description": "Exact fully qualified ref." },
            "revision": { "type": "string", "pattern": "^([0-9a-f]{40}|[0-9a-f]{64})$" }
        },
        "required": ["event", "ref", "revision"]
    })
}
