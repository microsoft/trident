import jinja2


class RenderedTemplate:
    def __init__(self, data: str) -> None:
        self.data = data

    def write(self, dest_file: str) -> None:
        with open(dest_file, "w", encoding="utf-8") as f:
            f.write(self.data)


class TemplateHelper(object):
    def __init__(self) -> None:
        self.env = jinja2.Environment(
            loader=jinja2.PackageLoader("virtdeploy"),
            autoescape=jinja2.select_autoescape(),
        )

    def _render_template(self, template: str, **kwargs) -> RenderedTemplate:
        return RenderedTemplate(self.env.get_template(template).render(kwargs))

    def generate(self) -> RenderedTemplate:
        raise NotImplementedError()
