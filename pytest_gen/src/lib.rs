use std::{ops::Deref, path::MAIN_SEPARATOR_STR};

use proc_macro::{TokenStream, TokenTree};
use quote::{quote, ToTokens};
use syn::Lit;

/// Negative/positive test case metadata enum
#[derive(PartialEq)]
enum TestCaseMetadataAttribute {
    Xfail,
    Skip,
    Negative,
    Feature,
}

/// Internal representation of test case metadata, populated from TokenStream of
/// the function annotated by `#[functional_test()]` attribute. `function` is inferred from
/// the function name, the rest can be provided by the user.
#[derive(Default, Debug)]
struct TestCaseMetadataInt {
    function: String,
    negative: bool,
    feature: String,
    xfail: StringOption,
    skip: StringOption,
}

/// Wrapper for `Option<String>` to implement `ToTokens` trait
#[derive(Debug, Default)]
struct StringOption(pub Option<String>);

impl StringOption {
    /// Creates a new `StringOption` with the provided value.
    fn new(value: String) -> Self {
        Self(Some(value))
    }
}

impl Deref for StringOption {
    type Target = Option<String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ToTokens for StringOption {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let value = match &self.0 {
            Some(value) => quote! { Some(#value) },
            None => quote! { None },
        };
        tokens.extend(value);
    }
}

/// The `functional_test` attribute macro. This macro is used to annotate test functions
/// that should be exposed to the pytest test runner. Currently, this should be
/// only done for the functional tests. This macro will automatically add the
/// `#[test]` attribute to the function, and will also add the function to the list
/// of tests that are submitted to the pytest test runner (though that list
/// needs to be generated separately using `make generate-pytest-wrappers`).
///
/// The attribute accepts a comma-separated list of key-value pairs, where the
/// supported keys are: `negative` and `feature`.
///
/// The `negative` key is used to mark the test as negative, meaning it tests
/// for a failure case. The expected associated value type is `boolean`. The
/// `negative` key is optional, and if not provided, the test will be considered
/// as positive (aka `negative: false`).
///
/// The `feature` key is used to mark the test as belonging to a specific
/// feature. The expected associated  value type is `string` and supported
/// values are: `raid`, `verity`, `encryption`, `abupdate`, `core`, `helpers`.
/// The `feature` key is optional, and if not provided, the test will be
/// considered as belonging to the `core` feature.
#[proc_macro_attribute]
pub fn functional_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    pytest(attr, item, "functional")
}

fn pytest(attr: TokenStream, item: TokenStream, test_type: &str) -> TokenStream {
    let mut metadata = extract_test_case_metadata(attr);

    // Parse the passed item as a function
    let func = syn::parse_macro_input!(item as syn::ItemFn);

    // Break the function down into its parts
    let syn::ItemFn {
        attrs,
        vis,
        sig,
        block,
    } = func;

    // Extract the name of the function
    metadata.function = format!("{}", sig.ident);

    validate_metadata(&metadata);

    let function = metadata.function.as_str();
    let negative = metadata.negative;
    let feature = metadata.feature.as_str();
    let xfail = metadata.xfail;
    let skip = metadata.skip;

    // Construct the output, injecting the inventory::submit!() and `#[test]` macros
    let output = quote! {
        inventory::submit!{pytest::TestCaseMetadata {
            module: module_path!(),
            function: #function,
            negative: #negative,
            feature: #feature,
            xfail: #xfail,
            skip: #skip,
            type_: #test_type,
        }}

        #[test]
        #(#attrs)*
        #vis #sig {
            #block
        }
    };
    TokenStream::from(output)
}

