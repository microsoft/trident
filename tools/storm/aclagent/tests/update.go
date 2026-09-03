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

// sha384File computes the lowercase hex-encoded SHA-384 digest that tridentd
// expects for a given image path.
//
// For a .cosi file, tridentd does NOT hash the whole archive: a COSI is a
// plain tar with an embedded "metadata.json" entry, and tridentd's Host
// Configuration "sha384" field must match the hash of just that entry's
// bytes (see crates/trident/src/osimage/cosi/mod.rs's read_cosi_metadata).
// For any other file, this hashes the whole file's contents directly.
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

// expectValidateConnection runs "trident-acl-agent --validate-connection
// <mode>" on the VM and asserts its exit status matches wantSuccess. All
// configuration is via TRIDENT_ACL_AGENT_* environment variables (see
// crates/trident-acl-agent/src/config.rs) - there is no config file - so
// envVars lets a caller inject one-off overrides for this single
// invocation only (nil/empty runs against whatever the agent's own
// environment already has, i.e. nothing, proving the compiled-in
// defaults). Used to prove both halves of the kubeconfig-server fix
// (https://github.com/microsoft/trident/pull/730): before the fake
// kubeconfig is delivered, only tridentd (socket-activated, no config
// needed) should succeed while kubernetes/nebraska fail on unreachable
// defaults; after prepareVmForAclAgent delivers it, kubernetes succeeds
// too. Nebraska has no static config at all, so proving it can succeed
// requires passing envVars explicitly (see the env-var-override checks in
// RunABUpdate).
func expectValidateConnection(cfg stormvmconfig.VMConfig, vmIP, mode string, wantSuccess bool, envVars map[string]string) error {
	var prefix strings.Builder
	prefix.WriteString("sudo")
	if len(envVars) > 0 {
		prefix.WriteString(" env")
		for k, v := range envVars {
			fmt.Fprintf(&prefix, " %s=%q", k, v)
		}
	}
	command := fmt.Sprintf("%s trident-acl-agent --validate-connection %s", prefix.String(), mode)
	out, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, command)
	gotSuccess := err == nil
	if gotSuccess != wantSuccess {
		return fmt.Errorf("--validate-connection %s (envVars=%v): expected success=%v, got success=%v (err=%v, output=%s)", mode, envVars, wantSuccess, gotSuccess, err, out)
	}
	return nil
}

