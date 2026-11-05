package scenario

import (
	"fmt"
	"os"
	"path"
	"path/filepath"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

// maxCiRingImageVersion and maxSplitRingImageVersion are the highest image
// versions the scenario's A/B sequence consumes. The sequence is: install (v1)
// → ab-update-1 (v2) → auto-rollback (v3) → ab-update-2 (reuses v3) → split
// (v4, only at ring >= pre). v1 and v2 must be provided as distinct real
// images; v3+ are hard-links (see prepareTestImages).
const (
	maxCiRingImageVersion    = 3
	maxSplitRingImageVersion = 4
)

// prepareTestImages ensures the versioned test images the scenario's A/B updates
// will request exist in the test image directory. It folds the versioning half
// of the legacy `prepare-images` helper into the scenario: v1 (<type>.cosi) and
// v2 (<type>_v2.cosi) are real, distinct images that must already be present,
// and this creates the higher versions as hard-links following the same scheme
// prepare-images used — odd versions alias v1, even versions alias v2 (so a
// version's filesystem UUID differs from the currently-active volume). It is a
// no-op for configs without A/B updates and for OCI-hosted images (which the
// pipeline stages in ACR).
func (s *TridentE2EScenario) prepareTestImages(tc storm.TestCase) error {
	if !s.originalConfig.HasABUpdate() {
		return nil
	}

	url, ok := s.config.S("image", "url").Data().(string)
	if !ok {
		return fmt.Errorf("failed to read image.url from Host Config")
	}
	if strings.HasPrefix(url, "oci://") {
		tc.Skip("Image is OCI-hosted; versioned images are staged in ACR by the pipeline")
	}

	base := path.Base(url)
	ext := strings.TrimPrefix(filepath.Ext(base), ".")
	if ext == "" {
		return fmt.Errorf("failed to determine extension of image %q", base)
	}
	imageType := strings.TrimSuffix(base, "."+ext)

	maxVersion := maxCiRingImageVersion
	if !s.splitTestsSkippedForCurrentRing() {
		maxVersion = maxSplitRingImageVersion
	}

	for version := 3; version <= maxVersion; version++ {
		if err := s.ensureVersionedImage(imageType, ext, version); err != nil {
			return err
		}
	}
	return nil
}

// ensureVersionedImage hard-links <type>_v<version>.<ext> to the appropriate
// base image if it does not already exist: odd versions alias v1
// (<type>.<ext>), even versions alias v2 (<type>_v2.<ext>).
func (s *TridentE2EScenario) ensureVersionedImage(imageType, ext string, version int) error {
	dir := s.args.TestImageDir
	targetName := fmt.Sprintf("%s_v%d.%s", imageType, version, ext)
	targetPath := filepath.Join(dir, targetName)

	if _, err := os.Stat(targetPath); err == nil {
		logrus.Debugf("Versioned image %q already exists; leaving as-is", targetName)
		return nil
	}

	var sourceName string
	if version%2 == 0 {
		sourceName = fmt.Sprintf("%s_v2.%s", imageType, ext)
	} else {
		sourceName = fmt.Sprintf("%s.%s", imageType, ext)
	}
	sourcePath := filepath.Join(dir, sourceName)

	if _, err := os.Stat(sourcePath); err != nil {
		return fmt.Errorf("cannot create versioned image %q: source image %q not found: %w",
			targetName, sourceName, err)
	}

	logrus.Infof("Linking test image %q -> %q (v%d)", targetName, sourceName, version)
	if err := os.Link(sourcePath, targetPath); err != nil {
		return fmt.Errorf("failed to link %q to %q: %w", targetName, sourceName, err)
	}
	return nil
}
