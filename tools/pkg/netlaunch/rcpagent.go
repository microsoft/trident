package netlaunch

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"net/http"
	"os"
	"time"
	rcpagent "tridenttools/pkg/rcp/agent"
	rcpclient "tridenttools/pkg/rcp/client"
	"tridenttools/pkg/rcp/tlscerts"

	"github.com/google/uuid"
	log "github.com/sirupsen/logrus"
)

type rcpAgentFileDownload struct {
	name        string
	destination string
	mode        os.FileMode
	data        []byte
}

func newRcpAgentFileDownload(name string, destination string, mode os.FileMode, data []byte) rcpAgentFileDownload {
	return rcpAgentFileDownload{
		name:        name,
		destination: destination,
		mode:        mode,
		data:        data,
	}
}

type rcpAgentConfigBuilder struct {
	rcpConf              *rcpagent.RcpAgentConfiguration
	mux                  *http.ServeMux
	AnnounceIp           string
	announceHttp         string
	rcpListener          *rcpclient.RcpListener
	serverAddress        string
	serverConnectionType string
}

func newRcpAgentConfigBuilder(
	mux *http.ServeMux,
	announceIp string,
	announceHttpAddress string,
	rcpListener *rcpclient.RcpListener,
	serverAddress string,
	serverConnectionType string,
) *rcpAgentConfigBuilder {
	return &rcpAgentConfigBuilder{
		rcpConf:              &rcpagent.RcpAgentConfiguration{},
		mux:                  mux,
		AnnounceIp:           announceIp,
		announceHttp:         announceHttpAddress,
		rcpListener:          rcpListener,
		serverAddress:        serverAddress,
		serverConnectionType: serverConnectionType,
	}
}

func (b *rcpAgentConfigBuilder) registerRcpFile(file rcpAgentFileDownload) {
	// generate a unique URL path for this file based on its name
	name := fmt.Sprintf("%s-%s", file.name, uuid.New().String())
	// Create an http endpoint that exclusively serves the local file
	b.mux.HandleFunc(fmt.Sprintf("/%s", name), func(w http.ResponseWriter, r *http.Request) {
		http.ServeContent(w, r, file.name, time.Now(), bytes.NewReader(file.data))
	})

	fileUrl := fmt.Sprintf("http://%s/%s", b.announceHttp, name)
	log.WithField("url", fileUrl).Infof("Serving local file '%s' via HTTP", file.name)
	b.rcpConf.AdditionalFiles = append(b.rcpConf.AdditionalFiles, rcpagent.RcpAdditionalFile{
		DownloadUrl: fileUrl,
		Destination: file.destination,
		Mode:        file.mode,
	})
}

func (b *rcpAgentConfigBuilder) startService(serviceName string) {
	log.Infof("Scheduling service '%s' to be started by RCP agent", serviceName)
	b.rcpConf.Services.Start = append(b.rcpConf.Services.Start, serviceName)
}

func (b *rcpAgentConfigBuilder) build() *rcpagent.RcpAgentConfiguration {
	if b.rcpListener != nil {
		b.rcpConf.ClientAddress = fmt.Sprintf("%s:%d", b.AnnounceIp, b.rcpListener.Port)
		b.rcpConf.ServerAddress = b.serverAddress
		b.rcpConf.ServerConnectionType = b.serverConnectionType

		// Populate TLS certs for mutual authentication
		clientCert, clientKey, serverCert := tlscerts.ClientTlsData()
		b.rcpConf.RcpClientTls = rcpagent.RcpTlsClientData{
			ClientCert: base64.StdEncoding.EncodeToString(clientCert),
			ClientKey:  base64.StdEncoding.EncodeToString(clientKey),
			ServerCert: base64.StdEncoding.EncodeToString(serverCert),
		}
	}

	return b.rcpConf
}
