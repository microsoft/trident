package tests

import (
	"context"
	"fmt"
	"os"
	"time"

	stormproxies "tridenttools/storm/aksagent/proxies"
	stormaksconfig "tridenttools/storm/aksagent/utils/config"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

// RunRollback exercises trident-aks-agent's rollback annotation end-to-end
// against the real gRPC-backed RollbackService (rollback_stage +
// rollback_finalize) implemented by tridentd, followed by the real reboot
// and post-reboot commit. It assumes the VM is already staged/finalized to
// testConfig.TargetVersion (i.e. it runs after run-ab-update in the same
// scenario), so ManualRollbackAbStaged/Finalized has a prior version to roll
// back to.
func RunRollback(testConfig stormaksconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	vmIP, err := stormvm.GetVmIP(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP: %w", err)
	}
	if err := os.MkdirAll(testConfig.OutputPath, 0o755); err != nil {
		return err
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Rollback doesn't stage a new image from Nebraska - it re-activates the
	// previously-finalized volume trident already has on disk - so only the
	// fake apiserver is needed here, not the Nebraska/image-server mocks
	// run-ab-update starts. trident-aks-agent never gets a config file at
	// all (see prepareVmForAksAgent); rollback's PatchSteps leave
	// `server`/`appId` unset too, since Nebraska is never queried during a
	// rollback request.
	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(testConfig.NodeName, map[string]string{}))
	apiServer := stormproxies.NewAPIServer(testConfig.NodeName, nodeStore)
	if _, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("0.0.0.0:%d", testConfig.APIServerPort)); err != nil {
		return fmt.Errorf("failed to start fake apiserver: %w", err)
	}

	nodeStore.PatchLabels(map[string]string{stormproxies.NodeImageVersionLabel: testConfig.TargetVersion})
	nodeStore.SetReadyCondition(true)

	if err := prepareVmForAksAgent(vmConfig.VMConfig, vmIP, testConfig); err != nil {
		return err
	}

	rp := &stormproxies.RPClient{APIServerURL: fmt.Sprintf("http://%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort), NodeName: testConfig.NodeName}
	scenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Patch: &stormproxies.PatchStep{NodeUpdateID: "22222222-2222-2222-2222-222222222222", OperationID: "rollback-op", Operation: "rollback"}},
		{Expect: &stormproxies.ExpectStep{OperationID: "rollback-op", Operation: "rollback", Code: "Success", Timeout: 180 * time.Second}},
	}}
	report, err := rp.RunScenario(ctx, scenario)
	logScenarioTimeline("rollback stage/finalize", report)
	if err != nil {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("AKS agent rollback scenario failed (stage/finalize): %w", err)
	}
	if !report.Passed {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("AKS agent rollback scenario failed (stage/finalize): %+v", report)
	}

	// rollback_finalize triggers a real "systemctl reboot" from
	// trident-aks-agent, same as update's finalize - wait it out the same
	// way run-ab-update does.
	nodeStore.SetReadyCondition(false)
	if err := waitForVmRebootAndSshBack(vmConfig, vmIP, testConfig); err != nil {
		return fmt.Errorf("failed waiting for VM to come back after rollback finalize reboot: %w", err)
	}
	nodeStore.SetReadyCondition(true)

	// Same rationale as run-ab-update: the rollback reboot lands on the
	// previous root, which needs the fake kubeconfig re-delivered before it
	// can talk to the fake apiserver again.
	if err := prepareVmForAksAgent(vmConfig.VMConfig, vmIP, testConfig); err != nil {
		return fmt.Errorf("failed to reconfigure AKS agent on post-rollback-reboot root: %w", err)
	}

	finalScenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Expect: &stormproxies.ExpectStep{OperationID: "rollback-op", Operation: "commit", Code: "Success", Timeout: 180 * time.Second}},
	}}
	finalReport, err := rp.RunScenario(ctx, finalScenario)
	logScenarioTimeline("post-rollback-reboot commit", finalReport)
	if err != nil {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("AKS agent rollback scenario failed (post-reboot commit): %w", err)
	}
	if !finalReport.Passed {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("AKS agent rollback scenario failed (post-reboot commit): %+v", finalReport)
	}

	snapshot := nodeStore.Snapshot()
	if got := snapshot.Annotations[stormproxies.UpdateCommitStatusAnnotation]; got == "" {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("final rollback commit status annotation missing")
	}

	// Regression coverage for the "rollback with nothing to roll back"
	// bug: the only AB rollback available was just consumed above, so a
	// second rollback request now must be detected as a no-op (via
	// RollbackStage's servicing_kind, which tridentd now reports the same
	// way update/install do) rather than reporting a false Success and
	// rebooting the node again for no reason. This exercises that fix
	// end-to-end against the real tridentd, not just the mock.
	secondScenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Patch: &stormproxies.PatchStep{NodeUpdateID: "33333333-3333-3333-3333-333333333333", OperationID: "rollback-op-2", Operation: "rollback"}},
		{Expect: &stormproxies.ExpectStep{OperationID: "rollback-op-2", Operation: "rollback", Code: "OperationFailed", Timeout: 60 * time.Second}},
	}}
	secondReport, err := rp.RunScenario(ctx, secondScenario)
	logScenarioTimeline("second rollback with empty chain", secondReport)
	if err != nil {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("AKS agent second-rollback (empty chain) scenario failed: %w", err)
	}
	if !secondReport.Passed {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("AKS agent second-rollback (empty chain) scenario failed: %+v", secondReport)
	}
	// A no-op rollback must not trigger another reboot: the VM should
	// still be reachable immediately, with no reboot wait needed.
	if _, err := stormvm.GetVmIP(vmConfig); err != nil {
		collectAksArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("VM appears to have rebooted (or become unreachable) after a no-op rollback, which should not trigger a reboot: %w", err)
	}

	return collectAksArtifacts(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
}
