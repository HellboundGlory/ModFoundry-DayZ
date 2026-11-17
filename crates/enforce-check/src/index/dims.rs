use std::collections::HashMap;

use super::model::Dim;
use super::resolve::{base_name, Kind, Scope};
use super::rules::Line;

// The lines the dimensions of a static array print, for a class member and a
// local alike. `constants` holds the `const int` names a size may read.
pub fn dimension_lines(
    constants: &HashMap<String, Option<i64>>,
    types: &Scope,
    name: &str,
    dims: &[Dim],
    type_name: &str,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut error = |message: String| lines.push(Line { soft: false, message });
    let mut count: Option<i64> = Some(1);
    for dim in dims {
        match dim {
            Dim::Number(value) if *value <= 0 => {
                error("Wrong array size".to_string());
                count = None;
            }
            Dim::Number(value) if *value > 65535 => {
                error(format!("Maximum static array index is 65535 '{name}'"));
                count = None;
            }
            Dim::Number(value) => count = count.and_then(|total| total.checked_mul(*value)),
            Dim::Hex | Dim::Expression => {
                error("Expected numeric constant".to_string());
                count = None;
            }
            Dim::Name(constant) => match constants.get(constant) {
                Some(Some(value)) => count = count.and_then(|total| total.checked_mul(*value)),
                Some(None) => count = None,
                None => {
                    error("Expected numeric constant".to_string());
                    count = None;
                }
            },
        }
    }
    if dims.len() > 1 && type_name == "vector" {
        lines.push(Line {
            soft: true,
            message: "Static 2D array of 'vector' is not supported".to_string(),
        });
    }
    if type_name == "typename" && !dims.is_empty() {
        lines.push(Line {
            soft: true,
            message: "Static array of 'typename' types is not supported".to_string(),
        });
    }
    if let (Some(count), Some(size)) = (count, type_size(types, type_name))
        && size.checked_mul(count).is_some_and(|bytes| bytes > 0x20000)
    {
        lines.push(Line { soft: false, message: format!("Variable '{name}' too large") });
        lines.push(Line { soft: false, message: format!("Can't create variable '{name}'") });
    }
    lines
}

// Sizes: `int`/`float` 4 bytes, `vector` 12 (counted three times), an enum 4.
fn type_size(types: &Scope, type_name: &str) -> Option<i64> {
    Some(match base_name(type_name) {
        "int" | "float" | "bool" => 4,
        "vector" => 12,
        _ => match types.kind_of(type_name) {
            Some(Kind::Enum) => 4,
            _ => return None,
        },
    })
}
