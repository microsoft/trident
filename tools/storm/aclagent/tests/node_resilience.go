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
// NodeGone can only ever reach Orchestrator::run() from one of three call
// sites, and this test exercises the two that are actually reachable
// against a real apiserver:
//
//  1. The startup Node read (recover_from_trident_state ->
//     get_node_with_retry). Exercised by phase 1 below: restarting the
//     agent while the Node is "gone" forces this explicit GET into the
//     fake apiserver's 404.
//  2. An in-flight status PATCH while the agent is already running and
//     connected (reconcile_node -> handle_stage -> publish_status, inside
//     the long-lived watch loop). Exercised by phase 2 below, without any
//     restart, proving the already-running process (same PID throughout)
//     survives too - not just a process that happens to restart into a
//     bad state.
//  3. The watch stream itself yielding a 404. This is NOT exercised here,
//     deliberately: kube-rs's watcher() (crates/trident-acl-agent/src/
//     annotations/k8s.rs's watch_node) lists+watches the node collection
//     filtered by a metadata.name fieldSelector, and a fieldSelector LIST
//     matching zero objects is a 200 with empty items in real Kubernetes,
//     never a 404 - there is no real apiserver behavior this fake could
//     emulate to make that happen. The corresponding branch in run()'s
//     watch loop (is_node_gone_error on a raw stream error) exists purely
//     as defense in depth for an error shape the real API server's
//     documented semantics don't actually produce.
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

	productUUID, err := readVmProductUUID(vmConfig.VMConfig, vmIP)
	if err != nil {
		return err
	}

	// Only the fake apiserver is needed for phase 1. Phase 2 patches in a
	// "stage" request to reach handle_stage's first publish_status call,
	// but is deleted before the agent would ever actually dial the
	// Nebraska server named in it (see phase 2 below), so no Nebraska/
	// image-server mock is needed either. trident-acl-agent never gets a
	// config file at all (see prepareVmForAclAgent's doc comment in
	// update.go).
	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(testConfig.NodeName, map[string]string{}, productUUID))
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

	const nodeGoneLogSubstring = "no longer exists; waiting for it to reappear"
	const nodeReappearedLogSubstring = "reappeared after"

	// --- Phase 1: NodeGone from the startup Node read ---
	//
	// Make the Node vanish, then restart the agent so its startup
	// get_node_with_retry call hits the fake apiserver's new 404 (see this
	// function's doc comment for why a restart, not a passive
	// DeleteNode(), is the trigger here).
	nodeStore.DeleteNode()
	// Captured right before the restart and reused for every
	// waitForJournalOccurrenceCountAbove call below (both phases), so
	// journalctl queries stay scoped to this test run instead of the
	// unit's entire history, while keeping occurrence counts cumulative
	// and comparable across phase 1 and phase 2.
	//
	// Read from the VM's own clock (vmClockNow), not the test runner
	// host's clock (time.Now()): journalctl timestamps entries using the
	// VM's clock, and shortly after boot the VM's clock may not be
	// NTP-synced yet (chronyd needs a few seconds to step/slew it after
	// boot), so a host-side timestamp can race ahead of the VM's clock by
	// multiple seconds. That skew made this scoped query flaky - the
	// since-cutoff could land just *after* the exact restart instant it
	// needs to capture, excluding the very "no longer exists" log line
	// phase 1 waits for, even though the agent logged it immediately and
	// correctly. Reading "now" from the VM itself keeps both sides of the
	// comparison in the same clock domain, immune to any skew or sync
	// state.
	journalSince, err := vmClockNow(vmConfig.VMConfig, vmIP)
	if err != nil {
		return fmt.Errorf("failed to read VM clock for journal scoping: %w", err)
	}
	if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo systemctl restart %s", aclAgentService)); err != nil {
		return fmt.Errorf("failed to restart %s: %w", aclAgentService, err)
	}

	// Confirm the agent actually took the new resilience code path (not
	// just that it happened to remain active for some unrelated reason).
	nodeGoneCount, err := waitForJournalOccurrenceCountAbove(vmConfig.VMConfig, vmIP, aclAgentService, nodeGoneLogSubstring, 0, 30*time.Second, journalSince)
	if err != nil {
		return fmt.Errorf("phase 1: agent did not log entering the node-gone resilience path: %w", err)
	}

	// Capture the MainPID only now (after the one intentional restart
	// above), then prove it stays stable while the Node remains gone - the
	// whole point of the fix is that the process must park and retry here,
	// not crash or exit. This same PID is reused as the baseline for phase
	// 2 below too, so that phase additionally proves no *second* restart
	// ever happens.
	pid, err := readServiceMainPID(vmConfig.VMConfig, vmIP, aclAgentService)
	if err != nil {
		return fmt.Errorf("phase 1: failed to read MainPID while node is gone: %w", err)
	}
	if err := assertServiceMainPIDUnchanged(vmConfig.VMConfig, vmIP, aclAgentService, pid, 20*time.Second); err != nil {
		return fmt.Errorf("phase 1: %s did not survive the Node being gone: %w", aclAgentService, err)
	}

	// Restore the Node and confirm the agent notices and resumes, still as
	// the very same process.
	nodeStore.RestoreNode()
	reappearedCount, err := waitForJournalOccurrenceCountAbove(vmConfig.VMConfig, vmIP, aclAgentService, nodeReappearedLogSubstring, 0, 60*time.Second, journalSince)
	if err != nil {
		return fmt.Errorf("phase 1: agent did not log resuming after the Node reappeared: %w", err)
	}
	if err := assertServiceMainPIDUnchanged(vmConfig.VMConfig, vmIP, aclAgentService, pid, 15*time.Second); err != nil {
		return fmt.Errorf("phase 1: %s did not remain stable after the Node reappeared: %w", aclAgentService, err)
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", true, nil); err != nil {
		return fmt.Errorf("phase 1: post-recovery validate-connection check failed: %w", err)
	}

	// --- Phase 2: NodeGone from an in-flight status PATCH, no restart ---
	//
	// Patch in a well-formed "stage" request while the Node still exists,
	// so the agent's already-open watch connection delivers it. handle_stage
	// calls publish_status(&in_progress) - a PATCH - as its very first
	// action once from_version != to_version, before it ever touches
	// Nebraska (see orchestrator.rs's handle_stage). The Node must go
	// missing no later than that reactive PATCH, so DeleteAfterNextPatch
	// arms the fake apiserver to flip the Node missing atomically inside
	// the very same locked MergePatch call that applies this stage patch
	// and delivers its broadcast - rather than a separate DeleteNode()
	// call made after RunScenario returns, which would instead depend on
	// winning a race against the agent's network round-trip reaction to
	// that broadcast. targetVersion is set to testConfig.TargetVersion
	// (guaranteed different from the VM's current version - the same
	// value run-ab-update stages for real) purely so from_version !=
	// to_version; server/appId/track are syntactically valid but
	// unreachable (nebraska.example.invalid, RFC 2606) since the agent
	// must never actually get far enough to dial them - this phase only
	// proves the PATCH-triggered NodeGone path, not a real stage.
	rp := &stormproxies.RPClient{APIServerURL: fmt.Sprintf("http://%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort), NodeName: testConfig.NodeName}
	stageScenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Patch: &stormproxies.PatchStep{
			NodeUpdateID:         "99999999-9999-9999-9999-999999999999",
			OperationID:          "88888888-8888-8888-8888-888888888888",
			Operation:            "stage",
			TargetOSImageVersion: testConfig.TargetVersion,
			Server:               "https://nebraska.example.invalid/v1/update",
			AppId:                "node-resilience-test",
			Track:                "stable",
		}},
	}}
	nodeStore.DeleteAfterNextPatch()
	if _, err := rp.RunScenario(ctx, stageScenario); err != nil {
		return fmt.Errorf("phase 2: failed to patch in stage request: %w", err)
	}

	nodeGoneCount, err = waitForJournalOccurrenceCountAbove(vmConfig.VMConfig, vmIP, aclAgentService, nodeGoneLogSubstring, nodeGoneCount, 30*time.Second, journalSince)
	if err != nil {
		return fmt.Errorf("phase 2: agent did not log entering the node-gone resilience path for an in-flight PATCH: %w", err)
	}
	// Same pid as phase 1's baseline: proves this second NodeGone episode,
	// triggered without any restart at all, is survived by the exact same
	// already-running process.
	if err := assertServiceMainPIDUnchanged(vmConfig.VMConfig, vmIP, aclAgentService, pid, 20*time.Second); err != nil {
		return fmt.Errorf("phase 2: %s did not survive an in-flight PATCH hitting a gone Node: %w", aclAgentService, err)
	}

	// Clear the stale stage request before restoring the Node: the agent
	// never recorded this operationId as completed (publish_status failed
	// before any local state was persisted - see handle_stage), so on
	// resuming it would otherwise reprocess the very same annotation and
	// genuinely try to reach the unreachable nebraska.example.invalid
	// above, which is irrelevant to this test and would fail for unrelated
	// reasons. The agent's watch connection is already torn down at this
	// point (reconcile_node's error broke it out of the watch loop before
	// await_node_recreation parks it), so this direct NodeStore mutation
	// (not going through HTTP, so unaffected by DeleteNode) is never
	// raced against a live watcher.
	nodeStore.PatchAnnotations(map[string]string{stormproxies.UpdateRequestAnnotation: ""})
	nodeStore.RestoreNode()

	if _, err := waitForJournalOccurrenceCountAbove(vmConfig.VMConfig, vmIP, aclAgentService, nodeReappearedLogSubstring, reappearedCount, 60*time.Second, journalSince); err != nil {
		return fmt.Errorf("phase 2: agent did not log resuming after the Node reappeared: %w", err)
	}
	if err := assertServiceMainPIDUnchanged(vmConfig.VMConfig, vmIP, aclAgentService, pid, 15*time.Second); err != nil {
		return fmt.Errorf("phase 2: %s did not remain stable after the Node reappeared: %w", aclAgentService, err)
	}

	// A plain validate-connection check must now succeed again too,
	// proving the fake apiserver side of the test (not just the agent's
	// own logs) agrees the Node is back and reachable.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", true, nil); err != nil {
		return fmt.Errorf("phase 2: post-recovery validate-connection check failed: %w", err)
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

