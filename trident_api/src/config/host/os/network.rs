#[cfg(feature = "schemars")]
pub(super) mod schema_helpers {
    use schemars::{gen::SchemaGenerator, schema::Schema};
    use serde_json::{json, Map, Value};

    /// Returns a placeholder schema for a netplan field.
    pub fn make_placeholder_netplan_schema(gen: &mut SchemaGenerator) -> Schema {
        let mut schema = gen
            .subschema_for::<Option<Map<String, Value>>>()
            .into_object();
        schema.format = Some("Netplan YAML".to_owned());
        schema.object().additional_properties = None;
        schema.extensions.insert("nullable".to_owned(), json!(true));
        Schema::Object(schema)
    }
}