/// Extracts the test case metadata from the `#[functional_test]` attribute argument
/// list. Failures here will result in red squiggles in VSCode when running with
/// Rust Analyzer.
fn extract_test_case_metadata(attr: TokenStream) -> TestCaseMetadataInt {
    let mut metadata = TestCaseMetadataInt {
        ..Default::default()
    };

    let mut current_key: Option<TestCaseMetadataAttribute> = None;
    for token in attr {
        match token {
            TokenTree::Ident(ident) => {
                let ident_str = ident.to_string();
                match ident_str.as_str() {
                    "negative" => {
                        if current_key.is_some() {
                            panic!("Missing attribute value");
                        }

                        current_key = Some(TestCaseMetadataAttribute::Negative);
                    }
                    "feature" => {
                        if current_key.is_some() {
                            panic!("Missing attribute value");
                        }
                        current_key = Some(TestCaseMetadataAttribute::Feature);
                    }
                    "xfail" => {
                        if current_key.is_some() {
                            panic!("Missing attribute value");
                        }

                        current_key = Some(TestCaseMetadataAttribute::Xfail);
                    }
                    "skip" => {
                        if current_key.is_some() {
                            panic!("Missing attribute value");
                        }

                        current_key = Some(TestCaseMetadataAttribute::Skip);
                    }
                    "true" | "false" => {
                        if current_key != Some(TestCaseMetadataAttribute::Negative) {
                            panic!("Unknown attribute value: {}", ident_str);
                        }
                        metadata.negative = ident_str == "true";
                    }
                    _ => {
                        panic!("Unknown attribute name: {}", ident_str);
                    }
                }
            }
            TokenTree::Punct(punct) => {
                let punct_str = punct.to_string();
                match punct_str.as_str() {
                    "=" => {
                        if current_key.is_none() {
                            panic!("= found, but attribute key was not specified");
                        }
                    }
                    "," => {
                        if current_key.is_none() {
                            panic!(", found, but attribute key was not specified");
                        }
                        current_key = None;
                    }
                    _ => {
                        panic!("Unknown attribute separator: {}", punct_str);
                    }
                }
            }
            TokenTree::Literal(_) => {
                let ts = TokenStream::from(token.clone());
                let ast: Lit = syn::parse(ts).unwrap();
                let literal_str = match &ast {
                    Lit::Str(lit_str) => Some(lit_str.value()),
                    _ => None,
                };
                let literal_bool = match ast {
                    Lit::Bool(lit_bool) => Some(lit_bool.value()),
                    _ => None,
                };
                if literal_bool.is_none() && literal_str.is_none() {
                    panic!("Unknown attribute value: {:?}", token);
                }
                match current_key {
                    None => {
                        panic!("Missing attribute key")
                    }
                    Some(ref key) => match key {
                        TestCaseMetadataAttribute::Negative => {
                            panic!("Unexpected attribute key: {:?}", token)
                            // handled above
                        }
                        TestCaseMetadataAttribute::Feature => {
                            metadata.feature = literal_str.unwrap();
                        }
                        TestCaseMetadataAttribute::Xfail => {
                            metadata.xfail = StringOption::new(literal_str.unwrap());
                        }
                        TestCaseMetadataAttribute::Skip => {
                            metadata.skip = StringOption::new(literal_str.unwrap());
                        }
                    },
                }
            }
            _ => {
                panic!("Unknown attribute: {:?}", token);
            }
        }
    }
    metadata
}

/// Validates the test case metadata. Failures here will result in red squiggles
/// in VSCode when running with Rust Analyzer.
fn validate_metadata(metadata: &TestCaseMetadataInt) {
    if metadata.function.is_empty() || metadata.function.contains(MAIN_SEPARATOR_STR) {
        panic!("Missing function attribute or function name contains path separator");
    }
    match metadata.feature.as_str() {
        "raid" | "encryption" | "verity" | "abupdate" | "core" | "helpers" | "engine" | "" => {}
        _ => panic!("Unknown feature: {}", metadata.feature),
    }
    if !metadata.function.starts_with("test_") {
        panic!(
            "Test case name must start with \"test_\", got: {}",
            metadata.function
        );
    }
    if let Some(content) = metadata.xfail.as_ref() {
        if content.trim().is_empty() {
            panic!("xfail attribute must contain a non-empty string explanation");
        }
    }

    if let Some(content) = metadata.skip.as_ref() {
        if content.trim().is_empty() {
            panic!("skip attribute must contain a non-empty string explanation");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_sample_metadata() -> TestCaseMetadataInt {
        TestCaseMetadataInt {
            function: "test_settle".into(),
            negative: false,
            feature: "helpers".into(),
            xfail: StringOption::default(),
            skip: StringOption::default(),
        }
    }

    #[test]
    fn test_validate_metadata() {
        validate_metadata(&get_sample_metadata());
    }

    #[test]
    #[should_panic(
        expected = "Missing function attribute or function name contains path separator"
    )]
    fn test_validate_metadata_missing_function() {
        let mut metadata = get_sample_metadata();
        metadata.function = "".into();
        validate_metadata(&metadata);
    }

    #[test]
    #[should_panic(
        expected = "Missing function attribute or function name contains path separator"
    )]
    fn test_validate_metadata_bad_function() {
        let mut metadata = get_sample_metadata();
        metadata.function = MAIN_SEPARATOR_STR.into();
        validate_metadata(&metadata);
    }

    #[test]
    #[should_panic(expected = "Unknown feature: invalid")]
    fn test_validate_metadata_invalid_feature() {
        let mut metadata = get_sample_metadata();
        metadata.feature = "invalid".into();
        validate_metadata(&metadata);
    }

    #[test]
    #[should_panic(expected = "Test case name must start with \"test_\", got: invalid")]
    fn test_validate_metadata_invalid_function_name() {
        let mut metadata = get_sample_metadata();
        metadata.function = "invalid".into();
        validate_metadata(&metadata);
    }
}
