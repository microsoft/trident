package scenario

import (
	"fmt"
	"os"
	"path/filepath"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
)

const (
	testingUsername = "testing-user"
)

func (s *TridentE2EScenario) prepareHostConfig(tc storm.TestCase) error {
	// Generate an SSH key pair for VM access, store the private key for later use
	private, public, err := sshutils.GenerateRsaKeyPair(2048)
	if err != nil {
		return fmt.Errorf("failed to generate RSA key pair for e2e: %w", err)
	}
	s.sshPrivateKey = private

	// Dump the private key to a file if requested
	if s.args.DumpSshKeyFile != "" {
		err := os.MkdirAll(filepath.Dir(s.args.DumpSshKeyFile), 0755)
		if err != nil {
			return fmt.Errorf("failed to create directory for SSH key file: %w", err)
		}

		err = os.WriteFile(s.args.DumpSshKeyFile, private, 0600)
		if err != nil {
			return fmt.Errorf("failed to write SSH private key to file %s: %w", s.args.DumpSshKeyFile, err)
		}
	}

	// Add the public key to the testing user
	found := false
	for _, user := range s.config.S("os", "users").Children() {
		name, ok := user.S("name").Data().(string)
		if !ok {
			continue
		}
		if name == testingUsername {
			user.ArrayAppend(string(public), "sshPublicKeys")
			found = true
		}
	}

	if !found {
		s.config.ArrayConcat(map[string]interface{}{
			"name":          testingUsername,
			"sshPublicKeys": []string{string(public)},
		}, "os", "users")
	}

	// If this is a container runtime, add the trident-container.tar.gz file to additional files.
	if s.runtime == trident.RuntimeTypeContainer {
		containerAdditionalFile := map[string]string{
			"source":      "/var/lib/trident/trident-container.tar.gz",
			"destination": "/var/lib/trident/trident-container.tar.gz",
		}
		s.config.ArrayAppend(containerAdditionalFile, "os", "additionalFiles")
	}

	// Inject any pipeline-provided OCI overrides (extension images, ACR-hosted
	// COSI URL). Mirrors tests/e2e_tests/helpers/edit_host_config.py.
	s.applyOciOverrides()

	return nil
}

// applyOciOverrides injects the OCI-based Host Configuration edits requested via
// scenario arguments: system/configuration extension images (os.sysexts /
// os.confexts) and an override of the COSI image URL (image.url). Each edit is
// applied only when its argument is provided. Ports the OCI handling of
// edit_host_config.py used by the pipeline's trident-prep step.
func (s *TridentE2EScenario) applyOciOverrides() {
	if s.args.SysextOciUrl != "" {
		s.config.ArrayAppend(map[string]interface{}{
			"url":    s.args.SysextOciUrl,
			"sha384": s.args.SysextSha384,
		}, "os", "sysexts")
	}

	if s.args.ConfextOciUrl != "" {
		s.config.ArrayAppend(map[string]interface{}{
			"url":    s.args.ConfextOciUrl,
			"sha384": s.args.ConfextSha384,
		}, "os", "confexts")
	}

	if s.args.OciImageUrl != "" {
		s.config.Set(s.args.OciImageUrl, "image", "url")
	}
}
