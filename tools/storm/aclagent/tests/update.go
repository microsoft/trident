package tests

import (
	"archive/tar"
	"context"
	"crypto/sha512"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	stormproxies "tridenttools/storm/aclagent/proxies"
	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormfile "tridenttools/storm/utils/file"
	stormssh "tridenttools/storm/utils/ssh"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

// sha384File computes the lowercase hex-encoded SHA-384 digest that tridentd
// expects for a given image path.
//
// For a .cosi file, tridentd does NOT hash the whole archive: a COSI is a
// plain tar with an embedded "metadata.json" entry, and tridentd's Host
// Configuration "sha384" field must match the hash of just that entry's
// bytes (see crates/trident/src/osimage/cosi/mod.rs's read_cosi_metadata).
// For any other file, this hashes the whole file's contents directly.
// logScenarioTimeline prints a human-readable, step-by-step trace of a
// scenario's progress through the ACL agent's stage/finalize/commit state
// machine. It runs regardless of pass/fail so a test run's output always
// documents exactly how far the state machine got and why, instead of
// forcing readers to reconstruct the timeline from raw journal logs.
func logScenarioTimeline(label string, report *stormproxies.ScenarioReport) {
	if report == nil {
		return
	}
	logrus.Infof("=== %s state machine timeline ===", label)
	for _, step := range report.Steps {
		status := "PASS"
		if !step.Passed {
			status = "FAIL"
		}
		logrus.Infof("  [%d] %-6s kind=%-8s (%dms) %s", step.Index, status, step.Kind, step.ElapsedMS, step.Message)
		if !step.Passed {
			logrus.Infof("        expected: %+v", step.Expected)
			logrus.Infof("        actual:   %+v", step.Actual)
		}
	}
	overall := "PASSED"
	if !report.Passed {
		overall = "FAILED"
	}
	logrus.Infof("=== %s state machine timeline: %s ===", label, overall)
}

func sha384File(path string) (string, error) {
	if strings.HasSuffix(path, ".cosi") {
		return sha384CosiMetadata(path)
	}

	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()

	h := sha512.New384()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

// sha384CosiMetadata extracts the "metadata.json" entry from a COSI (a plain
// tar archive) and returns the lowercase hex-encoded SHA-384 digest of its
// raw bytes, matching what tridentd validates the Host Configuration's
// "sha384" field against.
func sha384CosiMetadata(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()

	tr := tar.NewReader(f)
	for {
		header, err := tr.Next()
		if err == io.EOF {
			return "", fmt.Errorf("metadata.json entry not found in COSI file %s", path)
		}
		if err != nil {
			return "", fmt.Errorf("failed to read COSI tar entries in %s: %w", path, err)
		}
		if header.Name != "metadata.json" {
			continue
		}
		h := sha512.New384()
		if _, err := io.Copy(h, tr); err != nil {
			return "", fmt.Errorf("failed to hash metadata.json in COSI file %s: %w", path, err)
		}
		return hex.EncodeToString(h.Sum(nil)), nil
	}
}

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

	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(testConfig.NodeName, map[string]string{}))
	apiServer := stormproxies.NewAPIServer(testConfig.NodeName, nodeStore)
	// Bind on all interfaces (not 127.0.0.1) so the VM can reach the fake
	// apiserver directly over the libvirt NAT network at testConfig.HostEndpointIP,
	// instead of relying on reverse SSH tunnels. Tunnels don't survive a real
	// VM reboot; a real host IP does.
	if _, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("0.0.0.0:%d", testConfig.APIServerPort)); err != nil {
		return fmt.Errorf("failed to start fake apiserver: %w", err)
	}

	nebraskaCodebase := testConfig.NebraskaCodebase
	nebraskaPackageName := testConfig.NebraskaPackageName
	nebraskaSHA384 := testConfig.NebraskaSHA384

	// tridentd downloads and hashes the image itself as part of staging a
	// runtime update; it is not enough for the acl-agent to merely reach the
	// fake apiserver/Nebraska endpoints. When a real image is configured, serve
	// it over plain HTTP from this same test runner and advertise its real
	// SHA-384 hash, so tridentd's download+verify path is exercised faithfully
	// instead of failing on an unreachable https://example.invalid URL or a
	// hash that doesn't match any downloadable bytes.
	imagePath := testConfig.ImagePath
	if imagePath == "" {
		found, err := stormfile.FindFile(testConfig.ArtifactsDir, ".*\\.cosi$")
		if err != nil {
			return fmt.Errorf("failed to find a .cosi update image under %s: %w", testConfig.ArtifactsDir, err)
		}
		imagePath = found
	}

	{
		hash, err := sha384File(imagePath)
		if err != nil {
			return fmt.Errorf("failed to hash image %s: %w", imagePath, err)
		}
		imageServer := &stormproxies.ImageServer{ImagePath: imagePath}
		if _, err := imageServer.ListenAndServe(ctx, fmt.Sprintf("0.0.0.0:%d", testConfig.ImageServerPort)); err != nil {
			return fmt.Errorf("failed to start fake image server: %w", err)
		}
		nebraskaCodebase = fmt.Sprintf("http://%s:%d/", testConfig.HostEndpointIP, testConfig.ImageServerPort)
		nebraskaPackageName = imageServer.PackageBaseName()
		nebraskaSHA384 = hash
	}

	nebraska := &stormproxies.NebraskaProxy{Scenario: &stormproxies.NebraskaScenario{
		Available:   true,
		Version:     testConfig.TargetVersion,
		URL:         nebraskaCodebase,
		SHA384:      nebraskaSHA384,
		PackageName: nebraskaPackageName,
	}}
	if _, err := nebraska.ListenAndServe(ctx, fmt.Sprintf("0.0.0.0:%d", testConfig.NebraskaPort)); err != nil {
		return fmt.Errorf("failed to start fake Nebraska endpoint: %w", err)
	}

	nodeStore.PatchLabels(map[string]string{stormproxies.NodeImageVersionLabel: testConfig.ExpectedInitialVolume})
	nodeStore.SetReadyCondition(true)

	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig); err != nil {
		return err
	}

	rp := &stormproxies.RPClient{APIServerURL: fmt.Sprintf("http://%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort), NodeName: testConfig.NodeName}
	scenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Patch: &stormproxies.PatchStep{NodeUpdateID: "11111111-1111-1111-1111-111111111111", OperationID: "stage-op", Operation: "stage", TargetOSImageVersion: testConfig.TargetVersion}},
		{Expect: &stormproxies.ExpectStep{OperationID: "stage-op", Operation: "stage", Code: "Success", Timeout: 120 * time.Second}},
		{Patch: &stormproxies.PatchStep{NodeUpdateID: "11111111-1111-1111-1111-111111111111", OperationID: "finalize-op", Operation: "finalize", TargetOSImageVersion: testConfig.TargetVersion}},
	}}
	report, err := rp.RunScenario(ctx, scenario)
	logScenarioTimeline("stage/finalize", report)
	if err != nil {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("ACL agent scenario failed (stage/finalize): %w", err)
	}
	if !report.Passed {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("ACL agent scenario failed (stage/finalize): %+v", report)
	}

	// Finalize triggers a real "systemctl reboot" from trident-acl-agent
	// itself. Reflect the VM actually going away/coming back in the fake
	// Node's Ready condition, then wait for SSH to come back before
	// checking the agent's post-reboot commit.
	nodeStore.SetReadyCondition(false)
	if err := waitForVmRebootAndSshBack(vmConfig, vmIP, testConfig); err != nil {
		return fmt.Errorf("failed waiting for VM to come back after finalize reboot: %w", err)
	}
	nodeStore.SetReadyCondition(true)

	// The real A/B reboot lands on a different root filesystem than the
	// one prepareVmForAclAgent originally configured: /etc/trident and
	// /var/lib/kubelet are per-root ext4 partitions, not shared storage,
	// so the config/kubeconfig written before staging do not carry over
	// to the newly-activated root. trident-acl-agent.service is enabled
	// by default there (baked into the update image), but it needs its
	// config re-delivered before it can talk to the fake Nebraska/API
	// server endpoints. Re-run the same delivery+restart steps now that
	// we're SSH'd into the post-reboot root.
	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig); err != nil {
		return fmt.Errorf("failed to reconfigure ACL agent on post-reboot root: %w", err)
	}

	finalScenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Expect: &stormproxies.ExpectStep{OperationID: "finalize-op.commit", Operation: "commit", Code: "Success", Timeout: 180 * time.Second}},
	}}
	finalReport, err := rp.RunScenario(ctx, finalScenario)
	logScenarioTimeline("post-reboot commit", finalReport)
	if err != nil {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("ACL agent scenario failed (post-reboot commit): %w", err)
	}
	if !finalReport.Passed {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("ACL agent scenario failed (post-reboot commit): %+v", finalReport)
	}

	snapshot := nodeStore.Snapshot()
	if got := snapshot.Annotations[stormproxies.UpdateStatusAnnotation]; got == "" {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("final status annotation missing")
	}
	return collectAclArtifacts(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
}

