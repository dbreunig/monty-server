use crate::functions::signature::HandlerSignature;
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
    _sig: &HandlerSignature,
    _body: &Value,
) -> Result<Map<String, Value>, SchemaError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::signature::{ParamSpec, TypeHint};
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
}
