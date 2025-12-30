package scenario

import (
	"fmt"
	"net/http"
	"path"
	"regexp"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

func (s *TridentE2EScenario) AddAbUpdateTests(r storm.TestRegistrar, prefix string) {
	r.RegisterTestCase(prefix+"-update-hc", s.updateHostConfig)
}

func (s *TridentE2EScenario) updateHostConfig(tc storm.TestCase) error {
	// Bump the image version by 1:
	s.version += 1

	// Get the old image URL from config
	oldUrl, ok := s.config.S("image", "url").Data().(string)
	if !ok {
		return fmt.Errorf("failed to get old image URL from config")
	}

	logrus.Infof("Old image URL: %s", oldUrl)

	// Extract the base name of the image URL
	base := path.Base(oldUrl)
	if base == "" {
		return fmt.Errorf("failed to get base name from URL: %s", oldUrl)
	}

	// Get the URL path without the base name
	urlPath, ok := strings.CutSuffix(oldUrl, base)
	if !ok {
		return fmt.Errorf("failed to remove suffix '%s' from URL '%s'", base, oldUrl)
	}

	logrus.Debugf("Base name: %s", base)

	var newCosiName string
	if strings.HasPrefix(oldUrl, "oci://") {
		// Special handling for OCI URLs

		// Match form <repository_base>:v<build ID>.<config>.<deployment env>.<version number>
		matches := regexp.MustCompile(`^(.+):v(\d+)\.(.+)\.(.+)\.(\d+)$`).FindStringSubmatch(base)
		if len(matches) != 6 {
			return fmt.Errorf("failed to parse OCI image name: %s", base)
		}

		name := matches[1]
		buildId := matches[2]
		config := matches[3]
		deploymentEnv := matches[4]
		newCosiName = fmt.Sprintf("%s:v%s.%s.%s.%d", name, buildId, config, deploymentEnv, s.version)
	} else {
		// Match form <name>_v<version number>.<file extension> (note that "_v<version number>" is optional)
		matches := regexp.MustCompile(`^(.*?)(_v\d+)?\.(.+)$`).FindStringSubmatch(base)
		if len(matches) != 4 {
			return fmt.Errorf("failed to parse image name: %s", base)
		}

		name := matches[1]
		ext := matches[3]
		newCosiName = fmt.Sprintf("%s_v%d.%s", name, s.version, ext)
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

	return nil
}

func (s *TridentE2EScenario) abUpdateOs(tc storm.TestCase) error {
	// ensure ssh client is populated
	err := s.populateSshClient(tc.Context())
	if err != nil {
		// At this point we know the VM is up, so failing to populate SSH client is a test error.
		return fmt.Errorf("failed to populate SSH client: %w", err)
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
