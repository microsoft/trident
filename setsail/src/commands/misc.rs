use std::error::Error;

/// Parse a single key-value pair
pub fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

pub fn parse_key_val_list<T, U>(
    s: &str,
) -> Result<Vec<(T, U)>, Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let divider = if s.contains(';') {
        // If we have semi-colons then we use them as dividers
        ";"
    } else {
        // Otherwise we want to determine if commas should be used as dividers
        // or if they are part of the values
        let eq_count = s.matches('=').count();
        let comma_count = s.matches(',').count();
        if (eq_count == comma_count && s.ends_with(',')) || (eq_count == comma_count + 1) {
            // IF there is a comma for every equals sign (and one is at the back!) OR
            // there is exactly one comma less than equal signs
            // then we use commas as dividers
            ","
        } else {
            // Otherwise we assume the commas are part of the values and
            // we use semi-colons as dividers
            ";"
        }
    };

    s.split(divider)
        .filter(|v| !v.trim().is_empty())
        .map(|v| parse_key_val(v))
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct KeyValList(pub Vec<(String, String)>);

impl KeyValList {
    pub fn parse(s: &str) -> Result<Self, Box<dyn Error + Send + Sync + 'static>> {
        Ok(Self(parse_key_val_list(s)?))
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.0
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
                .collect(),
        )
    }

    /// Maps the key-value to an object using the provided function
    /// f() is called for each key-value pair
    /// f() receives the key, value and a mutable reference to the object
    /// f() is expected to perform the mapping and return Ok(())
    /// If f() returns an error, the error is added to the list of errors
    pub fn map<T: Default>(
        &self,
        f: fn(&str, &str, &mut T) -> Result<(), String>,
    ) -> Result<T, Vec<String>> {
        let mut obj = T::default();
        let mut errors = Vec::new();

        for (k, v) in &self.0 {
            if let Err(e) = f(k, v, &mut obj) {
                errors.push(format!("{}: {} = {}", e, k, v));
            }
        }

        if errors.is_empty() {
            Ok(obj)
        } else {
            Err(errors)
        }
    }
}

impl std::str::FromStr for KeyValList {
    type Err = Box<dyn Error + Send + Sync + 'static>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
