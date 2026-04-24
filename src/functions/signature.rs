#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub type_hint: TypeHint,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeHint {
    Str,
    Int,
    Float,
    Bool,
    List,
    Dict,
    None_,
    Any,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandlerSignature {
    pub params: Vec<ParamSpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("code must define `def handler(...)`")]
    NoHandler,
    #[error("malformed handler signature: {0}")]
    Malformed(String),
}

pub fn parse_handler(_code: &str) -> Result<HandlerSignature, ParseError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_typed_signature() {
        let code = "def handler(name: str, count: int) -> dict:\n    return {}\n";
        let sig = parse_handler(code).unwrap();
        assert_eq!(
            sig.params,
            vec![
                ParamSpec { name: "name".into(), type_hint: TypeHint::Str, required: true },
                ParamSpec { name: "count".into(), type_hint: TypeHint::Int, required: true },
            ],
        );
    }
}
