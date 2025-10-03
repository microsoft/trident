import logging
from typing import List
import jinja2

from .libvirthelper import VirtualMachine
from ..utils import TemplateHelper, RenderedTemplate, read_file

log = logging.getLogger(__name__)


class ConfigHelper(TemplateHelper):
    def generate_netlaunch(
        self,
        node: VirtualMachine,
        host_ip: str,
    ) -> RenderedTemplate:
        return self._render_template(
            "netlaunch.yaml.jinja2",
            host_ip=host_ip,
            local_vm_uuid=node.UUIDString,
        )

    def generate_trident(self) -> RenderedTemplate:
        return self._render_template("trident.yaml.jinja2")
