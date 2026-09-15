package scenario

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"tridenttools/storm/utils/sha384"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

const (
	testingUsername = "testing-user"

	// usrVerityCosiSuffix identifies the usr-verity UKI test image whose
	// encryption must not seal to PCR 7 under a containerized runtime.
	usrVerityCosiSuffix = "usrverity.cosi"

	// secureBootPolicyPcr is PCR 7, which a containerized Trident cannot
	// reproduce because Secure Boot measures the host boot chain, not the
	// container's.
	secureBootPolicyPcr = "secure-boot-policy"

	// EffectiveHostConfigArtifact is where prepare-hc publishes the Host
	// Configuration that was actually deployed, after every scenario-side edit.
	// Downstream pipeline steps (metrics enrichment) read it so they describe
	// the deployed config rather than the on-disk template.
	EffectiveHostConfigArtifact = "host-config.yaml"
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
	if err := s.applyOciOverrides(); err != nil {
		return err
	}

	// Inject UEFI fallback validation last among the edits, so the health
	// checks describe the configuration that is actually deployed.
	if err := s.injectUefiFallbackValidation(); err != nil {
		return err
	}

	// Publish the fully-edited configuration last, so it reflects exactly what
	// gets deployed. Only the testing user's *public* key was added above, so
	// this carries no secret.
	effective, err := s.config.ToYaml()
	if err != nil {
		return fmt.Errorf("failed to serialize the effective Host Configuration: %w", err)
	}
	tc.ArtifactBroker().PublishArtifactData(EffectiveHostConfigArtifact, effective)

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
	// Filter the configured list rather than replacing it: a configuration that
	// seals to additional PCRs must keep them, and only PCR 7 is the problem
	// here.
	var kept []interface{}
	for _, pcr := range s.config.S("storage", "encryption", "pcrs").Children() {
		if name, ok := pcr.Data().(string); ok && name == secureBootPolicyPcr {
			continue
		}
		kept = append(kept, pcr.Data())
	}
	s.config.Set(kept, "storage", "encryption", "pcrs")
}

// applyOciOverrides injects the OCI-based Host Configuration edits requested via
// scenario arguments: system/configuration extension images (os.sysexts /
// os.confexts) and an override of the COSI image URL (image.url). Each edit is
// applied only when its argument is provided. Ports the OCI handling of
// edit_host_config.py used by the pipeline's trident-prep step.
func (s *TridentE2EScenario) applyOciOverrides() error {
	sysextUrl, sysextSha, err := s.resolveSysextImage()
	if err != nil {
		return err
	}
	if sysextUrl != "" {
		if err := s.config.ArrayAppend(map[string]interface{}{
			"url":    sysextUrl,
			"sha384": sysextSha,
		}, "os", "sysexts"); err != nil {
			return fmt.Errorf("failed to inject system extension image: %w", err)
		}
	}

	if s.args.ConfextOciUrl != "" {
		if s.args.ConfextSha384 == "" {
			return fmt.Errorf("--confext-sha384 is required when --confext-oci-url is provided")
		}
		if err := s.config.ArrayAppend(map[string]interface{}{
			"url":    s.args.ConfextOciUrl,
			"sha384": s.args.ConfextSha384,
		}, "os", "confexts"); err != nil {
			return fmt.Errorf("failed to inject configuration extension image: %w", err)
		}
	}

	if s.args.OciImageUrl != "" {
		s.config.Set(s.args.OciImageUrl, "image", "url")
	}

	return nil
}

// resolveSysextImage returns the OCI URL and hash of the system extension image
// to inject, if any.
//
// An explicit --sysext-oci-url wins. Otherwise the URL is assembled from the
// ACR/repository/tag the push step reported, and the hash is computed from the
// local copy of the image - so the caller passes what it knows rather than
// building an OCI reference and shelling out to sha384sum. A configuration that
// uses no extensions simply leaves these empty.
func (s *TridentE2EScenario) resolveSysextImage() (url string, hash string, err error) {
	if s.args.SysextOciUrl != "" {
		// sha384 is a required field of the extension schema, so injecting an
		// empty one surfaces as an opaque Trident validation error much later.
		if s.args.SysextSha384 == "" {
			return "", "", fmt.Errorf("--sysext-sha384 is required when --sysext-oci-url is provided")
		}
		return s.args.SysextOciUrl, s.args.SysextSha384, nil
	}

	if s.args.SysextAcr == "" || s.args.SysextRepo == "" || s.args.SysextTag == "" {
		return "", "", nil
	}

	if s.args.SysextFile == "" {
		return "", "", fmt.Errorf("--sysext-file is required to hash the image referenced by --sysext-acr/--sysext-repo/--sysext-tag")
	}

	hash, err = sha384.CalculateSha384(s.args.SysextFile)
	if err != nil {
		return "", "", fmt.Errorf("failed to hash system extension image %q: %w", s.args.SysextFile, err)
	}

	url = fmt.Sprintf("oci://%s.azurecr.io/%s:%s", s.args.SysextAcr, s.args.SysextRepo, s.args.SysextTag)
	logrus.Infof("System extension image: %s (sha384 %s)", url, hash)
	return url, hash, nil
}
