use tera::Context;

pub(super) struct TeraContextFactory {
    docfx: bool,
}

impl TeraContextFactory {
    pub(super) fn new(docfx: bool) -> Self {
        Self { docfx }
    }

    pub(super) fn global_context(&self) -> Context {
        let mut context = tera::Context::new();
        context.insert("docfx", &self.docfx);
        context
    }
}
