/// Takes a list of existing `&str` constants and produces a
/// `HashMap<String, String>` mapping each constant's name to its value.
///
/// ```ignore
/// const FOO: &str = "hello";
/// const BAR: &str = "world";
/// let map = string_const_map!(FOO, BAR);
/// // map = {"FOO": "hello", "BAR": "world"}
/// ```
macro_rules! string_const_map {
    ($($name:expr),* $(,)?) => {
        std::collections::HashMap::from([
            $((stringify!($name).to_string(), $name.to_string()),)*
        ])
    };
}

pub(crate) use string_const_map;
