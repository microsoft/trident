from typing import Dict, List
import pytest
import re


class ResultAggregator:
    def __init__(self) -> None:
        self.results = []

    def add_report(self, report) -> None:
        self.results.append(report.outcome)

    def success(self) -> bool:
        return all(item == "passed" for item in self.results)


class NodeId:
    RE = re.compile(r"(.*\/)?(\w+\.py)?(::)?(Test\w+)?(::)?(test_\w+)?")

    class Groups:
        PACKAGE = 1
        MODULE = 2
        CLASS = 4
        CASE = 6

    @classmethod
    def parse(cls, val: str) -> "NodeId":
        # Avoid empty strings
        if not val:
            return None
        m = cls.RE.fullmatch(val)
        return NodeId(
            m.group(cls.Groups.PACKAGE),
            m.group(cls.Groups.MODULE),
            m.group(cls.Groups.CLASS),
            m.group(cls.Groups.CASE),
        )

    def __init__(self, test_package, test_module, test_class, test_case):
        self.test_package = test_package
        self.test_module = test_module
        self.test_class = test_class
        self.test_case = test_case

    def merge(self, source: "NodeId") -> None:
        # Only add preceding context!
        # Stop merge once the target (self) has something.
        update_done = False

        def updateifempty(name: str):
            nonlocal update_done
            name = "test_" + name
            if getattr(self, name) is None and not update_done:
                setattr(self, name, getattr(source, name))
            else:
                update_done = True

        updateifempty("package")
        updateifempty("module")
        updateifempty("class")
        updateifempty("case")

    def render(self):
        out = ""
        if self.test_package:
            out = self.test_package
        if self.test_module:
            out += self.test_module
        if self.test_class:
            if out:
                out += "::"
            out += self.test_class
        if self.test_case:
            if out:
                out += "::"
            out += self.test_case
        return out


class DependencyTracker:
    Scopes = {
        # "session": pytest.Session,
        "package": pytest.Package,
        "module": pytest.Module,
        "class": pytest.Class,
        "node": pytest.Item,
    }
    _instance: "DependencyTracker" = None
    RE = re.compile(r"((.*)\/)?(\w+\.py)?(::)?(Test\w+)?(::)?(test_\w+)?")

    @staticmethod
    def instance() -> "DependencyTracker":
        if DependencyTracker._instance is None:
            DependencyTracker._instance = DependencyTracker()
        return DependencyTracker._instance

    def __init__(self) -> None:
        self.results: Dict[str, ResultAggregator] = {}

    def add_result(self, item: pytest.Item, report: pytest.TestReport) -> None:
        parents = {k: item.getparent(v) for k, v in DependencyTracker.Scopes.items()}
        for scope, parent in parents.items():
            if parent is None:
                # print("NOT FOUND PARENT OF TYPE", scope, "for", item.nodeid)
                continue
            name: str = parent.nodeid

            # Remove ugly file in packages
            if scope == "package":
                name = name.replace("__init__.py", "")

            aggregator = self.results.setdefault(name, ResultAggregator())
            if report is not None:
                aggregator.add_report(report)

    def check_dependencies(
        self, item: pytest.Item, relative_dependencies: List[str]
    ) -> None:
        full_names = [
            DependencyTracker.infer_full_id(item, d) for d in relative_dependencies
        ]
        for name in full_names:
            if name not in self.results:
                pytest.skip(f"Dependency not found: {name}")
            else:
                if not self.results[name].success():
                    pytest.skip(f"Dependency failed: {name}")

    @classmethod
    def infer_full_id(cls, source: pytest.Item, target: str) -> str:
        targetid = NodeId.parse(target)
        localid = NodeId.parse(source.nodeid)
        targetid.merge(localid)
        return targetid.render()


# Bulk add all collected items to our tracker
def pytest_collection_modifyitems(session, config, items: List[pytest.Item]):
    dt = DependencyTracker.instance()
    for item in items:
        dt.add_result(item, None)


# Add results to tracker once tests start running
@pytest.hookimpl(tryfirst=True, hookwrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call):
    outcome = yield
    report: pytest.TestReport = outcome.get_result()
    dt = DependencyTracker.instance()
    dt.add_result(item, report)


# Add a new marker for marking dependencies
def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "depends(test*): "
        "mark a test as dependent on another test, "
        "test class, test module or test package",
    )


def pytest_runtest_setup(item: pytest.Item):
    dependencies = []
    for mark in item.iter_markers():
        if mark.name != "depends":
            continue
        dependencies += list(mark.args)
    DependencyTracker.instance().check_dependencies(item, dependencies)
