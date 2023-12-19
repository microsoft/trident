from .tools.runner import RunnerTool

def test_osutils(vm):

    testRunner = RunnerTool(vm)
    testRunner.run("osutils")

    pass

def test_setsail(vm):

    testRunner = RunnerTool(vm)
    testRunner.run("setsail")

    pass

def test_trident(vm):

    testRunner = RunnerTool(vm)
    testRunner.run("trident")

    pass

def test_trident_api(vm):

    testRunner = RunnerTool(vm)
    testRunner.run("trident_api")

    pass
