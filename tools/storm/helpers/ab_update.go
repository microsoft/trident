package helpers

import (
	"context"
	"crypto/sha512"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"os"
	"path"
	"regexp"
	"strings"
	"time"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"
	"oras.land/oras-go/v2"
	"oras.land/oras-go/v2/content/file"
	"oras.land/oras-go/v2/registry/remote"

	"tridenttools/storm/utils"
)

type AbUpdateHelper struct {
	args struct {
		utils.SshCliSettings `embed:""`
		utils.EnvCliSettings `embed:""`
		TridentConfig        string   `short:"c" required:"" help:"File name of the custom read-write Trident config on the host to point Trident to."`
		Version              string   `short:"v" required:"" help:"Version of the Trident image to use for the A/B update."`
		StageAbUpdate        bool     `short:"s" help:"Controls whether A/B update should be staged."`
		FinalizeAbUpdate     bool     `short:"f" help:"Controls whether A/B update should be finalized."`
		EnvVars              []string `short:"e" help:"Environment variables. Multiple vars can be passed as a list of comma-separated strings, or this flag can be used multiple times. Each var should include the env var name, i.e. HTTPS_PROXY=http://0.0.0.0."`
		ExpectFailedCommit   bool     `help:"Controls whether this test treats failed commits as successful." default:"false"`
		ForcedRollback       bool     `help:"Controls whether this test includes a forced auto-rollback during A/B update." default:"false"`
	}

	client *ssh.Client
	config map[string]interface{}
}

func (h AbUpdateHelper) Name() string {
	return "ab-update"
}

func (h *AbUpdateHelper) Args() any {
	return &h.args
}

func (h *AbUpdateHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("get-config", h.getHostConfig)
	r.RegisterTestCase("update-hc", h.updateHostConfig)
	r.RegisterTestCase("trigger-update", h.triggerTridentUpdate)
	r.RegisterTestCase("check-trident-service", h.checkTridentService)
	return nil
}

func (h *AbUpdateHelper) getHostConfig(tc storm.TestCase) error {
	if h.args.Env == utils.TridentEnvironmentNone {
		return fmt.Errorf("environment %s is not supported", h.args.Env)
	}

	var err error
	h.client, err = utils.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
	}

	tc.SuiteCleanup(func() {
		if h.client != nil {
			h.client.Close()
		}
	})

	out, err := utils.InvokeTrident(h.args.Env, h.client, h.args.EnvVars, "get configuration")
	if err != nil {
		return fmt.Errorf("failed to invoke Trident: %w", err)
	}

	if err := out.Check(); err != nil {
		return fmt.Errorf("failed to run trident to get host config: %w", err)
	}

	logrus.Debugf("Trident stdout:\n%s", out.Stdout)
	logrus.Debugf("Trident stderr:\n%s", out.Stderr)

	err = yaml.Unmarshal([]byte(out.Stdout), &h.config)
	if err != nil {
		return fmt.Errorf("failed to unmarshal YAML: %w", err)
	}
	logrus.Infof("Trident configuration: %v", h.config)

	return nil
}

