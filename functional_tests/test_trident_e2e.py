import yaml

from .tools.trident import TridentTool
from .conftest import TRIDENT_REPO_DIR_PATH

class SafeLoaderIgnoreUnknown(yaml.SafeLoader):
    def ignore_unknown(self, node):
        return None

def test_trident_run(vm):
    trident = TridentTool(vm)
    trident.run()

    pass

def test_trident_get(vm):
    trident = TridentTool(vm)

    SafeLoaderIgnoreUnknown.add_constructor(None, SafeLoaderIgnoreUnknown.ignore_unknown)

    host_status = trident.get()
    host_status = yaml.load(host_status, Loader=SafeLoaderIgnoreUnknown)
    # TODO remove the placeholder logic by patching the template with the actual
    # values, which we can fetch using lsblk, sfdisk and information about the
    # images we put into the HostConfiguraion.
    placeholder = "placeholder"
    host_status["storage"]["disks"]["os"]["uuid"] = placeholder
    for partition in host_status["storage"]["disks"]["os"]["partitions"]:
        partition["uuid"] = placeholder
        partition["path"] = placeholder
        if isinstance(partition["contents"], dict) and "sha256" in partition["contents"]:
            partition["contents"]["sha256"] = placeholder
            partition["contents"]["length"] = placeholder
    with open(TRIDENT_REPO_DIR_PATH / 'functional_tests/host-status-template.yaml', 'r') as file:
        host_status_expected = yaml.load(file, Loader=SafeLoaderIgnoreUnknown)
    assert host_status == host_status_expected

    pass

def test_trident_start_network(vm):
    trident = TridentTool(vm)
    trident.start_network()

    pass