func RunABUpdate(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
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

	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(testConfig.NodeName, map[string]string{}))
	apiServer := stormproxies.NewAPIServer(testConfig.NodeName, nodeStore)
	// Bind on HostEndpointIP (not 127.0.0.1) so the VM can reach the fake
	// apiserver directly over the libvirt NAT network, instead of relying
	// on reverse SSH tunnels (which don't survive a real VM reboot; a real
	// host IP does). Binding to that specific address rather than 0.0.0.0
	// avoids unintentionally exposing the fake server on every other host
	// interface too.
	//
	// The bind itself is deliberately deferred (see apiServerDelayedStart
	// below) until ~10s after trident-acl-agent.service has been told to
	// (re)start, so the agent's connect-retry/backoff logic has to actually
	// retry against a closed port for a while instead of always finding the
	// apiserver already up.
	apiServerReady := make(chan error, 1)
	apiServerDelayedStart := func() {
		_, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort))
		apiServerReady <- err
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
		if _, err := imageServer.ListenAndServe(ctx, fmt.Sprintf("%s:%d", testConfig.HostEndpointIP, testConfig.ImageServerPort)); err != nil {
			return fmt.Errorf("failed to start fake image server: %w", err)
		}
		nebraskaCodebase = fmt.Sprintf("http://%s:%d/", testConfig.HostEndpointIP, testConfig.ImageServerPort)
		nebraskaPackageName = imageServer.PackageBaseName()
		nebraskaSHA384 = hash
	}

	nebraska := &stormproxies.NebraskaProxy{
		Scenario: &stormproxies.NebraskaScenario{
			Available:   true,
			Version:     testConfig.TargetVersion,
			URL:         nebraskaCodebase,
			SHA384:      nebraskaSHA384,
			PackageName: nebraskaPackageName,
		},
		PostgresImage: testConfig.PostgresImage,
	}
	if _, err := nebraska.ListenAndServe(ctx, fmt.Sprintf("%s:%d", testConfig.HostEndpointIP, testConfig.NebraskaPort)); err != nil {
		return fmt.Errorf("failed to start fake Nebraska endpoint: %w", err)
	}

	nodeStore.PatchLabels(map[string]string{stormproxies.NodeImageVersionLabel: testConfig.ExpectedInitialVolume})
	nodeStore.SetReadyCondition(true)

	// Before the fake kubeconfig has been delivered to the VM, trident-acl-agent
	// falls back to its compiled-in defaults for everything: tridentd's
	// socket is reached via systemd socket activation regardless of any
	// config, so it should succeed; kubernetes and nebraska both default to
	// unreachable placeholders (no real kubeconfig at
	// /var/lib/kubelet/kubeconfig yet, and https://nebraska.example.invalid
	// / no app_id/server override in play), so both should fail.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "tridentd", true, nil); err != nil {
		return fmt.Errorf("pre-config validate-connection check failed: %w", err)
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", false, nil); err != nil {
		return fmt.Errorf("pre-config validate-connection check failed: %w", err)
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "nebraska", false, nil); err != nil {
		return fmt.Errorf("pre-config validate-connection check failed: %w", err)
	}

	// Start the ~10s countdown right as we hand off to prepareVmForAclAgent,
	// which is what actually delivers the kubeconfig and issues `systemctl
	// restart trident-acl-agent.service`. Exact timing off the real restart
	// isn't important - roughly 10s of the service being up against a
	// closed apiserver port is good enough to exercise the retry/backoff
	// path.
	time.AfterFunc(10*time.Second, apiServerDelayedStart)

	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig); err != nil {
		return err
	}

	// Once prepareVmForAclAgent has delivered the fake kubeconfig, kubernetes
	// now succeeds (its own `server:` field points at the fake apiserver);
	// tridentd is unaffected either way. trident-acl-agent never gets a
	// config file at all (see prepareVmForAclAgent's doc comment) - Nebraska
	// endpoint/app_id/track are only ever supplied per-request, either via
	// the update-request annotation's `server`/`appId`/`track` fields or
	// (for this one-off diagnostic check) a TRIDENT_ACL_AGENT_NEBRASKA_*
	// env var - so a bare "--validate-connection nebraska" with neither in
	// play still fails here exactly as it did before configuration, proving
	// there is no static Nebraska configuration of any kind for the agent
	// to fall back on.
	for _, mode := range []string{"tridentd", "kubernetes"} {
		if mode == "kubernetes" {
			// This is the first thing in the test that actually needs the
			// (deliberately delayed) fake apiserver to be reachable - block
			// here instead of racing it, both so this one-shot check isn't
			// spuriously flaky and so the stage/finalize scenario below
			// never starts patching the fake Node before the apiserver
			// exists at all (that HTTP client has no retry of its own).
			if err := <-apiServerReady; err != nil {
				return fmt.Errorf("failed to start fake apiserver: %w", err)
			}
		}
		if err := expectValidateConnection(vmConfig.VMConfig, vmIP, mode, true, nil); err != nil {
			return fmt.Errorf("post-config validate-connection check failed: %w", err)
		}
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "nebraska", false, nil); err != nil {
		return fmt.Errorf("post-config validate-connection check failed (trident-acl-agent has no static Nebraska config): %w", err)
	}

	rp := &stormproxies.RPClient{APIServerURL: fmt.Sprintf("http://%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort), NodeName: testConfig.NodeName}
	nebraskaServer := fmt.Sprintf("http://%s:%d", testConfig.HostEndpointIP, testConfig.NebraskaPort)
	nebraskaAppID := nebraska.AppID()
	nebraskaTrack := nebraska.Track()

	// Proves the env-var override path itself: the same one-off diagnostic
	// check that just failed with no override now succeeds when given the
	// exact TRIDENT_ACL_AGENT_NEBRASKA_* values RunABUpdate is about to send
	// via the update-request annotation, and fails again with a deliberately
	// wrong endpoint - i.e. both the success and failure paths of the
	// env-var config mechanism are exercised directly, not just inferred
	// from the annotation-driven flow below.
	nebraskaEnv := map[string]string{
		"TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT": nebraskaServer,
		"TRIDENT_ACL_AGENT_NEBRASKA_APP_ID":   nebraskaAppID,
		"TRIDENT_ACL_AGENT_NEBRASKA_TRACK":    nebraskaTrack,
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "nebraska", true, nebraskaEnv); err != nil {
		return fmt.Errorf("env-var-override validate-connection check failed (expected success): %w", err)
	}
	wrongNebraskaEnv := map[string]string{
		"TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT": fmt.Sprintf("http://%s:%d", testConfig.HostEndpointIP, testConfig.NebraskaPort+1),
		"TRIDENT_ACL_AGENT_NEBRASKA_APP_ID":   nebraskaAppID,
		"TRIDENT_ACL_AGENT_NEBRASKA_TRACK":    nebraskaTrack,
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "nebraska", false, wrongNebraskaEnv); err != nil {
		return fmt.Errorf("env-var-override validate-connection check failed (expected failure against wrong port): %w", err)
	}

	scenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Patch: &stormproxies.PatchStep{NodeUpdateID: "11111111-1111-1111-1111-111111111111", OperationID: "aaaaaaaa-1111-1111-1111-111111111111", Operation: "stage", TargetOSImageVersion: testConfig.TargetVersion, Server: nebraskaServer, AppId: nebraskaAppID, Track: nebraskaTrack}},
		{Expect: &stormproxies.ExpectStep{OperationID: "aaaaaaaa-1111-1111-1111-111111111111", Operation: "stage", Code: "Success", Timeout: 120 * time.Second}},
		{Patch: &stormproxies.PatchStep{NodeUpdateID: "11111111-1111-1111-1111-111111111111", OperationID: "bbbbbbbb-2222-2222-2222-222222222222", Operation: "finalize", TargetOSImageVersion: testConfig.TargetVersion, Server: nebraskaServer, AppId: nebraskaAppID, Track: nebraskaTrack}},
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

	// /etc/trident and /var/lib/kubelet are their own dedicated ext4
	// partitions (not part of the A/B-swapped root), so the config and
	// kubeconfig prepareVmForAclAgent delivered before staging carry over
	// unchanged to the newly-activated root. trident-acl-agent.service is
	// enabled by default there (baked into the update image) and starts
	// with that same config at boot, so no re-delivery is needed here.

	finalScenario := &stormproxies.Scenario{Steps: []stormproxies.ScenarioStep{
		{Expect: &stormproxies.ExpectStep{OperationID: "bbbbbbbb-2222-2222-2222-222222222222", Operation: "commit", Code: "Success", Timeout: 180 * time.Second}},
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
	if got := snapshot.Annotations[stormproxies.UpdateCommitStatusAnnotation]; got == "" {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("final commit status annotation missing")
	}

	// The node annotation only proves the ACL-agent-facing rollout API
	// reported success; it says nothing about whether trident-acl-agent
	// actually drove Nebraska's own instance state machine correctly. Assert
	// that too, against the real Nebraska instance_status_history this
	// scenario's seeded application accumulated over stage/finalize/commit.
	if err := nebraska.ValidateStatusHistory(stormproxies.ExpectedUpdateStatusSequence); err != nil {
		collectAclArtifactsBestEffort(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
		return fmt.Errorf("ACL agent scenario failed Nebraska status validation: %w", err)
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
	// trident-acl-agent needs no config file at all for this scenario:
	//   - nebraska.app_id and nebraska.endpoint are supplied per-request via
	//     the update-request annotation's `appId`/`server` fields instead
	//     (see PatchStep.AppId/Server usage in RunABUpdate).
	//   - kubernetes.api_server is left unset (its own default): the fake
	//     kubeconfig written below already has `server:` pointing at the
	//     fake apiserver, so there is nothing to override.
	//   - kubernetes.node_name is also left unset (its own default: the
	//     node's real hostname, lowercased). testConfig.NodeName is set to
	//     match the VM image's Image Customizer 'hostname' setting
	//     (baseimg-acl-agent.yaml), so the agent's own hostname-derived
	//     default already agrees with the fake apiserver's seeded Node -
	//     no override needed, exactly like a real deployment.
	//   - kubernetes.kubeconfig and trident.socket are also already their
	//     compiled-in defaults.
	// trident-acl-agent.conf is simply never written to the VM.
	//
	// kubernetes.annotation_prefix and current_version.key DO differ from
	// their compiled-in defaults for this scenario: it pins its
	// expectations to the AKS-era values (annotation prefix
	// "acl.azure.com" - see proxies/constants.go - and current-version key
	// "IMAGE_VERSION"), which used to be trident-acl-agent's own compiled-in
	// defaults and so needed no override at all. Now that the defaults have
	// moved to the generic "acl.microsoft.com"/"VERSION_ID", both VM images
	// (baseimg-acl-agent.yaml and updateimg-acl-agent.yaml) bake in a
	// systemd drop-in setting them explicitly - not done here at runtime,
	// because a drop-in written under /etc/systemd/system after boot would
	// live only on the currently-active root and not survive this
	// usr-verity image's A/B root swap.

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
	//
	// tridentd.service is socket-activated (tridentd.socket): it doesn't
	// hold any per-test-case state that needs refreshing, so a "restart"
	// is unnecessary and, on the post-reboot path, risks killing an
	// in-progress gRPC call (e.g. a commit already underway) that a
	// trident-acl-agent instance auto-started at boot may be mid-flight
	// on. Use "start" instead: it's a no-op if tridentd is already
	// running (whether via socket activation or a prior start here), and
	// otherwise brings it up without disturbing anything already using it.
	command := strings.Join([]string{
		"sudo systemctl start tridentd.service",
		"sudo systemctl enable trident-acl-agent.service",
		"sudo systemctl restart trident-acl-agent.service",
		// api_server lives only in the fake kubeconfig now (agent config
		// never sets it - see this function's doc comment).
		fmt.Sprintf("sudo grep -qF '%s:%d' /var/lib/kubelet/kubeconfig", testConfig.HostEndpointIP, testConfig.APIServerPort),
		// Lock in the design invariant: trident-acl-agent.conf is never
		// written to the VM at all - app_id/endpoint are only ever supplied
		// per-request via the update-request annotation's `appId`/`server`
		// fields.
		"sudo test ! -e /etc/trident/trident-acl-agent.conf",
	}, " && ")
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, command); err != nil {
		return fmt.Errorf("failed to prepare VM for ACL agent: %w", err)
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
	wentDown := false
	downTimeout := time.Now().Add(60 * time.Second)
	for time.Now().Before(downTimeout) {
		if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, "true"); err != nil {
			wentDown = true
			break
		}
		time.Sleep(2 * time.Second)
	}
	if !wentDown {
		return fmt.Errorf("VM never became unreachable over SSH within %s; reboot did not appear to happen", 60*time.Second)
	}

	// A single successful SSH command right after boot is not proof the VM
	// is stably back up: sshd (or the network stack) can accept one
	// connection and then bounce again moments later while later boot
	// units are still settling (observed in practice as one successful
	// "true" immediately followed by "connection refused" on the very
	// next SSH dial). Require a few consecutive successes, spaced out,
	// before declaring the VM ready, so callers that immediately issue
	// real SSH commands (e.g. prepareVmForAclAgent) don't race a
	// still-settling boot.
	const requiredConsecutiveSuccesses = 3
	consecutiveSuccesses := 0
	upTimeout := time.Now().Add(5 * time.Minute)
	for time.Now().Before(upTimeout) {
		if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, "true"); err == nil {
			consecutiveSuccesses++
			if consecutiveSuccesses >= requiredConsecutiveSuccesses {
				return nil
			}
		} else {
			consecutiveSuccesses = 0
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
	}
	for _, cmd := range cmds {
		_, _ = stormssh.SshCommandCombinedOutput(cfg, vmIP, cmd)
	}
	for _, remote := range []string{"/tmp/trident-acl-agent.log", "/tmp/tridentd.log"} {
		if err := stormssh.ScpDownloadFile(cfg, vmIP, remote, filepath.Join(outputPath, filepath.Base(remote))); err != nil {
			logrus.Warnf("failed to download %s: %v", remote, err)
		}
	}
	return nil
}