func (h *AbUpdateHelper) updateHostConfig(tc storm.TestCase) error {
	if !h.args.StageAbUpdate {
		tc.Skip("Staging not requested")
	}

	// Handle auto-rollback settings if configured
	h.handleAutoRollback(tc)

	// Extract the OLD URL from the configuration
	oldUrl, ok := h.config["image"].(map[string]interface{})["url"].(string)
	if !ok {
		return fmt.Errorf("failed to get old image URL from configuration")
	}

	logrus.Infof("Old image URL: %s", oldUrl)

	// Extract the base name of the image URL
	base := path.Base(oldUrl)
	if base == "" {
		return fmt.Errorf("failed to get base name from URL: %s", oldUrl)
	}

	// Then extract everything but the base by removing it as a suffix
	urlPath, ok := strings.CutSuffix(oldUrl, base)
	if !ok {
		return fmt.Errorf("failed to remove suffix '%s' from URL '%s'", base, oldUrl)
	}

	logrus.Debugf("Base name: %s", base)

	// Match form <repository>:v<build ID>.<config>.<deployment env>.<version number>
	matches_oci := regexp.MustCompile(`^(.+):v(\d+)\.(.+)\.(.+)\.(\d+)$`).FindStringSubmatch(base)
	// Match form <name>_v<version number>.<file extension> (note that "_v<version number>" is optional)
	matches := regexp.MustCompile(`^(.*?)(_v\d+)?\.(.+)$`).FindStringSubmatch(base)

	var newCosiName string

	if strings.HasPrefix(oldUrl, "oci://") && len(matches_oci) == 6 {
		name := matches_oci[1]
		buildId := matches_oci[2]
		config := matches_oci[3]
		deploymentEnv := matches_oci[4]
		newCosiName = fmt.Sprintf("%s:v%s.%s.%s.%s", name, buildId, config, deploymentEnv, h.args.Version)
	} else if len(matches) == 4 {
		name := matches[1]
		ext := matches[3]
		newCosiName = fmt.Sprintf("%s_v%s.%s", name, h.args.Version, ext)
	} else {
		return fmt.Errorf("failed to parse image name: %s", base)
	}

	newUrl := fmt.Sprintf("%s%s", urlPath, newCosiName)
	logrus.Infof("New image URL: %s", newUrl)

	logrus.Infof("Checking if new image URL is accessible...")
	err := checkUrlIsAccessible(newUrl)
	if err != nil {
		logrus.WithError(err).Errorf("New image URL is not accessible: %s (continuing)", newUrl)
	} else {
		logrus.Infof("New image URL is accessible")
	}

	// Update the image URL in the configuration
	h.config["image"].(map[string]any)["url"] = newUrl
	h.config["image"].(map[string]any)["sha384"] = "ignored"

	// Set the config to NOT self-upgrade
	internalParams, ok := h.config["internalParams"].(map[string]any)
	if !ok {
		internalParams = make(map[string]any)
		h.config["internalParams"] = internalParams
	}
	internalParams["selfUpgradeTrident"] = false

	// Delete the storage section from the config, not needed for A/B update
	delete(h.config, "storage")

	// Update the sysext and confext files.
	osConfig, ok := h.config["os"].(map[string]interface{})
	if ok {
		err := updateExtensions(osConfig)
		if err != nil {
			return fmt.Errorf("failed to update extensions in Host Configuration: %w", err)
		}
	}

	hc_yaml, err := yaml.Marshal(h.config)
	if err != nil {
		return fmt.Errorf("failed to marshal YAML: %w", err)
	}

	sftpClient, err := utils.NewSftpSudoClient(h.client)
	if err != nil {
		return fmt.Errorf("failed to create SudoSFTP client: %w", err)
	}
	defer sftpClient.Close()

	// Ensure the cosi file exists
	err = sftpClient.MkdirAll(path.Dir(h.args.TridentConfig))
	if err != nil {
		return fmt.Errorf("failed to create directory for new Host Config file: %w", err)
	}

	file, err := sftpClient.Create(h.args.TridentConfig)
	if err != nil {
		return fmt.Errorf("failed to create new Host Config file: %w", err)
	}
	defer file.Close()

	_, err = file.Write(hc_yaml)
	if err != nil {
		return fmt.Errorf("failed to write new Host Config file: %w", err)
	}

	err = file.Chmod(0644)
	if err != nil {
		return fmt.Errorf("failed to change permissions of new Host Config file: %w", err)
	}

	err = file.Chown(0, 0)
	if err != nil {
		return fmt.Errorf("failed to change ownership of new Host Config file: %w", err)
	}

	return nil
}

