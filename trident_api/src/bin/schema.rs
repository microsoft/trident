use schemars::gen::SchemaSettings;
use trident_api::config::HostConfiguration;

fn main() {
    let settings = SchemaSettings::draft07().with(|s| {
        s.option_nullable = true;
        s.option_add_null_type = false;
    });
    let gen = settings.into_generator();
    let mut schema = gen.into_root_schema_for::<HostConfiguration>();

    // Because netplan-types currently does not support schemars, we have to
    // manually provide a placeholder schema for the netplan fields using
    // `schemars(with = "...")`. These are Option<> fields, but overriding
    // schematization using `with` removes this behavior. (is_option is a
    // "private" function in the JsonSchema trait) This means we have to
    // manually edit the schema to remove these two fields from the required
    // list.
    schema.schema.object().required.remove("network");
    schema.schema.object().required.remove("networkProvision");

    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}
