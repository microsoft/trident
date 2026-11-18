package activevolume

import (
	"fmt"
	"time"

	stormretry "tridenttools/storm/utils/retry"
	stormssh "tridenttools/storm/utils/ssh"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

func CheckActiveVolume(cfg stormvmconfig.VMConfig, vmIP string, expectedVolume string) error {
	_, err := stormretry.Retry(
		time.Second*600,
		time.Second,
		func(attempt int) (*bool, error) {
			logrus.Tracef("Checking active volume (attempt %d)", attempt)
			hostStatusStr, err := stormssh.SshCommandWithRetries(cfg, vmIP, "sudo trident get", 5, 5)
			logrus.Tracef("(attempt %d) [%v]: %s", attempt, err, hostStatusStr)
			if err != nil {
				return nil, fmt.Errorf("failed to get host status: %w", err)
			}
			logrus.Tracef("Retrieved host status")
			if err := ValidateHostStatus(hostStatusStr, expectedVolume); err != nil {
				return nil, err
			}
			return nil, nil
		},
	)
	return err
}

// ValidateHostStatus checks a captured Host Status independently of its SSH transport.
func ValidateHostStatus(hostStatusStr, expectedVolume string) error {
	hostStatus := make(map[string]interface{})
	if err := yaml.Unmarshal([]byte(hostStatusStr), &hostStatus); err != nil {
		return fmt.Errorf("failed to unmarshal YAML output: %w", err)
	}
	if hostStatus["servicingState"] != "provisioned" {
		return fmt.Errorf("trident state is not 'provisioned': %v", hostStatus["servicingState"])
	}
	hsActiveVol := hostStatus["abActiveVolume"]
	if hsActiveVol != expectedVolume {
		return fmt.Errorf("expected active volume '%s', got '%v'", expectedVolume, hsActiveVol)
	}
	logrus.Infof("Active volume '%s' matches expected volume '%s'", hsActiveVol, expectedVolume)
	return nil
}
