use crate::functions::signature::{HandlerSignature, TypeHint};
#[cfg(test)]
use crate::functions::signature::ParamSpec;
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Serialize)]
pub struct SchemaError {
    pub error: &'static str,
    pub expected: Map<String, Value>,
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
    pub wrong_type: Vec<String>,
}

pub fn validate(
    sig: &HandlerSignature,
    body: &Value,
) -> Result<Map<String, Value>, SchemaError> {
    let expected_map: Map<String, Value> = sig
        .params
        .iter()
        .map(|p| (p.name.clone(), Value::String(type_hint_name(&p.type_hint).into())))
        .collect();

    let obj = match body {
        Value::Object(m) => m,
        _ => {
            return Err(SchemaError {
                error: "invalid input",
                expected: expected_map,
                missing: sig.params.iter().filter(|p| p.required).map(|p| p.name.clone()).collect(),
                unexpected: vec![],
                wrong_type: vec![],
            });
        }
    };

    let mut missing = Vec::new();
    let mut wrong_type = Vec::new();
    let mut kwargs: Map<String, Value> = Map::new();

    for p in &sig.params {
        match obj.get(&p.name) {
            Some(v) => {
                if !matches_type(&p.type_hint, v) {
                    wrong_type.push(p.name.clone());
                } else {
                    kwargs.insert(p.name.clone(), v.clone());
                }
            }
            None if p.required => missing.push(p.name.clone()),
            None => {} // optional with default — let Python apply the default
        }
    }

    let param_names: std::collections::HashSet<&str> =
        sig.params.iter().map(|p| p.name.as_str()).collect();
    let unexpected: Vec<String> = obj
        .keys()
        .filter(|k| !param_names.contains(k.as_str()))
        .cloned()
        .collect();

    if !missing.is_empty() || !wrong_type.is_empty() || !unexpected.is_empty() {
        return Err(SchemaError {
            error: "invalid input",
            expected: expected_map,
            missing,
            unexpected,
            wrong_type,
        });
    }

    Ok(kwargs)
}

fn type_hint_name(t: &TypeHint) -> &'static str {
    match t {
        TypeHint::Str => "str",
        TypeHint::Int => "int",
        TypeHint::Float => "float",
        TypeHint::Bool => "bool",
        TypeHint::List => "list",
        TypeHint::Dict => "dict",
        TypeHint::None_ => "None",
        TypeHint::Any => "any",
    }
}

fn matches_type(t: &TypeHint, v: &Value) -> bool {
    match (t, v) {
        (TypeHint::Any, _) => true,
        (TypeHint::Str, Value::String(_)) => true,
        (TypeHint::Int, Value::Number(n)) => n.is_i64() || n.is_u64(),
        (TypeHint::Float, Value::Number(_)) => true,
        (TypeHint::Bool, Value::Bool(_)) => true,
        (TypeHint::List, Value::Array(_)) => true,
        (TypeHint::Dict, Value::Object(_)) => true,
        (TypeHint::None_, Value::Null) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sig(params: Vec<(&str, TypeHint, bool)>) -> HandlerSignature {
        HandlerSignature {
            params: params
                .into_iter()
                .map(|(n, t, r)| ParamSpec { name: n.into(), type_hint: t, required: r })
                .collect(),
        }
    }

    #[test]
    fn accepts_matching_body() {
        let s = sig(vec![("name", TypeHint::Str, true), ("count", TypeHint::Int, true)]);
        let body = json!({ "name": "hi", "count": 3 });
        let kw = validate(&s, &body).unwrap();
        assert_eq!(kw["name"], json!("hi"));
        assert_eq!(kw["count"], json!(3));
    }

    #[test]
    fn flags_missing_required_field() {
        let s = sig(vec![("x", TypeHint::Str, true)]);
        let err = validate(&s, &json!({})).unwrap_err();
        assert_eq!(err.missing, vec!["x"]);
    }

    #[test]
    fn flags_wrong_type() {
        let s = sig(vec![("x", TypeHint::Int, true)]);
        let err = validate(&s, &json!({ "x": "oops" })).unwrap_err();
        assert_eq!(err.wrong_type, vec!["x"]);
    }

    #[test]
    fn flags_unexpected_keys() {
        let s = sig(vec![("x", TypeHint::Int, true)]);
        let err = validate(&s, &json!({ "x": 1, "extra": true })).unwrap_err();
        assert_eq!(err.unexpected, vec!["extra"]);
    }

    #[test]
    fn optional_param_may_be_omitted() {
        let s = sig(vec![("x", TypeHint::Int, false)]);
        let kw = validate(&s, &json!({})).unwrap();
        assert!(kw.is_empty());
    }

    #[test]
    fn any_type_accepts_anything() {
        let s = sig(vec![("x", TypeHint::Any, true)]);
        assert!(validate(&s, &json!({ "x": [1, 2] })).is_ok());
        assert!(validate(&s, &json!({ "x": null })).is_ok());
    }
}