func (h *AbUpdateHelper) handleAutoRollback(tc storm.TestCase) error {
	systemdCheckName := "check-non-existent-service-to-invoke-rollback"
	scriptCheckName := "invoke-rollback-from-script"

	if h.args.ForcedRollback {
		if _, ok := h.config["health"].(map[string]interface{}); !ok {
			h.config["health"] = map[string]interface{}{}
		}
		if _, ok := h.config["health"].(map[string]interface{})["checks"].([]interface{}); !ok {
			h.config["health"].(map[string]interface{})["checks"] = make([]interface{}, 0)
		}

		// Add a script health check that always fails during A/B update to trigger auto-rollback
		h.config["health"].(map[string]interface{})["checks"] = append(
			h.config["health"].(map[string]interface{})["checks"].([]interface{}),
			map[string]interface{}{
				"content": "exit 1",
				"runOn":   []string{"ab-update"},
				"name":    scriptCheckName,
			},
		)

		// Add a systemd health check that always fails during A/B update to trigger auto-rollback
		h.config["health"].(map[string]interface{})["checks"] = append(
			h.config["health"].(map[string]interface{})["checks"].([]interface{}),
			map[string]interface{}{
				"runOn":           []string{"ab-update"},
				"name":            systemdCheckName,
				"systemdServices": []string{"non-existent-service1", "non-existent-service2"},
				"timeoutSeconds":  30,
			},
		)
	} else {
		// Remove check-non-existent-service-to-invoke-rollback and
		// invoke-rollback-from-script checks if they exist
		if health, ok := h.config["health"].(map[string]interface{}); ok {
			if checks, ok := health["checks"].([]interface{}); ok {
				newChecks := make([]interface{}, 0)
				for _, check := range checks {
					checkMap, ok := check.(map[string]interface{})
					if !ok {
						newChecks = append(newChecks, check)
						continue
					}
					name, ok := checkMap["name"].(string)
					if !ok {
						newChecks = append(newChecks, check)
						continue
					}
					if name == systemdCheckName || name == scriptCheckName {
						continue
					}
					newChecks = append(newChecks, check)
				}
				health["checks"] = newChecks
			}
		}
	}
	return nil
}

func (h *AbUpdateHelper) triggerTridentUpdate(tc storm.TestCase) error {
	allowedOperations := make([]string, 0)

	if h.args.StageAbUpdate {
		logrus.Infof("Allowed operations: stage")
		allowedOperations = append(allowedOperations, "stage")
	}

	if h.args.FinalizeAbUpdate {
		logrus.Infof("Allowed operations: finalize")
		allowedOperations = append(allowedOperations, "finalize")
	}

	args := fmt.Sprintf(
		"update -v trace %s --allowed-operations %s",
		path.Join(h.args.Env.HostPath(), h.args.TridentConfig),
		strings.Join(allowedOperations, ","),
	)

	file, err := utils.CommandOutput(h.client, fmt.Sprintf("sudo cat %s", h.args.TridentConfig))
	if err != nil {
		return fmt.Errorf("failed to read new Host Config file: %w", err)
	}

	logrus.Debugf("Trident config file:\n%s", file)

	for i := 1; ; i++ {
		logrus.Infof("Invoking Trident attempt #%d with args: %s", i, args)

		out, err := utils.InvokeTrident(h.args.Env, h.client, h.args.EnvVars, args)
		if err != nil {
			if err, ok := err.(*ssh.ExitMissingError); ok && strings.Contains(out.Stderr, "Rebooting system") {
				// The connection closed without an exit code, and the output contains "Rebooting system".
				// This indicates that the host has rebooted.
				logrus.Infof("Host rebooted successfully")
				break
			} else {
				// Some unknown error occurred.
				logrus.Errorf("Failed to invoke Trident: %s; %s", err, out.Report())
				return fmt.Errorf("failed to invoke Trident: %w", err)
			}
		}

		if out.Status == 0 && strings.Contains(out.Stderr, "Staging of update 'AbUpdate' succeeded") {
			logrus.Infof("Staging of update 'AbUpdate' succeeded")
			break
		}

		if out.Status == 2 && strings.Contains(out.Stderr, "Failed to run post-configure script 'fail-on-the-first-run'") {
			logrus.Infof("Detected intentional failure. Re-running...")
			continue
		}

		logrus.Errorf("Trident update failed %s", out.Report())

		tc.Fail(fmt.Sprintf("Trident update failed with status %d", out.Status))
	}

	// On success close the client because the host will reboot into the new OS.
	h.client.Close()
	h.client = nil

	return nil
}

