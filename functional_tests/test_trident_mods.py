from .tools.runner import RunnerTool


def test_osutils(vm):
    """Invokes unit and functional tests for the osutils crate."""
    testRunner = RunnerTool(vm)
    testRunner.run("osutils")

    pass


def test_setsail(vm):
    """Invokes unit and functional tests for the setsail crate."""
    testRunner = RunnerTool(vm)
    testRunner.run("setsail")

    pass


def test_trident(vm):
    """Invokes unit and functional tests for the trident crate."""
    testRunner = RunnerTool(vm)
    testRunner.run("trident")

    pass


def test_trident_api(vm):
    """Invokes unit and functional tests for the trident_api crate."""
    testRunner = RunnerTool(vm)
    testRunner.run("trident_api")

    pass
