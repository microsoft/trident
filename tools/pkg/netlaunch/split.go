package netlaunch

import (
	"fmt"

	log "github.com/sirupsen/logrus"
)

const systemdSplitServiceExecOverrideTemplate = `
[Service]
ExecStart=
ExecStart=/usr/bin/trident install --allowed-operations stage
ExecStart=/usr/bin/trident install --allowed-operations finalize
StandardOutput=journal+console
StandardError=journal+console
Environment="LOGSTREAM_URL=%s"
`

func makeSplitOverrideFileDownload(tridentConfig map[string]any, logstreamAddress string) (rcpAgentFileDownload, error) {
	fileContent := fmt.Sprintf(
		systemdSplitServiceExecOverrideTemplate,
		logstreamAddress,
	)

	log.Infof("Generated split override file content:\n%s", fileContent)

	return newRcpAgentFileDownload(
		"trident-split-override",
		"/etc/systemd/system/trident-install.service.d/override.conf",
		0644,
		[]byte(fileContent),
	), nil
}
