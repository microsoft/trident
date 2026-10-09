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
// claims via its Subject CN (system:node:<name>), deliberately different
// from testConfig.NodeName (the VM's real, lowercased hostname). The fake
// apiserver (see proxies.NewAPIServer) only ever serves a single Node name -
// whatever its own nodeName argument is - so seeding it with
// certDerivedNodeName instead of testConfig.NodeName makes the two
// node_name sources mutually exclusive: resolving to the wrong one always
// 404s against this fake, there is no overlap that could make this test
// pass vacuously.
const certDerivedNodeName = "node-name-source-test-cert-node"

// RunNodeNameSource proves TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE
// (microsoft/trident#839) actually switches which identity
// trident-acl-agent resolves node_name from, entirely through one-shot
// `trident-acl-agent --validate-connection kubernetes` invocations - no
// service restart or reboot needed, since validate-connection is a
// standalone CLI invocation of the same binary, not a call into the
// long-running trident-acl-agent.service. That keeps this test fast
// relative to the rest of this VM-based scenario.
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

	// Only the fake apiserver is needed - no Nebraska/image-server mock,
	// exactly like RunNodeResilience (see its own doc comment for why).
	nodeStore := stormproxies.NewNodeStore(stormproxies.NewSeedNode(certDerivedNodeName, map[string]string{}))
	apiServer := stormproxies.NewAPIServer(certDerivedNodeName, nodeStore)
	_, apiServerStop, err := apiServer.ListenAndServe(ctx, fmt.Sprintf("%s:%d", testConfig.HostEndpointIP, testConfig.APIServerPort))
	if err != nil {
		return fmt.Errorf("failed to start fake apiserver: %w", err)
	}
	// Deferred instead of relying on ctx cancellation alone - the next test
	// case binds the same HostEndpointIP:APIServerPort and must not race
	// this apiserver's teardown, same reasoning as RunABUpdate/
	// RunNodeResilience's own apiServerStop defer.
	defer apiServerStop()
	nodeStore.SetReadyCondition(true)

	// Only need the fake kubeconfig this writes, so the agent binary's own
	// compiled-in default kubeconfig path resolves to the fake apiserver
	// above - trident-acl-agent.service itself is never touched by this
	// test, since every check below is a direct one-shot CLI invocation.
	if err := prepareVmForAclAgent(vmConfig.VMConfig, vmIP, testConfig, nil); err != nil {
		return err
	}

	certPEM, err := generateFakeKubeletClientCertPEM(certDerivedNodeName)
	if err != nil {
		return fmt.Errorf("failed to generate fake kubelet client cert: %w", err)
	}
	if err := uploadKubeletClientCert(vmConfig.VMConfig, vmIP, certPEM); err != nil {
		return err
	}

	// 1. kubelet-cert source resolves node_name from the cert's Subject CN,
	//    which IS this fake apiserver's one known Node - must succeed.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", true, map[string]string{
		"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "kubelet-cert",
	}); err != nil {
		return fmt.Errorf("kubelet-cert source: %w", err)
	}

	// 2. hostname source (explicit, and separately the compiled-in default
	//    below) resolves to the VM's real hostname, which this fake
	//    apiserver does NOT serve - must fail. Proves the two sources are
	//    genuinely different code paths, not just hostname silently used
	//    both times.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", false, map[string]string{
		"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "hostname",
	}); err != nil {
		return fmt.Errorf("hostname source: %w", err)
	}
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", false, nil); err != nil {
		return fmt.Errorf("default (unset NODE_NAME_SOURCE) source: %w", err)
	}

	// 3. An explicit TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME must still win
	//    outright regardless of NODE_NAME_SOURCE - proves override
	//    precedence over source selection.
	if err := expectValidateConnection(vmConfig.VMConfig, vmIP, "kubernetes", true, map[string]string{
		"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME":        certDerivedNodeName,
		"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "hostname",
	}); err != nil {
		return fmt.Errorf("explicit node_name override: %w", err)
	}

	// 4. An unreadable/missing cert must make kubelet-cert source degrade
	//    to the hostname behavior (logging a warning), not error out
	//    outright - confirmed two ways: the CLI invocation fails with
	//    exactly the same (hostname-not-served) shape as case 2 above, and
	//    the fallback warning is actually logged (trident-acl-agent's
	//    default --verbosity is Debug, so a Warn-level log is never
	//    filtered out here).
	if _, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo mv %s %s.bak", kubeletClientCertPath, kubeletClientCertPath)); err != nil {
		return fmt.Errorf("failed to hide kubelet client cert: %w", err)
	}
	// Deferred (not a plain call at the end) so the cert is restored
	// regardless of which return path below is taken, leaving the VM in
	// the same state this test case found it in for anyone re-running
	// just this test case in isolation.
	defer func() {
		if err := restoreKubeletClientCert(vmConfig.VMConfig, vmIP); err != nil {
			fmt.Fprintf(os.Stderr, "run-node-name-source: %v\n", err)
		}
	}()

	out, err := runValidateConnectionKubernetes(vmConfig.VMConfig, vmIP, map[string]string{
		"TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE": "kubelet-cert",
	})
	if err == nil {
		return fmt.Errorf("kubelet-cert source with missing cert: expected failure (falls back to hostname, which this fake apiserver doesn't serve), got success")
	}
	if !strings.Contains(out, "falling back to hostname") {
		return fmt.Errorf("kubelet-cert source with missing cert: expected a 'falling back to hostname' warning in output, got: %s", out)
	}

	return collectAclArtifacts(vmConfig.VMConfig, vmIP, testConfig.OutputPath)
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

// restoreKubeletClientCert undoes the "hide the cert" step in case 4 above,
// so this test case leaves the VM in the same state it found it in (a
// valid cert present) regardless of pass/fail, for anyone re-running just
// this test case in isolation.
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

// runValidateConnectionKubernetes is like expectValidateConnection (see
// update.go) but returns the combined output too, for the one case in this
// file (kubelet-cert source with an unreadable cert) that needs to assert
// on a specific log message, not just the exit status.
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
