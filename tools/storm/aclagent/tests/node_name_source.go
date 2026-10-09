package tests

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"math/big"
	"os"
	"path/filepath"
	"strings"
	"time"

	stormproxies "tridenttools/storm/aclagent/proxies"
	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormssh "tridenttools/storm/utils/ssh"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

// kubeletClientCertPath mirrors trident-acl-agent's own
// DEFAULT_KUBELET_CLIENT_CERT (crates/trident-acl-agent/src/core/config.rs).
const kubeletClientCertPath = "/var/lib/kubelet/pki/kubelet-client-current.pem"

// certDerivedNodeName is the Node name this test's fake kubelet client cert
// claims via its Subject CN (system:node:<name>). Deliberately distinct from
// both the VM's real hostname and explicitOverrideNodeName below, so each
// phase's assertion is a genuine positive match on a specific value, never
// a coincidental overlap between two phases.
const certDerivedNodeName = "node-name-source-test-cert-node"

// explicitOverrideNodeName is a third literal, distinct from both
// certDerivedNodeName and the VM's real hostname, used only for the
// explicit TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME override check - so that
// check's success can only be explained by the override actually being
// honored, never by it happening to match another phase's expected value.
const explicitOverrideNodeName = "node-name-source-hostname-override-node"

// RunNodeNameSource proves TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE
// (microsoft/trident#839) actually switches which identity
// trident-acl-agent resolves node_name from, entirely through one-shot
// `trident-acl-agent --validate-connection kubernetes` invocations - no
// service restart or reboot needed, since validate-connection is a
// standalone CLI invocation of the same binary, not a call into the
// long-running trident-acl-agent.service. That keeps this test fast
// relative to the rest of this VM-based scenario.
//
// The fake apiserver (see proxies.NewAPIServer) only ever serves a single
// Node name - whatever its own nodeName argument is - so this test runs
// three sequential phases, each with its own single-node apiserver
// instance seeded with the one specific identity that phase expects to
// resolve to. Every check then asserts the *exact* Node name
// validate-connection's own success output reports fetching, not just
// pass/fail - so a phase can only pass by actually resolving to the value
// it claims, never by vacuously matching a different phase's answer.
func RunNodeNameSource(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
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

	// Delivers the fake kubeconfig pointing trident-acl-agent's compiled-in
	// default kubeconfig path at HostEndpointIP:APIServerPort. That address
	// stays constant across every phase below - only which single Node the
	// fake apiserver bound there currently serves changes - so this only
	// needs to happen once, up front. trident-acl-agent.service itself is
	// never touched by this test, since every check is a direct one-shot
	// CLI invocation.
	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig, nil); err != nil {
		return err
	}

	// --- Phase 1: kubelet-cert source resolves to the cert's Subject CN ---
	if err := withNodeNameSourceApiServer(ctx, testConfig, certDerivedNodeName, func() error {
		certPEM, err := generateFakeKubeletClientCertPEM(certDerivedNodeName)
		if err != nil {
			return fmt.Errorf("failed to generate fake kubelet client cert: %w", err)
		}
		if err := uploadKubeletClientCert(vmConfig.VMConfig, vmIP, certPEM); err != nil {
			return err
		}
		return expectValidateConnectionOutput(vmConfig.VMConfig, vmIP, map[string]string{
			"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "kubelet-cert",
		}, fmt.Sprintf("fetched Node %q", certDerivedNodeName))
	}); err != nil {
		return fmt.Errorf("phase kubelet-cert: %w", err)
	}

	// --- Phase 2: hostname source (explicit and default) resolves to the
	//     VM's own real hostname, and a missing/unreadable cert degrades
	//     kubelet-cert source to that exact same resolution ---
	realHostname, err := readVmHostname(vmConfig.VMConfig, vmIP)
	if err != nil {
		return err
	}
	if err := withNodeNameSourceApiServer(ctx, testConfig, realHostname, func() error {
		if err := expectValidateConnectionOutput(vmConfig.VMConfig, vmIP, map[string]string{
			"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "hostname",
		}, fmt.Sprintf("fetched Node %q", realHostname)); err != nil {
			return fmt.Errorf("explicit hostname source: %w", err)
		}
		if err := expectValidateConnectionOutput(vmConfig.VMConfig, vmIP, nil,
			fmt.Sprintf("fetched Node %q", realHostname)); err != nil {
			return fmt.Errorf("default (unset NODE_NAME_SOURCE) source: %w", err)
		}

		// An unreadable/missing cert must make kubelet-cert source degrade
		// to the hostname behavior (logging a warning), not error out
		// outright - confirmed by asserting the very same successful
		// resolution to realHostname as above, plus the fallback warning
		// actually being logged (trident-acl-agent's default --verbosity
		// is Debug, so a Warn-level log is never filtered out here).
		if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo mv %s %s.bak", kubeletClientCertPath, kubeletClientCertPath)); err != nil {
			return fmt.Errorf("failed to hide kubelet client cert: %w", err)
		}
		defer func() {
			if err := restoreKubeletClientCert(vmConfig.VMConfig, vmIP); err != nil {
				fmt.Fprintf(os.Stderr, "run-node-name-source: %v\n", err)
			}
		}()
		return expectValidateConnectionOutput(vmConfig.VMConfig, vmIP, map[string]string{
			"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "kubelet-cert",
		}, "falling back to hostname", fmt.Sprintf("fetched Node %q", realHostname))
	}); err != nil {
		return fmt.Errorf("phase hostname/missing-cert-fallback: %w", err)
	}

	// --- Phase 3: an explicit TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME must
	//     win outright regardless of NODE_NAME_SOURCE. Uses a third literal
	//     distinct from both phases above, so success here can only be
	//     explained by the override itself, not by coincidentally matching
	//     the cert-derived name or the real hostname. ---
	if err := withNodeNameSourceApiServer(ctx, testConfig, explicitOverrideNodeName, func() error {
		return expectValidateConnectionOutput(vmConfig.VMConfig, vmIP, map[string]string{
			"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME":        explicitOverrideNodeName,
			"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "hostname",
		}, fmt.Sprintf("fetched Node %q", explicitOverrideNodeName))
	}); err != nil {
		return fmt.Errorf("phase explicit node_name override: %w", err)
	}

	return collectAclArtifacts(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
}

