import fabric
import pytest

pytestmark = [pytest.mark.extensions]


def test_extensions(
    connection: fabric.Connection,
    hostConfiguration: dict,
) -> None:
    osConfig = hostConfiguration["os"]
    sysextsConfig = osConfig["sysexts"]
    for sysext in sysextsConfig:
        # Verify that the path exists on the target OS
        path = sysext["path"]
        result = connection.run(f"test -e path {path}", warn=True)
        assert result.ok, f"sysext path does not exist: {path}"

    confextsConfig = osConfig["confexts"]
    for confext in confextsConfig:
        # Verify that the path exists on the target OS
        path = confext["path"]
        result = connection.run(f"test -e path {path}", warn=True)
        assert result.ok, f"confext path does not exist: {path}"
