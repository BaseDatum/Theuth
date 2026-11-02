//! Argument re-check immediately before an app invocation (46 §4): the
//! renderer evaluated the args dynamically, the parser helper checked them
//! statically at publish; this catches a drifted schema or a bad value at
//! the last moment, before the gate and before anything runs. Full JSON
//! Schema via `jsonschema`, built without retrieval features: a `$ref` that
//! leaves the document is a schema that does not compile, and a schema that
//! does not compile fails the call closed rather than skipping the check.
//!
//! [`compile`] runs once per registered tool: the registry keeps the result
//! beside the tool map and drops it with the registration, so a polling app
//! pays the compile on its first call only and the cache can never outgrow
//! the tools that exist.

use serde_json::Value;

/// The compiled validator, or why the schema cannot be checked.
pub type Compiled = Result<jsonschema::Validator, String>;

/// Compile once; `Err` is final for that schema (the call fails closed).
pub fn compile(schema: &Value) -> Compiled {
    jsonschema::options()
        .should_validate_formats(false)
        .build(schema)
        .map_err(|e| format!("tool schema does not compile: {e}"))
}

/// `Err(text)` names the failing instance path, or the schema fault.
pub fn check(compiled: &Compiled, value: &Value) -> Result<(), String> {
    let validator = compiled.as_ref().map_err(Clone::clone)?;
    validator
        .validate(value)
        .map_err(|e| format!("{}: {e}", e.instance_path()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{check, compile};

    #[test]
    fn rejects_by_path_and_fails_closed_on_external_ref() {
        let s = compile(&json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string"},
                "mode": {"oneOf": [{"const": "a"}, {"const": "b"}]},
                "again": {"$ref": "#/properties/name"},
            }
        }));
        assert!(check(&s, &json!({"name": "x", "mode": "b", "again": "y"})).is_ok());
        assert!(check(&s, &json!({})).unwrap_err().contains("name"));
        assert!(
            check(&s, &json!({"name": "x", "mode": "c"}))
                .unwrap_err()
                .starts_with("/mode:")
        );
        assert!(
            check(&s, &json!({"name": "x", "again": 1}))
                .unwrap_err()
                .starts_with("/again:")
        );
        let external = compile(&json!({"$ref": "https://example.invalid/schema.json"}));
        assert!(
            check(&external, &json!({}))
                .unwrap_err()
                .contains("does not compile")
        );
    }
}