// withNodeNameSourceApiServer starts a fresh single-node fake apiserver
// bound to testConfig.HostEndpointIP:APIServerPort, seeded with exactly one
// Node named nodeName, runs body, then synchronously stops that apiserver
// before returning - so each of RunNodeNameSource's phases gets a clean
// apiserver instance serving only the one identity that phase expects,
// with no overlap or leftover state from a previous phase, and the next
// phase (or the next test case entirely) never races this one's teardown
// for the same bind address.
func withNodeNameSourceApiServer(ctx context.Context, testConfig stormaclconfig.TestConfig, nodeName string, body func() error) error {
	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(nodeName, map[string]string{}))
	apiServer := stormproxies.NewAPIServer(nodeName, nodeStore)
	_, stop, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort))
	if err != nil {
		return fmt.Errorf("failed to start fake apiserver for node %q: %w", nodeName, err)
	}
	defer stop()
	nodeStore.SetReadyCondition(true)
	return body()
}

// readVmHostname reads the VM's real hostname (the same OS-level value
// trident-acl-agent's own hostname-source fallback reads via hostname::get()
// - see hostname_node_name() in crates/trident-acl-agent/src/core/
// config.rs), lowercased the same way that function lowercases it before
// using it as a Kubernetes Node name.
func readVmHostname(cfg stormvmconfig.VMConfig, vmIP string) (string, error) {
	out, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, "hostname")
	if err != nil {
		return "", fmt.Errorf("failed to read VM hostname: %w", err)
	}
	return strings.ToLower(strings.TrimSpace(out)), nil
}

// uploadKubeletClientCert writes certPEM to kubeletClientCertPath on the VM,
// mirroring how prepareVmForAclAgent delivers the fake kubeconfig.
func uploadKubeletClientCert(cfg stormvmconfig.VMConfig, vmIP string, certPEM []byte) error {
	localCertFile, err := os.CreateTemp("", "trident-acl-agent-kubelet-client-cert-*.pem")
	if err != nil {
		return fmt.Errorf("failed to create local temp file for fake kubelet client cert: %w", err)
	}
	defer os.Remove(localCertFile.Name())
	if _, err := localCertFile.Write(certPEM); err != nil {
		localCertFile.Close()
		return fmt.Errorf("failed to write local temp fake kubelet client cert: %w", err)
	}
	if err := localCertFile.Close(); err != nil {
		return fmt.Errorf("failed to close local temp fake kubelet client cert: %w", err)
	}
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo mkdir -p %s", filepath.Dir(kubeletClientCertPath))); err != nil {
		return fmt.Errorf("failed to create kubelet pki directory on VM: %w", err)
	}
	if err := stormssh.ScpUploadFileWithSudo(cfg, vmIP, localCertFile.Name(), kubeletClientCertPath); err != nil {
		return fmt.Errorf("failed to upload fake kubelet client cert to VM: %w", err)
	}
	return nil
}

