package helpers

import (
	"fmt"
	"os"
	"storm/pkg/storm"
	"storm/suites/trident/utils"
	"strings"
	"time"

	"golang.org/x/crypto/ssh"
)

type CheckSshHelper struct {
	args struct {
		SshKeyPath string `arg:"" help:"Path to the SSH key file" type:"existingfile"`
		Host       string `arg:"" help:"Host to check SSH connection"`
		User       string `arg:"" help:"User to use for SSH connection"`
		Env        string `arg:"" help:"Environment where Trident is running" enum:"host,container"`
		Port       uint16 `short:"p" help:"Port to connect to" default:"22"`
		Timeout    int    `short:"t" help:"Timeout in seconds for the first SSH connection" default:"600"`
	}
}

func (h CheckSshHelper) Name() string {
	return "check-ssh"
}

func (h *CheckSshHelper) Args() any {
	return &h.args
}

func (h CheckSshHelper) Run(ctx storm.Context) error {
	ctx.Logger().Infof("Checking SSH connection to '%s' as user '%s'", h.args.Host, h.args.User)

	private_key, err := os.ReadFile(h.args.SshKeyPath)
	if err != nil {
		return fmt.Errorf("failed to read SSH key file '%s': %w", h.args.SshKeyPath, err)
	}

	signer, err := ssh.ParsePrivateKey(private_key)
	if err != nil {
		return fmt.Errorf("failed to parse SSH key: %w", err)
	}

	clientConfig := &ssh.ClientConfig{
		User: h.args.User,
		Auth: []ssh.AuthMethod{
			ssh.PublicKeys(signer),
		},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         time.Second * 15,
	}

	host := fmt.Sprintf("%s:%d", h.args.Host, h.args.Port)
	tc := ctx.NewTestCase("SSH Dial")

	client, err := utils.Retry(
		time.Second*time.Duration(h.args.Timeout),
		time.Second*5,
		func(attempt int) (*ssh.Client, error) {
			tc.Logger().Infof("SSH dial to '%s' (attempt %d)", host, attempt)
			return ssh.Dial("tcp", host, clientConfig)
		},
	)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}
	defer client.Close()

	if h.args.Env == "host" {
		tc = ctx.NewTestCase("Check Trident Service")
		_, err = utils.Retry(
			time.Minute*5,
			time.Second*5,
			func(attempt int) (*ssh.Client, error) {
				tc.Logger().Infof("Checking Trident service status (attempt %d)", attempt)
				return nil, checkTridentService(tc, client)
			},
		)

		if err != nil {
			// Log this as a test failure
			tc.FailFromError(err)
		}
	}

	return nil
}

func checkTridentService(lp storm.LoggerProvider, client *ssh.Client) error {
	session, err := client.NewSession()
	if err != nil {
		return fmt.Errorf("failed to create SSH session: %w", err)
	}
	defer session.Close()

	output, err := session.CombinedOutput("sudo systemctl status trident.service --no-pager")
	if err != nil {
		// For some reason systemctl likes returning 3 as an exit code when we do this, so ignore. :)
		if exitErr, ok := err.(*ssh.ExitError); !(ok && exitErr.ExitStatus() == 3) {
			return fmt.Errorf("failed to check Trident service status: %w", err)
		}
	}

	outputStr := string(output)

	lp.Logger().Debugf("Trident service status:\n%s", outputStr)

	if !strings.Contains(outputStr, "(code=exited, status=0/SUCCESS") {
		return fmt.Errorf("expected to find '(code=exited, status=0/SUCCESS)' in Trident service status")
	}

	lp.Logger().Info("Trident service ran successfully")

	return nil
}
