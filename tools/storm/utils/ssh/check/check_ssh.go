package check

import (
	"time"

	stormenv "tridenttools/storm/utils/env"
	stormretry "tridenttools/storm/utils/retry"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormtrident "tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

func CheckTridentService(
	sshClientArgs stormsshconfig.SshCliSettings,
	envArgs stormenv.EnvCliSettings,
	expectSuccessfulCommit bool,
	timeout time.Duration,
	tc storm.TestCase) error {
	logrus.Infof("Waiting for the host to reboot and come back online...")
	time.Sleep(time.Second * 10)

	// Reconnect via SSH to the updated OS
	endTime := time.Now().Add(timeout)
	_, err := stormretry.Retry(
		time.Until(endTime),
		time.Second*5,
		func(attempt int) (*bool, error) {
			logrus.Infof("SSH dial to '%s' (attempt %d)", sshClientArgs.FullHost(), attempt)
			client, err := stormsshclient.OpenSshClient(sshClientArgs)
			if err != nil {
				logrus.Warnf("Failed to dial SSH server '%s': %s", sshClientArgs.FullHost(), err)
				return nil, err
			}
			defer client.Close()

			logrus.Infof("SSH dial to '%s' succeeded", sshClientArgs.FullHost())

			// Enable tests to handle success and failure of commit service
			// depending on configuration
			err = stormtrident.CheckTridentService(client, envArgs.Env, time.Until(endTime), expectSuccessfulCommit)
			if err != nil {
				logrus.Warnf("Trident service is not in expected state: %s", err)
				return nil, err
			}

			logrus.Infof("Trident service is in expected state")
			return nil, nil
		},
	)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	return nil
}
