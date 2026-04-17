#[macro_export]
macro_rules! def_unwrap_list {
    ($fnname:ident, $name:ident, $xmlname:expr) => {
        fn $fnname<'de, D>(deserializer: D) -> Result<Vec<$name>, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            /// Represents <list>...</list>
            #[derive(serde::Deserialize)]
            struct List {
                // default allows empty list
                #[serde(default, rename = $xmlname)]
                element: Vec<$name>,
            }
            Ok(List::deserialize(deserializer)?.element)
        }
    };
}