// waitForJournalOccurrenceCountAbove polls the given unit's journal until it
// contains more than minCount occurrences of substr, returning the new
// count once it does, or an error once timeout elapses. Counting
// occurrences (rather than just checking presence, as a single
// waitForJournalContains-style helper would) lets RunNodeResilience tell
// phase 2's log lines apart from phase 1's identical ones earlier in the
// same journal.
//
// vmClockNow returns the VM's own current time (UTC, second precision,
// formatted for journalctl --since) by querying it directly over SSH,
// rather than using the test runner host's clock. journalctl timestamps
// are generated by the VM, so comparing against a host-side timestamp is
// only safe once the VM's clock is NTP-synced - which it may not yet be in
// the first few seconds after boot. Reading the VM's clock directly keeps
// both sides of any --since comparison in the same clock domain.
func vmClockNow(cfg stormvmconfig.VMConfig, vmIP string) (string, error) {
	out, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, "date -u '+%Y-%m-%d %H:%M:%S'")
	if err != nil {
		return "", fmt.Errorf("failed to read VM clock: %w", err)
	}
	return strings.TrimSpace(out), nil
}

// since (a journalctl --since timestamp, e.g. from vmClockNow) scopes
// every poll to entries emitted no earlier than the start of this test
// run, rather than re-fetching and re-scanning the unit's entire history
// on every 2s tick - on a long-lived or noisy unit that would get slower
// over time and add unnecessary load. Callers that need a cumulative count
// across multiple calls (as RunNodeResilience's phase 1/phase 2 do) must
// pass the same since value to every call so counts stay comparable.
func waitForJournalOccurrenceCountAbove(cfg stormvmconfig.VMConfig, vmIP, service, substr string, minCount int, timeout time.Duration, since string) (int, error) {
	deadline := time.Now().Add(timeout)
	var lastJournal string
	var lastCount int
	for time.Now().Before(deadline) {
		journal, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo journalctl -u %s --no-pager --since %q", service, since))
		if err == nil {
			lastJournal = journal
			lastCount = strings.Count(journal, substr)
			if lastCount > minCount {
				return lastCount, nil
			}
		}
		time.Sleep(2 * time.Second)
	}
	return lastCount, fmt.Errorf("journal for %s did not show more than %d occurrence(s) of %q within %s (saw %d)\njournal:\n%s", service, minCount, substr, timeout, lastCount, lastJournal)
}
