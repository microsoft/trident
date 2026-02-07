package netlaunch

import (
	"fmt"

	"github.com/sirupsen/logrus"
)

const systemdServiceExecOverrideTemplate = `
[Service]
ExecStart=
ExecStart=%s
StandardOutput=journal+console
StandardError=journal+console
`

func makeStreamImageOverrideFileDownload(tridentConfig map[string]any) (rcpAgentFileDownload, error) {
	imgConf, ok := tridentConfig["image"]
	if !ok {
		return rcpAgentFileDownload{}, fmt.Errorf("trident config does not contain an image section")
	}

	logrus.Infof("CONFIG: %v", imgConf)

	imgConfMap, ok := imgConf.(map[any]any)
	if !ok {
		return rcpAgentFileDownload{}, fmt.Errorf("trident config image section is not a map")
	}

	imgUrl, ok := imgConfMap["url"].(string)
	if !ok {
		return rcpAgentFileDownload{}, fmt.Errorf("trident config image section does not contain a 'url' field of type string")
	}

	imgSha384, ok := imgConfMap["sha384"].(string)
	if !ok {
		imgSha384 = "ignored"
	}

	fileContent := fmt.Sprintf(
		systemdServiceExecOverrideTemplate,
		fmt.Sprintf("/usr/bin/trident grpc-client stream-image %s --hash %s", imgUrl, imgSha384),
	)

	return newRcpAgentFileDownload(
		"trident-stream-image-override",
		"/etc/systemd/system/trident-install.service.d/override.conf",
		0777,
		[]byte(fileContent),
	), nil
}
