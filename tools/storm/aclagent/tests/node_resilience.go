package tests

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	stormproxies "tridenttools/storm/aclagent/proxies"
	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormssh "tridenttools/storm/utils/ssh"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

// aclAgentService is the systemd unit trident-acl-agent runs as, factored
// out here since RunNodeResilience refers to it repeatedly.
const aclAgentService = "trident-acl-agent.service"

// RunNodeResilience proves the fix in
// https://github.com/microsoft/trident/pull/816: trident-acl-agent must
// survive its own Kubernetes Node object disappearing (an HTTP 404 from the
// apiserver, surfaced internally as K8sClientError::NodeGone) instead of
// exiting the process, and must resume normal operation once the Node
// reappears.
//
// kube-rs's watcher() relist path treats a vanished Node as an empty LIST
// result, not a 404 (a fieldSelector-filtered LIST matching zero objects is
// still a 200), so a passive NodeStore.DeleteNode() alone, with nothing
// else happening, would never exercise the fix at all - the long-lived
// watch connection just silently stops seeing the node. NodeGone only ever
// originates from an explicit single-object GET or PATCH call. The
// deterministic trigger used here is a service restart while the Node is
// "gone": that forces trident-acl-agent's startup path
// (get_node_with_retry, called from recover_from_trident_state) to issue
// exactly the kind of GET that turns into NodeGone, without needing any
// Nebraska/image-server mocks or a real stage/finalize/rollback request.
func RunNodeResilience(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	vmIP, err := stormvm.GetVmIP(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP: %w", err)
	}
	if testConfig.OutputPath != "" {
		if err := os.MkdirAll(testConfig.OutputPath, 0o755); err != nil {
			return err
		}
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Only the fake apiserver is needed - no Nebraska/image-server mocks,
	// since this test never issues a stage/finalize/rollback request (see
	// doc comment above). trident-acl-agent never gets a config file at
	// all (see prepareVmForAclAgent's doc comment in update.go).
	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(testConfig.NodeName, map[string]string{}))
	apiServer := stormproxies.NewAPIServer(testConfig.NodeName, nodeStore)
	_, apiServerStop, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort))
	if err != nil {
		return fmt.Errorf("failed to start fake apiserver: %w", err)
	}
	// Deferred instead of relying on ctx cancellation alone: run-ab-update
	// (the next test case) binds the same HostEndpointIP:APIServerPort and
	// must not race this apiserver's teardown - same reasoning as
	// RunABUpdate's own apiServerStop defer.
	defer apiServerStop()

	nodeStore.SetReadyCondition(true)

	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig, nil); err != nil {
		return err
	}
	if err := waitForServiceActive(vmConfig.VMConfig, vmIP, aclAgentService, 30*time.Second); err != nil {
		return fmt.Errorf("failed waiting for %s to become active before test: %w", aclAgentService, err)
	}

	// Sanity check: prove the fake apiserver and agent are talking to each
	// other at all before breaking anything.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", true, nil); err != nil {
		return fmt.Errorf("pre-test validate-connection check failed: %w", err)
	}

	// Make the Node vanish, then restart the agent so its startup
	// get_node_with_retry call hits the fake apiserver's new 404 instead of
	// relying on the (unobservable, from this harness's perspective) watch
	// stream to ever notice.
	nodeStore.DeleteNode()
	if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo systemctl restart %s", aclAgentService)); err != nil {
		return fmt.Errorf("failed to restart %s: %w", aclAgentService, err)
	}

	// Confirm the agent actually took the new resilience code path (not
	// just that it happened to remain active for some unrelated reason).
	const nodeGoneLogSubstring = "no longer exists; waiting for it to reappear"
	if err := waitForJournalContains(vmConfig.VMConfig, vmIP, aclAgentService, nodeGoneLogSubstring, 30*time.Second); err != nil {
		return fmt.Errorf("agent did not log entering the node-gone resilience path: %w", err)
	}

	// Capture the MainPID only now (after the one intentional restart
	// above), then prove it stays stable while the Node remains gone - the
	// whole point of the fix is that the process must park and retry here,
	// not crash or exit.
	pidWhileGone, err := readServiceMainPID(vmConfig.VMConfig, vmIP, aclAgentService)
	if err != nil {
		return fmt.Errorf("failed to read MainPID while node is gone: %w", err)
	}
	if err := assertServiceMainPIDUnchanged(vmConfig.VMConfig, vmIP, aclAgentService, pidWhileGone, 20*time.Second); err != nil {
		return fmt.Errorf("%s did not survive the Node being gone: %w", aclAgentService, err)
	}

	// Restore the Node and confirm the agent notices and resumes, still as
	// the very same process.
	nodeStore.RestoreNode()
	const nodeReappearedLogSubstring = "reappeared after"
	if err := waitForJournalContains(vmConfig.VMConfig, vmIP, aclAgentService, nodeReappearedLogSubstring, 60*time.Second); err != nil {
		return fmt.Errorf("agent did not log resuming after the Node reappeared: %w", err)
	}
	if err := assertServiceMainPIDUnchanged(vmConfig.VMConfig, vmIP, aclAgentService, pidWhileGone, 15*time.Second); err != nil {
		return fmt.Errorf("%s did not remain stable after the Node reappeared: %w", aclAgentService, err)
	}

	// A plain validate-connection check must now succeed again too,
	// proving the fake apiserver side of the test (not just the agent's
	// own logs) agrees the Node is back and reachable.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", true, nil); err != nil {
		return fmt.Errorf("post-recovery validate-connection check failed: %w", err)
	}

	return collectAclArtifacts(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
}

