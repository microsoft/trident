package phonehome

import (
	"net/http"
	"os"
	"strings"

	log "github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type OrchestratorMessage struct {
	State   string
	Message string
}

func SetupPhoneHomeServer(done chan<- bool, remoteAddressFile string) {
	http.HandleFunc("/phonehome", func(w http.ResponseWriter, r *http.Request) {
		// log.WithField("remote-address", r.RemoteAddr).Info("Phone Home")
		w.WriteHeader(201)
		w.Write([]byte("OK"))

		var message OrchestratorMessage
		err := yaml.NewDecoder(r.Body).Decode(&message)
		if err != nil {
			log.WithError(err).Fatalf("failed to decode phone home message")
			done <- true
			return
		}

		if message.State == "started" {
			log.Infof("Trident connected from %s", r.RemoteAddr)
			if remoteAddressFile != "" {
				// write the remote address to the address file
				err := os.WriteFile(remoteAddressFile, []byte(strings.Split(r.RemoteAddr, ":")[0]), 0644)
				if err != nil {
					log.WithError(err).Fatalf("Failed to write address file")
					done <- true
					return
				}
			}
		}

		if message.State == "failed" {
			log.Fatalf("Trident failed to deploy Runtime OS with message:\n%s", message.Message)
		}

		log.WithField("state", message.State).Info(message.Message)
		if message.State == "succeeded" {
			done <- true
		}
	})
}
