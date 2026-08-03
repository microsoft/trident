package tests

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	stormfakes "tridenttools/storm/aclagent/fakes"
	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormssh "tridenttools/storm/utils/ssh"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func RunABUpdate(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	vmIP, err := stormvm.GetVmIP(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP: %w", err)
	}
	if err := os.MkdirAll(testConfig.OutputPath, 0o755); err != nil {
		return err
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	nodeStore := stormfakes.NewNodeStore(stormfakes.NewSeedNode(testConfig.NodeName, map[string]string{}))
	apiServer := stormfakes.NewAPIServer(testConfig.NodeName, nodeStore)
	if _, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("127.0.0.1:%d", testConfig.APIServerPort)); err != nil {
		return fmt.Errorf("failed to start fake apiserver: %w", err)
	}
	nebraska := &stormfakes.NebraskaProxy{Scenario: &stormfakes.NebraskaScenario{
		Available:   true,
		Version:     testConfig.TargetVersion,
		URL:         testConfig.NebraskaCodebase,
		SHA384:      testConfig.NebraskaSHA384,
		PackageName: testConfig.NebraskaPackageName,
	}}
	if _, err := nebraska.ListenAndServe(ctx, fmt.Sprintf("127.0.0.1:%d", testConfig.NebraskaPort)); err != nil {
		return fmt.Errorf("failed to start fake Nebraska endpoint: %w", err)
	}

	markerFile := testConfig.RebootMarkerFile
	if markerFile == "" {
		markerFile = filepath.Join(testConfig.OutputPath, "trident-acl-agent-reboot-marker")
	}
	_ = os.Remove(markerFile)

	kubeletCtx, kubeletCancel := context.WithCancel(ctx)
	defer kubeletCancel()
	kubelet := &stormfakes.KubeletProxy{
		NodeStore:       nodeStore,
		BootstrapLabels: map[string]string{stormfakes.NodeImageVersionLabel: testConfig.ExpectedInitialVolume},
		MarkerFile:      markerFile,
		RebootDuration:  time.Duration(testConfig.RebootDurationSeconds) * time.Second,
	}
	go func() {
		if err := kubelet.Run(kubeletCtx); err != nil && kubeletCtx.Err() == nil {
			logrus.Errorf("kubelet proxy failed: %v", err)
		}
	}()

	apiStarted := make(chan bool, 1)
	nebraskaStarted := make(chan bool, 1)
	go stormssh.StartSshProxyPortAndWait(ctx, testConfig.APIServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, apiStarted)
	go stormssh.StartSshProxyPortAndWait(ctx, testConfig.NebraskaPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, nebraskaStarted)
	<-apiStarted
	<-nebraskaStarted

	if err := installRebootShim(vmConfig.VMConfig, vmIP); err != nil {
		return err
	}
	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig); err != nil {
		return err
	}

	rp := &stormfakes.RPClient{APIServerURL: fmt.Sprintf("http://127.0.0.1:%d", testConfig.APIServerPort), NodeName: testConfig.NodeName}
	scenario := &stormfakes.Scenario{Steps: []stormfakes.ScenarioStep{
		{Patch: &stormfakes.PatchStep{Request: "stage", RequestID: "R1", TargetOSImageVersion: testConfig.TargetVersion}},
		{Expect: &stormfakes.ExpectStep{State: "staged", ObservedRequestID: "R1", Timeout: 120 * time.Second}},
		{Patch: &stormfakes.PatchStep{Request: "finalize", RequestID: "R1"}},
		{Expect: &stormfakes.ExpectStep{State: "update-succeeded", ObservedRequestID: "R1", Timeout: 180 * time.Second}},
	}}
	report, err := rp.RunScenario(ctx, scenario)
	if err != nil {
		return fmt.Errorf("ACL agent scenario failed: %w", err)
	}
	if !report.Passed {
		return fmt.Errorf("ACL agent scenario failed: %+v", report)
	}

	snapshot := nodeStore.Snapshot()
	if got := snapshot.Labels[stormfakes.StateLabel]; got != "update-succeeded" {
		return fmt.Errorf("final state mismatch: got %q", got)
	}
	return collectAclArtifacts(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
}

func prepareVmForAclAgent(cfg stormvmconfig.VMConfig, vmIP string, testConfig stormaclconfig.TestConfig) error {
	command := strings.Join([]string{
		"sudo test -f /etc/trident/trident-acl-agent.conf",
		"sudo systemctl restart tridentd.service",
		"sudo systemctl restart trident-acl-agent.service",
		"sudo systemctl is-active tridentd.service",
		"sudo systemctl is-active trident-acl-agent.service",
		fmt.Sprintf("sudo grep -q 'localhost:%d' /etc/trident/trident-acl-agent.conf", testConfig.APIServerPort),
		fmt.Sprintf("sudo grep -q 'localhost:%d' /etc/trident/trident-acl-agent.conf", testConfig.NebraskaPort),
	}, " && ")
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, command); err != nil {
		return fmt.Errorf("failed to prepare VM ACL agent config: %w", err)
	}
	return nil
}

func installRebootShim(cfg stormvmconfig.VMConfig, vmIP string) error {
	localPath := filepath.Join("tools", "storm", "aclagent", "reboot-shim.sh")
	if err := stormssh.ScpUploadFileWithSudo(cfg, vmIP, localPath, "/usr/local/bin/reboot"); err != nil {
		return fmt.Errorf("failed to upload reboot shim as reboot: %w", err)
	}
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, "sudo chmod 0755 /usr/local/bin/reboot && sudo cp /usr/local/bin/reboot /usr/local/bin/systemctl"); err != nil {
		return fmt.Errorf("failed to activate reboot shim: %w", err)
	}
	return nil
}

func collectAclArtifacts(cfg stormvmconfig.VMConfig, vmIP string, outputPath string) error {
	if outputPath == "" {
		return nil
	}
	cmds := []string{
		"sudo journalctl --no-pager -u trident-acl-agent.service > /tmp/trident-acl-agent.log && sudo chmod 644 /tmp/trident-acl-agent.log",
		"sudo journalctl --no-pager -u tridentd.service > /tmp/tridentd.log && sudo chmod 644 /tmp/tridentd.log",
		"sudo cat /etc/trident/trident-acl-agent.conf > /tmp/trident-acl-agent.conf && sudo chmod 644 /tmp/trident-acl-agent.conf",
	}
	for _, cmd := range cmds {
		_, _ = stormssh.SshCommandCombinedOutput(cfg, vmIP, cmd)
	}
	for _, remote := range []string{"/tmp/trident-acl-agent.log", "/tmp/tridentd.log", "/tmp/trident-acl-agent.conf"} {
		if err := stormssh.ScpDownloadFile(cfg, vmIP, remote, filepath.Join(outputPath, filepath.Base(remote))); err != nil {
			logrus.Warnf("failed to download %s: %v", remote, err)
		}
	}
	return nil
}
