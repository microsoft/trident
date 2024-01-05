use std::collections::HashMap;

use serde_json::Value;
use tera::Error;

pub(super) fn render_characteristic(
    value: &Value,
    _: &HashMap<String, Value>,
) -> Result<Value, Error> {
    Ok(Value::String(if let Value::Object(obj) = value {
        match (
            obj.get("name").ok_or(Error::msg("Expected 'name'"))?,
            obj.get("value").ok_or(Error::msg("Expected 'value'"))?,
            obj.get("is_markdown"),
        ) {
            (Value::String(name), Value::String(value), None)
            | (Value::String(name), Value::String(value), Some(Value::Bool(false))) => {
                format!("| {name} | `{value}` |")
            },
            (Value::String(name), Value::String(value), Some(Value::Bool(true))) => {
                format!("| {name} | {value} |")
            }
            _ => return Err(Error::msg("Expected 'name' and 'value' to be strings and 'is_markdown', if present, to be a bool")),
        }
    } else {
        return Err(Error::msg(
            "Function can only be used on a characteristics object",
        ));
    }))
}

pub(super) fn header_level(value: &Value, _: &HashMap<String, Value>) -> Result<Value, Error> {
    Ok(Value::String(if let Value::Number(num) = value {
        let num = num.as_u64().ok_or(Error::msg("Expected u64"))?;
        let num = num as usize;
        if num > 6 {
            return Err(Error::msg("Expected u64 to be <= 6"));
        }
        let mut header = String::new();
        for _ in 0..num {
            header.push('#');
        }
        header
    } else {
        return Err(Error::msg("Function can only be used on a number"));
    }))
}
