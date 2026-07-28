package scenario

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
)

const (
	testingUsername = "testing-user"

	// usrVerityCosiSuffix identifies the usr-verity UKI test image whose
	// encryption must not seal to PCR 7 under a containerized runtime.
	usrVerityCosiSuffix = "usrverity.cosi"
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

	// Strip PCR 7 from the encryption policy when required by the container
	// runtime, before any OCI image-URL override changes image.url.
	s.applyContainerPcrExclusion()

	// Inject any pipeline-provided OCI overrides (extension images, ACR-hosted
	// COSI URL). Mirrors tests/e2e_tests/helpers/edit_host_config.py.
	s.applyOciOverrides()

	return nil
}

// applyContainerPcrExclusion drops PCR 7 (secure-boot-policy) from the
// encryption policy for usr-verity UKI images running under a containerized
// Trident. Secure Boot measures the host boot chain, not the container's, so a
// policy sealed to PCR 7 could never reproduce; Trident rejects the
// combination during dynamic validation
// (crates/trident/src/subsystems/storage/encryption.rs). This mirrors the
// legacy pipeline glue added in #221
// (.pipelines/templates/stages/testing_vm/netlaunch-testing.yml), which
// rewrites the Host Configuration for exactly this case so combined/rerun
// (usr-verity UKI + encryption) install on the container runtime.
func (s *TridentE2EScenario) applyContainerPcrExclusion() {
	if s.runtime != trident.RuntimeTypeContainer {
		return
	}
	url, ok := s.config.S("image", "url").Data().(string)
	if !ok || !strings.HasSuffix(url, usrVerityCosiSuffix) {
		return
	}
	if !s.config.Exists("storage", "encryption") {
		return
	}
	s.config.Set([]interface{}{"boot-loader-code", "kernel-boot"}, "storage", "encryption", "pcrs")
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