// collectAclArtifactsBestEffort collects the same diagnostic artifacts as
// collectAclArtifacts, but on a failure path where the harness is about to
// return an error anyway. run-ab-update's failure is otherwise a dead end
// for diagnostics: the storm-trident test runner marks collect-logs (and
// cleanup-vm) as NOTR ("dependency failure") whenever run-ab-update fails,
// so nothing ever calls collectAclArtifacts and the post-reboot journal
// (trident-acl-agent.log / tridentd.log) that would explain the failure is
// never captured or published as a pipeline artifact. Errors here are
// logged but swallowed so they never mask the original failure.
func collectAclArtifactsBestEffort(cfg stormvmconfig.VMConfig, vmIP string, outputPath string) {
	if err := collectAclArtifacts(cfg, vmIP, outputPath); err != nil {
		logrus.Warnf("best-effort artifact collection after test failure also failed: %v", err)
	}
}

func prepareVmForAclAgent(cfg stormvmconfig.VMConfig, vmIP string, testConfig stormaclconfig.TestConfig) error {
	config := fmt.Sprintf(`[nebraska]
endpoint = "http://%s:%d"
app_id = "trident-acl-agent-storm-test"
poll_interval = "5m"

[kubernetes]
api_server = "http://%s:%d"
kubeconfig = "/var/lib/kubelet/kubeconfig"
node_name = "%s"

[trident]
socket = "unix:///run/trident/trident.sock"

[orchestration]
goal_source = "labels"
`, testConfig.HostEndpointIP, testConfig.NebraskaPort, testConfig.HostEndpointIP, testConfig.APIServerPort, testConfig.NodeName)

	// Write the config to a local temp file and scp it up rather than
	// piping it through an SSH heredoc: heredocs are fragile to compose
	// with trailing shell operators (a bare "&&" right after the closing
	// delimiter is a syntax error), whereas scp-then-move is a plain file
	// transfer with no quoting/escaping pitfalls.
	localConfigFile, err := os.CreateTemp("", "trident-acl-agent-*.conf")
	if err != nil {
		return fmt.Errorf("failed to create local temp file for ACL agent config: %w", err)
	}
	defer os.Remove(localConfigFile.Name())
	if _, err := localConfigFile.WriteString(config); err != nil {
		localConfigFile.Close()
		return fmt.Errorf("failed to write local temp ACL agent config: %w", err)
	}
	if err := localConfigFile.Close(); err != nil {
		return fmt.Errorf("failed to close local temp ACL agent config: %w", err)
	}

	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, "sudo mkdir -p /etc/trident"); err != nil {
		return fmt.Errorf("failed to create /etc/trident on VM: %w", err)
	}
	if err := stormssh.ScpUploadFileWithSudo(cfg, vmIP, localConfigFile.Name(), "/etc/trident/trident-acl-agent.conf"); err != nil {
		return fmt.Errorf("failed to upload ACL agent config to VM: %w", err)
	}

	// The fake apiserver has no real kubelet-managed kubeconfig backing it,
	// and it takes plain HTTP with no auth/TLS, so provide a minimal
	// insecure kubeconfig pointing at it instead of relying on a real
	// kubelet bootstrap file that doesn't exist on this test image.
	kubeconfig := fmt.Sprintf(`apiVersion: v1
kind: Config
clusters:
- name: fake
  cluster:
    server: http://%s:%d
    insecure-skip-tls-verify: true
contexts:
- name: fake
  context:
    cluster: fake
    user: fake
current-context: fake
users:
- name: fake
  user: {}
`, testConfig.HostEndpointIP, testConfig.APIServerPort)

	localKubeconfigFile, err := os.CreateTemp("", "trident-acl-agent-kubeconfig-*.yaml")
	if err != nil {
		return fmt.Errorf("failed to create local temp file for fake kubeconfig: %w", err)
	}
	defer os.Remove(localKubeconfigFile.Name())
	if _, err := localKubeconfigFile.WriteString(kubeconfig); err != nil {
		localKubeconfigFile.Close()
		return fmt.Errorf("failed to write local temp fake kubeconfig: %w", err)
	}
	if err := localKubeconfigFile.Close(); err != nil {
		return fmt.Errorf("failed to close local temp fake kubeconfig: %w", err)
	}
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, "sudo mkdir -p /var/lib/kubelet"); err != nil {
		return fmt.Errorf("failed to create /var/lib/kubelet on VM: %w", err)
	}
	if err := stormssh.ScpUploadFileWithSudo(cfg, vmIP, localKubeconfigFile.Name(), "/var/lib/kubelet/kubeconfig"); err != nil {
		return fmt.Errorf("failed to upload fake kubeconfig to VM: %w", err)
	}

	// Enable (without "--now"), then always issue a single "restart" of
	// trident-acl-agent.service. Each RunX test case starts its own fresh
	// fake-apiserver/Nebraska instances, so a plain "enable --now" isn't
	// enough: it's a no-op restart-wise if the service is already active
	// (e.g. run-rollback runs right after run-ab-update, no reboot in
	// between), leaving the agent's watch connected to the prior test
	// case's now-torn-down apiserver, silently missing the new one and
	// timing out. A single unconditional "restart" fixes that by both
	// starting the unit if needed and cleanly restarting it if already
	// running. Do NOT combine "enable --now" with a separate "restart"
	// call: on the post-reboot path the unit auto-starts at boot and can
	// already be mid-commit (calling tridentd) by the time these SSH
	// commands run, so a second restart right after "--now" started it can
	// kill and restart the agent mid-call, and the new instance's retry
	// then fails with tridentd's "Servicing is active" error.
	command := strings.Join([]string{
		"sudo systemctl restart tridentd.service",
		"sudo systemctl enable trident-acl-agent.service",
		"sudo systemctl restart trident-acl-agent.service",
		fmt.Sprintf("sudo grep -q '%s:%d' /etc/trident/trident-acl-agent.conf", testConfig.HostEndpointIP, testConfig.APIServerPort),
		fmt.Sprintf("sudo grep -q '%s:%d' /etc/trident/trident-acl-agent.conf", testConfig.HostEndpointIP, testConfig.NebraskaPort),
	}, " && ")
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, command); err != nil {
		return fmt.Errorf("failed to prepare VM ACL agent config: %w", err)
	}

	// Both services can briefly report "activating" right after
	// "enable --now" before settling into "active" -- poll rather than
	// checking is-active exactly once.
	for _, svc := range []string{"tridentd.service", "trident-acl-agent.service"} {
		if err := waitForServiceActive(cfg, vmIP, svc, 30*time.Second); err != nil {
			return fmt.Errorf("failed waiting for %s to become active: %w", svc, err)
		}
	}
	return nil
}

