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

pub fn parse_handler(code: &str) -> Result<HandlerSignature, ParseError> {
    let start = find_handler_def(code).ok_or(ParseError::NoHandler)?;
    let (inside, _rest) = split_parens(&code[start..])
        .ok_or_else(|| ParseError::Malformed("unclosed `(`".into()))?;
    let params = split_top_level_commas(inside)
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .map(parse_param)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HandlerSignature { params })
}

fn find_handler_def(code: &str) -> Option<usize> {
    // Locate `def handler(` at any indentation / anywhere in the file.
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("def handler(") || trimmed.starts_with("async def handler(") {
            let line_start = code.lines().take(i).map(|l| l.len() + 1).sum::<usize>();
            let offset_in_line = line.find("handler(")?;
            return Some(line_start + offset_in_line + "handler".len());
        }
    }
    None
}

fn split_parens(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[1..i], &s[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn parse_param(raw: &str) -> Result<ParamSpec, ParseError> {
    let t = raw.trim();
    if t.starts_with('*') {
        return Err(ParseError::Malformed(
            "*args / **kwargs not supported in v1".into(),
        ));
    }
    // `name[: type][= default]`
    let (head, default) = match t.split_once('=') {
        Some((h, d)) => (h.trim(), Some(d.trim())),
        None => (t, None),
    };
    let (name, type_hint) = match head.split_once(':') {
        Some((n, ty)) => (n.trim().to_string(), parse_type(ty.trim())),
        None => (head.to_string(), TypeHint::Any),
    };
    if name.is_empty() {
        return Err(ParseError::Malformed("empty parameter name".into()));
    }
    Ok(ParamSpec {
        name,
        type_hint,
        required: default.is_none(),
    })
}

fn parse_type(s: &str) -> TypeHint {
    match s {
        "str" => TypeHint::Str,
        "int" => TypeHint::Int,
        "float" => TypeHint::Float,
        "bool" => TypeHint::Bool,
        "list" => TypeHint::List,
        "dict" => TypeHint::Dict,
        "None" => TypeHint::None_,
        _ => TypeHint::Any,
    }
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

    #[test]
    fn no_handler_means_error() {
        let code = "def other(): pass\n";
        assert!(matches!(parse_handler(code), Err(ParseError::NoHandler)));
    }

    #[test]
    fn default_makes_param_optional() {
        let code = "def handler(x: int = 5): pass\n";
        let sig = parse_handler(code).unwrap();
        assert!(!sig.params[0].required);
    }

    #[test]
    fn unknown_type_falls_back_to_any() {
        let code = "def handler(x: SomeModel): pass\n";
        let sig = parse_handler(code).unwrap();
        assert_eq!(sig.params[0].type_hint, TypeHint::Any);
    }

    #[test]
    fn missing_annotation_is_any() {
        let code = "def handler(x): pass\n";
        let sig = parse_handler(code).unwrap();
        assert_eq!(sig.params[0].type_hint, TypeHint::Any);
    }

    #[test]
    fn rejects_varargs() {
        let code = "def handler(*args): pass\n";
        assert!(matches!(parse_handler(code), Err(ParseError::Malformed(_))));
    }

    #[test]
    fn zero_params_ok() {
        let code = "def handler():\n    return 1\n";
        let sig = parse_handler(code).unwrap();
        assert!(sig.params.is_empty());
    }
}
