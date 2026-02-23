package netlaunch

import (
	"fmt"
)

const systemdServiceExecOverrideTemplate = `
[Service]
ExecStart=
ExecStart=%s
StandardOutput=journal+console
StandardError=journal+console
Environment="LOGSTREAM_URL=%s"
`

func getUmageUrlAndHashFromTridentConfig(tridentConfig map[string]any) (string, string, error) {
	imgConf, ok := tridentConfig["image"]
	if !ok {
		return "", "", fmt.Errorf("trident config does not contain an image section")
	}

	imgConfMap, ok := imgConf.(map[any]any)
	if !ok {
		return "", "", fmt.Errorf("trident config image section is not a map")
	}

	imgUrl, ok := imgConfMap["url"].(string)
	if !ok {
		return "", "", fmt.Errorf("trident config image section does not contain a 'url' field of type string")
	}

	imgSha384, ok := imgConfMap["sha384"].(string)
	if !ok {
		// If we can't find a sha384 field default to "ignored", which Trident
		// will accept and just skip the hash verification.
		imgSha384 = "ignored"
	}

	return imgUrl, imgSha384, nil
}

func makeStreamImageOverrideFileDownload(tridentConfig map[string]any, logstreamAddress string) (rcpAgentFileDownload, error) {
	imgUrl, imgSha384, err := getUmageUrlAndHashFromTridentConfig(tridentConfig)
	if err != nil {
		return rcpAgentFileDownload{}, fmt.Errorf("failed to get image URL and hash from Trident config: %w", err)
	}

	fileContent := fmt.Sprintf(
		systemdServiceExecOverrideTemplate,
		fmt.Sprintf("/usr/bin/trident grpc-client stream-image %s --hash %s", imgUrl, imgSha384),
		logstreamAddress,
	)

	return newRcpAgentFileDownload(
		"trident-stream-image-override",
		"/etc/systemd/system/trident-install.service.d/override.conf",
		0644,
		[]byte(fileContent),
	), nil
}
