//! For tests of an API description (OpenAPI 3.1): checks real values against the schemas in it, the way a
//! client generated from the description would, and finds the examples in it. Shared by the image server's
//! description and the admin API's.

use serde_json::Value;

/// The problems with `instance` against `schema`, a schema from `description` (which may refer to its
/// components). Empty if there are none.
pub(crate) fn check(description: &Value, schema: &Value, instance: &Value) -> Vec<String> {
    let mut schema = schema.clone();
    schema["components"] = description["components"].clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    validator
        .iter_errors(instance)
        .map(|e| e.to_string())
        .collect()
}

/// Every `$ref` in `description` that points inside it at something that is not there.
pub(crate) fn dangling_references(description: &Value) -> Vec<String> {
    fn walk(root: &Value, here: &Value, found: &mut Vec<String>) {
        match here {
            Value::Object(map) => {
                if let Some(Value::String(target)) = map.get("$ref")
                    && let Some(pointer) = target.strip_prefix('#')
                    && root.pointer(pointer).is_none()
                {
                    found.push(target.clone());
                }
                map.values().for_each(|value| walk(root, value, found));
            }
            Value::Array(items) => items.iter().for_each(|value| walk(root, value, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(description, description, &mut found);
    found
}

/// The problems with `instance` against the schema called `name` in `description`.
pub(crate) fn problems(description: &Value, name: &str, instance: &Value) -> Vec<String> {
    check(
        description,
        &serde_json::json!({ "$ref": format!("#/components/schemas/{name}") }),
        instance,
    )
}

/// Every example in `description`, with the schema it should satisfy and a note of where it is: those on
/// parameters and response bodies, and the `examples` arrays on schemas and their properties.
pub(crate) fn examples(description: &Value) -> Vec<(String, Value, Value)> {
    fn in_schemas(here: &Value, place: &str, found: &mut Vec<(String, Value, Value)>) {
        match here {
            Value::Object(map) => {
                if let Some(Value::Array(list)) = map.get("examples") {
                    for example in list {
                        found.push((format!("{place} schema"), here.clone(), example.clone()));
                    }
                }
                for (key, value) in map {
                    in_schemas(value, &format!("{place}/{key}"), found);
                }
            }
            Value::Array(items) => items
                .iter()
                .enumerate()
                .for_each(|(i, value)| in_schemas(value, &format!("{place}/{i}"), found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    in_schemas(&description["components"], "components", &mut found);
    for (path, item) in description["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            for parameter in operation["parameters"].as_array().into_iter().flatten() {
                if let Some(example) = parameter.get("example") {
                    found.push((
                        format!("{method} {path} parameter {}", parameter["name"]),
                        parameter["schema"].clone(),
                        example.clone(),
                    ));
                }
            }
            for (status, response) in operation["responses"].as_object().unwrap() {
                for (kind, media) in response["content"].as_object().into_iter().flatten() {
                    let Some(schema) = media.get("schema") else {
                        continue;
                    };
                    let named = media.get("example").into_iter().chain(
                        media["examples"]
                            .as_object()
                            .into_iter()
                            .flat_map(|all| all.values().map(|e| &e["value"])),
                    );
                    for example in named {
                        found.push((
                            format!("{method} {path} {status} {kind}"),
                            schema.clone(),
                            example.clone(),
                        ));
                    }
                }
            }
        }
    }
    found
}