func (h *AbUpdateHelper) checkTridentService(tc storm.TestCase) error {
	if h.args.Env == utils.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}

	logrus.Infof("Waiting for the host to reboot and come back online...")
	time.Sleep(time.Second * 10)

	// Reconnect via SSH to the updated OS
	endTime := time.Now().Add(h.args.TimeoutDuration())
	_, err := utils.Retry(
		time.Until(endTime),
		time.Second*5,
		func(attempt int) (*bool, error) {
			logrus.Infof("SSH dial to '%s' (attempt %d)", h.args.SshCliSettings.FullHost(), attempt)
			client, err := utils.OpenSshClient(h.args.SshCliSettings)
			if err != nil {
				logrus.Warnf("Failed to dial SSH server '%s': %s", h.args.SshCliSettings.FullHost(), err)
				return nil, err
			}
			defer client.Close()

			logrus.Infof("SSH dial to '%s' succeeded", h.args.SshCliSettings.FullHost())

			// Enable tests to handle success and failure of commit service
			// depending on configuration
			expectSuccessfulCommit := !h.args.ExpectFailedCommit
			err = utils.CheckTridentService(client, h.args.Env, time.Until(endTime), expectSuccessfulCommit)
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

func checkUrlIsAccessible(url string) error {
	resp, err := http.Head(url)
	if err != nil {
		return fmt.Errorf("failed to check new image URL: %w", err)
	}
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("new image URL is not accessible: %s, got HTTP code: %d", url, resp.StatusCode)
	}

	return nil
}

// Update the paths of the extension images. This update happens only once, from
// version 1 to 2. If images are already version 2, then keep as is.
func updateExtensions(osConfig map[string]interface{}) error {
	for _, extensionType := range []string{"sysexts", "confexts"} {
		extensions, ok := osConfig[extensionType].([]interface{})
		if !ok || len(extensions) == 0 {
			continue // No extensions of this type, skip
		}
		extension, ok := extensions[0].(map[string]interface{})
		if !ok {
			continue // Invalid extension format, skip
		}

		// Update URL from version 1 to 2
		oldUrl, ok := extension["url"].(string)
		if !ok || !strings.HasSuffix(oldUrl, ".1") {
			continue // No URL or not version 1, skip
		}
		trimmedUrl := strings.TrimSuffix(oldUrl, ".1")
		newUrl := fmt.Sprintf("%s.2", trimmedUrl)

		// Calculate new hash
		newHash, err := pullImageAndCalculateSha384(newUrl)
		if err != nil {
			return fmt.Errorf("failed to calculate SHA384 hash of %s: %w", newUrl, err)
		}

		// Update the extension configuration
		extension["url"] = newUrl
		extension["sha384"] = newHash
	}

	return nil
}

// Download the new extension image and calculate SHA384 hash to populate the
// updated Host Configuration.
func pullImageAndCalculateSha384(imageUrl string) (string, error) {
	url := strings.TrimPrefix(imageUrl, "oci://")
	parts := strings.Split(url, ":")
	if len(parts) != 2 {
		return "", fmt.Errorf("invalid image URL format: %s", imageUrl)
	}
	ref := parts[0]
	tag := parts[1]

	// Create repository client
	repo, err := remote.NewRepository(ref)
	if err != nil {
		return "", fmt.Errorf("failed to create repository client: %w", err)
	}

	// Create temporary directory to pull files into
	tempDir, err := os.MkdirTemp("", "oras_pull_*")
	if err != nil {
		return "", fmt.Errorf("failed to create temp directory: %w", err)
	}
	defer os.RemoveAll(tempDir) // Clean up

	// Create file store targeting the temp directory
	fileStore, err := file.New(tempDir)
	if err != nil {
		return "", fmt.Errorf("failed to create file store: %w", err)
	}
	defer fileStore.Close()

	// Pull the artifact
	_, err = oras.Copy(context.Background(), repo, tag, fileStore, tag, oras.DefaultCopyOptions)
	if err != nil {
		return "", fmt.Errorf("failed to pull artifact: %w", err)
	}

	// Find the .raw file in the directory
	files, err := os.ReadDir(tempDir)
	if err != nil {
		return "", fmt.Errorf("failed to read temp directory: %w", err)
	}
	if len(files) != 1 {
		return "", fmt.Errorf("expected to find exactly one file in temp directory after pulling artifact")
	}
	rawFilePath := path.Join(tempDir, files[0].Name())

	// Hash the .raw file
	file, err := os.Open(rawFilePath)
	if err != nil {
		return "", fmt.Errorf("failed to open .raw file: %w", err)
	}
	defer file.Close()
	hasher := sha512.New384()
	if _, err := io.Copy(hasher, file); err != nil {
		return "", fmt.Errorf("failed to calculate hash: %w", err)
	}
	return hex.EncodeToString(hasher.Sum(nil)), nil
}
