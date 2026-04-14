use std::collections::HashMap;

use tera::Context;

#[derive(Default)]
pub(super) struct TeraContextFactory {
    docfx: bool,
    variables: HashMap<String, String>,
}

impl TeraContextFactory {
    pub(super) fn with_docfx(mut self, docfx: bool) -> Self {
        self.docfx = docfx;
        self
    }

    #[allow(dead_code)]
    pub(super) fn with_variables(mut self, variables: HashMap<String, String>) -> Self {
        self.variables = variables;
        self
    }

    pub(super) fn global_context(&self) -> Context {
        let mut context = Context::new();
        context.insert("docfx", &self.docfx);
        for (name, value) in &self.variables {
            context.insert(name, value);
        }
        context
    }
}