// restoreKubeletClientCert undoes the "hide the cert" step in phase 2
// above, so this test case leaves the VM in the same state it found it in
// (a valid cert present) regardless of pass/fail, for anyone re-running
// just this test case in isolation.
func restoreKubeletClientCert(cfg stormvmconfig.VMConfig, vmIP string) error {
	if _, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, fmt.Sprintf("sudo mv %s.bak %s", kubeletClientCertPath, kubeletClientCertPath)); err != nil {
		return fmt.Errorf("failed to restore kubelet client cert: %w", err)
	}
	return nil
}

// generateFakeKubeletClientCertPEM builds a minimal self-signed certificate
// shaped like a real AKS kubelet client cert: Subject CN
// "system:node:<nodeName>", Organization "system:nodes" - the only two
// fields node_name_from_kubelet_cert (crates/trident-acl-agent/src/core/
// config.rs) actually reads.
func generateFakeKubeletClientCertPEM(nodeName string) ([]byte, error) {
	priv, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return nil, fmt.Errorf("failed to generate test key: %w", err)
	}
	template := x509.Certificate{
		SerialNumber: big.NewInt(1),
		Subject: pkix.Name{
			CommonName:   "system:node:" + nodeName,
			Organization: []string{"system:nodes"},
		},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(10 * 365 * 24 * time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth},
		BasicConstraintsValid: true,
	}
	der, err := x509.CreateCertificate(rand.Reader, &template, &template, &priv.PublicKey, priv)
	if err != nil {
		return nil, fmt.Errorf("failed to create test certificate: %w", err)
	}
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), nil
}

// expectValidateConnectionOutput runs `trident-acl-agent --validate-connection
// kubernetes` with envVars, asserts it succeeds, and asserts its combined
// output contains every string in wantSubstrings - used wherever this test
// needs to confirm not just that resolution succeeded, but which specific
// Node name it resolved to (e.g. "fetched Node \"<name>\"", the exact
// phrase connection_check.rs's validate_connection logs on success).
func expectValidateConnectionOutput(cfg stormvmconfig.VMConfig, vmIP string, envVars map[string]string, wantSubstrings ...string) error {
	out, err := runValidateConnectionKubernetes(cfg, vmIP, envVars)
	if err != nil {
		return fmt.Errorf("expected success (envVars=%v): %w", envVars, err)
	}
	for _, want := range wantSubstrings {
		if !strings.Contains(out, want) {
			return fmt.Errorf("expected output to contain %q (envVars=%v), got: %s", want, envVars, out)
		}
	}
	return nil
}

// runValidateConnectionKubernetes runs `trident-acl-agent --validate-connection
// kubernetes` with envVars and returns its combined output. Note:
// stormssh.SshCommandCombinedOutput (see ssh.go's innerSshCommand) discards
// the captured remote output on a non-zero exit, returning ("", err)
// instead - the real stdout/stderr only survives inside the wrapped
// error's own message ("...\nOutput: ..."). Callers that need to inspect
// output from an *expected failure* must check err.Error(), not the first
// return value.
func runValidateConnectionKubernetes(cfg stormvmconfig.VMConfig, vmIP string, envVars map[string]string) (string, error) {
	var prefix strings.Builder
	prefix.WriteString("sudo")
	if len(envVars) > 0 {
		prefix.WriteString(" env")
		for k, v := range envVars {
			fmt.Fprintf(&prefix, " %s=%q", k, v)
		}
	}
	command := fmt.Sprintf("%s trident-acl-agent --validate-connection kubernetes", prefix.String())
	return stormssh.SshCommandCombinedOutput(cfg, vmIP, command)
}
