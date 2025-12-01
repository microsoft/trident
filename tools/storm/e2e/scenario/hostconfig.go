package scenario

import (
	"fmt"
	"tridenttools/storm/utils/ssh/keys"

	"github.com/Jeffail/gabs/v2"
	"github.com/microsoft/storm"
)

const (
	testingUsername = "testing-user"
)

func (s *TridentE2EScenario) prepareHostConfig(tc storm.TestCase) error {
	c := gabs.Wrap(s.config)

	// Generate an SSH key pair for VM access, store the private key for later use
	private, public, err := keys.GenerateRsaKeyPair(2048)
	if err != nil {
		return fmt.Errorf("failed to generate RSA key pair for e2e: %w", err)
	}
	s.sshPrivateKey = string(private)

	// Add the public key to the testing user
	found := false
	for _, user := range c.S("os", "users").Children() {
		if user.S("name").Data().(string) == testingUsername {
			user.ArrayAppend(string(public), "sshPublicKeys")
			found = true
		}
	}

	if !found {
		c.ArrayConcat(map[string]interface{}{
			"name":          testingUsername,
			"sshPublicKeys": []string{string(public)},
		}, "os", "users")
	}

	// If this is a container runtime, add the trident-container.tar.gz file to additional files.
	if s.runtime == RuntimeTypeContainer {
		containerAdditionalFile := map[string]string{
			"source":      "/var/lib/trident/trident-container.tar.gz",
			"destination": "/var/lib/trident/trident-container.tar.gz",
		}
		c.ArrayAppend(containerAdditionalFile, "os", "additionalFiles")
	}

	// Update the scenario config
	s.config = c.Data().(map[string]interface{})

	return nil
}