// readServiceMainPID returns the systemd MainPID of the given unit. Used to
// prove trident-acl-agent.service survives the Node going away and
// reappearing without ever being restarted or crashing - unlike plain
// "is-active", which a crash-loop's automatic restart could also satisfy
// moments after a crash.
func readServiceMainPID(cfg stormvmconfig.VMConfig, vmIP, service string) (string, error) {
	out, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo systemctl show %s --property=MainPID --value", service))
	if err != nil {
		return "", fmt.Errorf("failed to read MainPID for %s: %w", service, err)
	}
	return strings.TrimSpace(out), nil
}

// assertServiceMainPIDUnchanged polls the given unit's MainPID over a
// bounded window and fails if it ever stops being active or its MainPID
// changes, either of which would indicate trident-acl-agent crashed or was
// restarted instead of surviving the Node disappearing - mirroring
// rollback.go's assertVmBootIDUnchanged pattern for "assert nothing bad
// happened" checks.
func assertServiceMainPIDUnchanged(cfg stormvmconfig.VMConfig, vmIP, service, pidBefore string, window time.Duration) error {
	const pollInterval = 2 * time.Second
	deadline := time.Now().Add(window)
	for {
		active, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo systemctl is-active %s", service))
		if err != nil || strings.TrimSpace(active) != "active" {
			journal, journalErr := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo journalctl -u %s --no-pager -n 200", service))
			if journalErr != nil {
				journal = fmt.Sprintf("<failed to collect journal: %v>", journalErr)
			}
			return fmt.Errorf("%s is not active (got %q, err=%v)\njournal for %s:\n%s", service, strings.TrimSpace(active), err, service, journal)
		}
		pidNow, err := readServiceMainPID(cfg, vmIP, service)
		if err != nil {
			return err
		}
		if pidNow != pidBefore {
			return fmt.Errorf("%s MainPID changed (%s -> %s), indicating it was restarted or crashed instead of surviving", service, pidBefore, pidNow)
		}
		if time.Now().After(deadline) {
			return nil
		}
		time.Sleep(pollInterval)
	}
}

// waitForJournalContains polls the given unit's journal until it contains
// substr, or returns an error once timeout elapses. Used to confirm
// trident-acl-agent actually took the node-gone resilience code path
// (crates/trident-acl-agent/src/annotations/orchestrator.rs's
// await_node_recreation), not just that it happened to stay up for some
// unrelated reason.
func waitForJournalContains(cfg stormvmconfig.VMConfig, vmIP, service, substr string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	var lastJournal string
	for time.Now().Before(deadline) {
		journal, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo journalctl -u %s --no-pager", service))
		if err == nil {
			lastJournal = journal
			if strings.Contains(journal, substr) {
				return nil
			}
		}
		time.Sleep(2 * time.Second)
	}
	return fmt.Errorf("journal for %s did not contain %q within %s\njournal:\n%s", service, substr, timeout, lastJournal)
}
