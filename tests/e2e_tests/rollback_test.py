import fabric
import pytest
import yaml

from base_test import get_host_status

pytestmark = [pytest.mark.rollback]


def test_rollback(
    connection: fabric.Connection,
    tridentCommand: str,
    abActiveVolume: str,
    expectedHostStatusState: str,
) -> None:
    print("Starting rollback test...")
    # Check Host Status
    host_status = get_host_status(connection, tridentCommand)
    # Assert that the servicing state is as expected
    assert host_status["servicingState"] == expectedHostStatusState
    # Assert that the last error reflects health.checks failure
    serializedLastError = yaml.dump(host_status["lastError"], default_flow_style=False)
    assert "Failed health-checks" in serializedLastError

    if expectedHostStatusState == "not-provisioned":
        # Assert that the active volume has not been set
        assert "abActiveVolume" not in host_status
    else:
        # Assert that the active volume has not changed
        assert host_status["abActiveVolume"] == abActiveVolume

    # Check log files for expected failure messages
    listLogsResult = connection.run(
        "sudo ls /var/lib/trident/trident-update-check-failure-*.log"
    )
    print(f"Log files: {listLogsResult.stdout.strip()}")
    # There should be 1 log file
    assert len(listLogsResult.stdout.strip().splitlines()) == 1
    # Get log file contents
    logResultContentResult = connection.run(f"sudo cat {listLogsResult.stdout.strip()}")
    print(f"Log file contents:\n{logResultContentResult.stdout}")
    # Verify that script failure message is in log file
    assert (
        "Script 'invoke-rollback-from-script' failed" in logResultContentResult.stdout
    )
    # Verify that systemd 2 failure messages are in log file
    assert (
        "Unit non-existent-service1.service could not be found"
        in logResultContentResult.stdout
    )
    assert (
        "Unit non-existent-service2.service could not be found"
        in logResultContentResult.stdout
    )
