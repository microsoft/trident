use std::collections::HashMap;

use serde_json::{Map, Value};
use tera::Error;

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

const CHARACTERISTIC_NAME_TITLE: &str = "Characteristic";
const CHARACTERISTIC_VALUE_TITLE: &str = "Value";

pub(super) fn render_characteristics_table(
    value: &Value,
    _: &HashMap<String, Value>,
) -> Result<Value, Error> {
    if !value.is_array() {
        return Err(Error::msg("Function can only be used on an array"));
    }

    let mut table_data: Vec<(String, String)> = Vec::new();
    for characteristic in value.as_array().unwrap() {
        if !characteristic.is_object() {
            return Err(Error::msg("Expected array to contain objects"));
        }
        table_data.push(render_characteristic(characteristic.as_object().unwrap())?);
    }

    let name_max_len = table_data
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .ok_or(Error::msg("Expected at least one characteristic"))?
        .max(CHARACTERISTIC_NAME_TITLE.len());

    let value_max_len = table_data
        .iter()
        .map(|(_, value)| value.len())
        .max()
        .ok_or(Error::msg("Expected at least one characteristic"))?
        .max(CHARACTERISTIC_VALUE_TITLE.len());

    let mut table: Vec<String> = vec![
        format!(
            "| {name:<name_max_len$} | {value:<value_max_len$} |",
            name = CHARACTERISTIC_NAME_TITLE,
            value = CHARACTERISTIC_VALUE_TITLE,
            name_max_len = name_max_len,
            value_max_len = value_max_len,
        ),
        format!(
            "| {} | {} |",
            "-".repeat(name_max_len),
            "-".repeat(value_max_len),
        ),
    ];

    for (name, value) in table_data {
        table.push(format!(
            "| {name:<name_max_len$} | {value:<value_max_len$} |",
            name = name,
            value = value,
            name_max_len = name_max_len,
            value_max_len = value_max_len,
        ));
    }

    Ok(Value::String(table.join("\n")))
}

fn render_characteristic(obj: &Map<String, Value>) -> Result<(String, String), Error> {
    Ok(match (
        obj.get("name").ok_or(Error::msg("Expected 'name'"))?,
        obj.get("value").ok_or(Error::msg("Expected 'value'"))?,
        obj.get("is_markdown"),
    ) {
        (Value::String(name), Value::String(value), None)
        | (Value::String(name), Value::String(value), Some(Value::Bool(false))) => {
            (name.clone(), format!("`{}`", value))
        }
        (Value::String(name), Value::String(value), Some(Value::Bool(true))) => {
            (name.clone(), value.clone())
        }
        _ => return Err(Error::msg(
            "Expected 'name' and 'value' to be strings and 'is_markdown', if present, to be a bool",
        )),
    })
}
