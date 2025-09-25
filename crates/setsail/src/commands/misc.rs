use std::error::Error;

/// Parse a single key-value pair
fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
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

/// Divide a string into key=value pairs using a divider
/// Parameters:
/// - s: the string to parse
/// - main_div: the divider to use
/// - alt_divider: an alternative divider to use if the string contains it
///
/// When alt_divider is None, the function will use main_div as the divider
/// to perform a simple split.
///
/// When alt_divider is Some, the function will attempt to determine if main_div
/// should be used as a divider or if it is part of the values.
/// The selection is **not** meant to understand user intent, but just to be able to support both separators.
/// Users should craft strings that are unambiguous by employing the alt_divider when needed.
///
/// Selection is done though a best effort approach using the following heuristic:
/// IF: the string contains alt_divider, THEN:
///     use alt_divider as the divider
/// ELSE
///     IF: (
///             the string contains the same number of main_div as equals signs
///             AND
///             the string ends with main_div
///         )
///         OR
///        (
///             the string contains one less main_div than equals signs
///             AND
///             the string does NOT end with main_div  
///         ), THEN:
///         use main_div as the divider
///    ELSE:
///         use alt_divider as the divider
///
/// Assuming main_div is ',' and alt_divider is ';', the following strings would be parsed as:
/// - "a=1,b=2,c=3" or "a=1;b=2;c=3"
///   - "a" = "1"
///   - "b" = "2"
///   - "c" = "3"
/// - "a=1,b=2;c=3"
///   - "a" = "1,b2"
///   - "c" = "3"
/// - "a=1;b=2,c=3"
///   - "a" = "1
///   - "b" = "2,c=3"
/// - "a=1,b=2,c=3;"
///  - "a" = "1,b=2,c=3"
/// - "a=1,2" or "a=1,2," or "a=1;2" or "a=1;2;"
///  - "a" = "1,2"
fn parse_key_val_list<T, U>(
    s: &str,
    mut main_div: char,
    alt_divider: Option<char>,
) -> Result<Vec<(T, U)>, Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    // If we have an alt divider attempt to use it
    if let Some(alt) = alt_divider {
        if s.contains(alt) {
            // If we have <alt> then we use them as dividers
            main_div = alt
        } else {
            // Otherwise we want to determine if <main> should be used as dividers
            // or if they are part of the values
            let eq_count = s.matches('=').count();
            let divider_count = s.matches(main_div).count();
            if !((eq_count == divider_count && s.ends_with(main_div))
                || ((eq_count == divider_count + 1) && !s.ends_with(main_div)))
            {
                // We want to preserve <main> IF:
                //  a) there is a <main> for every equals sign AND one is at the back!
                //    OR
                //  b) there is exactly one <main> less than equal signs AND there is NO <main> at the back
                //
                // We evaluate these conditions and negate them, so If we got here
                // we need to set <main> to <alt>
                main_div = alt;
            }
        };
    }

    s.split(main_div)
        .filter(|v| !v.trim().is_empty())
        .map(|v| parse_key_val(v))
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct KeyValList(pub Vec<(String, String)>);

impl KeyValList {
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
                errors.push(format!("{e}: {k} = {v}"));
            }
        }

        if errors.is_empty() {
            Ok(obj)
        } else {
            Err(errors)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn semicolon_alt_parser(s: &str) -> Result<Self, Box<dyn Error + Send + Sync + 'static>> {
        Ok(Self(parse_key_val_list(s, ',', Some(';'))?))
    }
}

impl std::str::FromStr for KeyValList {
    type Err = Box<dyn Error + Send + Sync + 'static>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse_key_val_list(s, ',', None)?))
    }
}

// Tests
#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_parse_empty() {
        let list = KeyValList::from_str("").expect("Failed to parse empty string");
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_parse_commas() {
        let list = KeyValList::from_str(",,,").expect("Failed to parse empty string");
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_parse_single() {
        let list = KeyValList::from_str("don't=panic").expect("Failed to parse single KV");
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_parse_multiple() {
        let list = KeyValList::from_str("mode=802.3ad,lacp_rate=1,miimon=100")
            .expect("Failed to parse KV string");
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_bad_equals() {
        KeyValList::from_str("mode=802.3ad,lacp_rate1,miimon=100").unwrap_err();
        // Note removed an equals sign here         ^
    }

    #[test]
    fn test_multiple_equals() {
        assert_eq!(
            KeyValList::from_str("mode=802.3ad,lacp_rate=stuff=something,miimon=100")
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn test_alt_parser() {
        let p = KeyValList::semicolon_alt_parser;

        // Well-defined behavior:
        assert_eq!(p("mode=802.3ad,lacp_rate=1,miimon=100").unwrap().len(), 3);
        assert_eq!(p("mode=802.3ad,lacp_rate=1,miimon=100,").unwrap().len(), 3);
        assert_eq!(p("mode=802.3ad;lacp_rate=1;miimon=100").unwrap().len(), 3);

        // Value with commas
        assert_eq!(p("lacp_rate=1,2,3,4;miimon=100").unwrap().len(), 2);
        assert_eq!(p("lacp_rate=1,2,3,4;miimon=100;").unwrap().len(), 2);

        // "Nested" objects examples:
        assert_eq!(p("key1=val1=1,val2=1").unwrap().len(), 1);
        assert_eq!(p("key1=val1=1,val2=1,").unwrap().len(), 1);

        assert_eq!(p("key1=val1=1,val2=1;key2=val1=1,val2=1").unwrap().len(), 2);
        assert_eq!(
            p("key1=val1=1,val2=1,;key2=val1=1,val2=1").unwrap().len(),
            2
        );

        // We don't care about repeated keys, so this is fine:
        assert_eq!(p("key1=1;key1=2").unwrap().len(), 2);

        // We ignore empty divisions, but now it only works with semicolons:
        assert_eq!(p(";;;;").unwrap().len(), 0);
        assert_eq!(p("key1=1;;;;;").unwrap().len(), 1);

        // This is useless but correct:
        assert_eq!(p("=;=;=;=").unwrap().len(), 4);
        assert_eq!(p("=;=;=;=;").unwrap().len(), 4);

        // Ambiguous behavior:

        // Too many commas, will make the parser assume we want a semicolon as a separator
        // and try to parse this as a k=v pair and fail
        p(",,,,").unwrap_err();
        // And here
        p("key1=1;,,,,").unwrap_err();

        // However this one works because we have a key=value pair, all the commas are seen as part of the value
        assert_eq!(p("key1=1,,,,,").unwrap().len(), 1);
        // It follows that this is possible too, albeit useless:
        assert_eq!(p(",,,,key1=1").unwrap().len(), 1);
        // Here with just 1 comma and 1 equals sign, the parser will assume we want a comma as a separator,
        // but the empty value will make it succeed
        assert_eq!(p(",key1=1").unwrap().len(), 1);

        // This one should make the parser assume we want a semicolon as a separator
        // because we hay way more commas than equals signs. It should get parsed as
        // a single key-value pair: "mode" = "802.3ad,lacp_rate=1,2,3,4,miimon=100"
        // Note: the example makes it look wrong, but it's not, this class should be
        // generic so commas in values should be allowed
        assert_eq!(
            p("mode=802.3ad,lacp_rate=1,2,3,4,miimon=100")
                .unwrap()
                .len(),
            1
        );

        // This comma is part of the value
        assert_eq!(p("mode=1,2").unwrap().len(), 1);
        assert_eq!(p("mode=1,2;").unwrap().len(), 1);

        // This value has 2 commas!
        assert_eq!(p("mode=1,2,").unwrap().len(), 1);
    }
}
