package netlaunch

import (
	"fmt"

	log "github.com/sirupsen/logrus"
)

const systemdServiceExecOverrideTemplate = `
[Service]
ExecStart=
ExecStart=%s
StandardOutput=journal+console
StandardError=journal+console
Environment="LOGSTREAM_URL=%s"
`

func makeStreamImageOverrideFileDownload(tridentConfig map[string]any, logstreamAddress string) (rcpAgentFileDownload, error) {
	imgConf, ok := tridentConfig["image"]
	if !ok {
		return rcpAgentFileDownload{}, fmt.Errorf("trident config does not contain an image section")
	}

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
		// If we can't find a sha384 field default to "ignored", which Trident
		// will accept and just skip the hash verification.
		imgSha384 = "ignored"
	}

	fileContent := fmt.Sprintf(
		systemdServiceExecOverrideTemplate,
		fmt.Sprintf("/usr/bin/trident grpc-client stream-image %s --hash %s", imgUrl, imgSha384),
		logstreamAddress,
	)

	log.Infof("Generated stream image override file content:\n%s", fileContent)

	return newRcpAgentFileDownload(
		"trident-stream-image-override",
		"/etc/systemd/system/trident-install.service.d/override.conf",
		0644,
		[]byte(fileContent),
	), nil
}
