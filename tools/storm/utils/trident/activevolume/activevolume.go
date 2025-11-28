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
			hostStatus := make(map[string]interface{})
			if err = yaml.Unmarshal([]byte(hostStatusStr), &hostStatus); err != nil {
				return nil, fmt.Errorf("failed to unmarshal YAML output: %w", err)
			}
			logrus.Tracef("Parsed host status")
			if hostStatus["servicingState"] != "provisioned" {
				return nil, fmt.Errorf("trident state is not 'provisioned'")
			}
			logrus.Tracef("Host satus servicingState is 'provisioned'")
			hsActiveVol := hostStatus["abActiveVolume"]
			if hsActiveVol != expectedVolume {
				return nil, fmt.Errorf("expected active volume '%s', got '%s'", expectedVolume, hsActiveVol)
			}
			logrus.Infof("Active volume '%s' matches expected volume '%s'", hsActiveVol, expectedVolume)
			return nil, nil
		},
	)
	return err
}