func waitForServiceActive(cfg stormvmconfig.VMConfig, vmIP, service string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	var lastErr error
	for time.Now().Before(deadline) {
		out, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo systemctl is-active %s", service))
		if err == nil && strings.TrimSpace(out) == "active" {
			return nil
		}
		lastErr = err
		time.Sleep(2 * time.Second)
	}

	// Pull the service's journal so a timeout is self-diagnosing even when
	// the scenario fails before the dedicated collect-logs test case runs.
	journal, journalErr := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo journalctl -u %s --no-pager -n 200", service))
	if journalErr != nil {
		journal = fmt.Sprintf("<failed to collect journal: %v>", journalErr)
	}
	return fmt.Errorf("service %s did not become active within %s (last error: %v)\njournal for %s:\n%s", service, timeout, lastErr, service, journal)
}

// waitForVmRebootAndSshBack polls SSH until it is unreachable (confirming
// the agent's real "systemctl reboot" actually took the VM down) and then
// reachable again (confirming it came back up), mirroring the real-reboot
// wait pattern already used by the storm servicing scenario.
func waitForVmRebootAndSshBack(vmConfig stormvmconfig.AllVMConfig, vmIP string, testConfig stormaclconfig.TestConfig) error {
	downTimeout := time.Now().Add(60 * time.Second)
	for time.Now().Before(downTimeout) {
		if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, "true"); err != nil {
			break
		}
		time.Sleep(2 * time.Second)
	}

	upTimeout := time.Now().Add(5 * time.Minute)
	for time.Now().Before(upTimeout) {
		if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, "true"); err == nil {
			return nil
		}
		time.Sleep(2 * time.Second)
	}
	return fmt.Errorf("VM did not come back up over SSH within timeout after finalize reboot")
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
