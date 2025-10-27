package helpers

import (
	"fmt"
	"time"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"

	"tridenttools/storm/utils"
)

type CheckSshHelper struct {
	args struct {
		utils.SshCliSettings `embed:""`
		utils.EnvCliSettings `embed:""`
		CheckActiveVolume    string `help:"Check that the indicated volume is the active one"`
	}
	client *ssh.Client
}

func (h CheckSshHelper) Name() string {
	return "check-ssh"
}

func (h *CheckSshHelper) Args() any {
	return &h.args
}

func (h *CheckSshHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("check-ssh", h.sshDial)
	r.RegisterTestCase("check-trident-service", h.checkTridentService)
	r.RegisterTestCase("check-active-volume", h.checkActiveVolume)
	return nil
}

func (h *CheckSshHelper) sshDial(tc storm.TestCase) error {
	logrus.Infof("Checking SSH connection to '%s' as user '%s'", h.args.Host, h.args.User)

	var err error
	h.client, err = utils.Retry(
		time.Second*time.Duration(h.args.Timeout),
		time.Second*5,
		func(attempt int) (*ssh.Client, error) {
			logrus.Infof("SSH dial to '%s' (attempt %d)", h.args.SshCliSettings.FullHost(), attempt)
			return utils.OpenSshClient(h.args.SshCliSettings)
		},
	)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	// Close the SSH client when the suite is done.
	tc.SuiteCleanup(func() {
		if h.client != nil {
			h.client.Close()
		}
	})

	return nil
}

func (h *CheckSshHelper) checkTridentService(tc storm.TestCase) error {
	if h.args.Env == utils.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}

	err := utils.CheckTridentService(h.client, h.args.Env, h.args.TimeoutDuration(), true)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	return nil
}

func (h *CheckSshHelper) checkActiveVolume(tc storm.TestCase) error {
	if h.args.CheckActiveVolume == "" {
		tc.Skip("No active volume check requested")
	}

	_, err := utils.Retry(
		time.Second*5,
		time.Second,
		func(attempt int) (*ssh.Client, error) {
			logrus.Infof("Checking active volume (attempt %d)", attempt)
			return nil, checkActiveVolumeInner(h.client, h.args.CheckActiveVolume)
		},
	)

	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	return nil
}

func checkActiveVolumeInner(client *ssh.Client, expectedActiveVolume string) error {
	session, err := client.NewSession()
	if err != nil {
		return fmt.Errorf("failed to create SSH session: %w", err)
	}
	defer session.Close()

	output, err := session.Output("sudo trident get")
	if err != nil {
		return fmt.Errorf("failed to get volumes: %w", err)
	}

	outputStr := string(output)

	logrus.Debugf("Host Status:\n%s", outputStr)

	hostStatus := make(map[string]interface{})
	if err = yaml.Unmarshal([]byte(outputStr), &hostStatus); err != nil {
		return fmt.Errorf("failed to unmarshal YAML output: %w", err)
	}

	if hostStatus["servicingState"] != "provisioned" {
		return fmt.Errorf("trident state is not 'provisioned'")
	}

	logrus.Info("Host is in provisioned state")

	hsActiveVol := hostStatus["abActiveVolume"]
	if hsActiveVol != expectedActiveVolume {
		return fmt.Errorf("expected active volume '%s', got '%s'", expectedActiveVolume, hsActiveVol)
	}

	logrus.Infof("Active volume is '%s'", hsActiveVol)

	return nil
}
