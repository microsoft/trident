package phonehome

import (
	"net/http"
	"os"
	"strings"

	"github.com/pkg/errors"
	log "github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type OrchestratorMessage struct {
	State       string
	Message     string
	Host_Status string
}

type PhoneHomeResult struct {
	State      PhoneHomeResultState `json:"state"`
	Message    string               `json:"message"`
	HostStatus string               `json:"host_status"`
}

func (result *PhoneHomeResult) Log() {
	if result.State == PhoneHomeResultFailure {
		log.Errorf("Trident failed to deploy target OS with error:\n%s", result.Message)
	} else if result.State == PhoneHomeResultSuccess {
		log.Info("Trident successfully deployed target OS")
	} else if result.State == PhoneHomeResultError {
		log.Errorf("Logstream had an error:\n%s", result.Message)
	}
}

func (result *PhoneHomeResult) ExitCode() int {
	if result.State == PhoneHomeResultSuccess {
		return 0
	} else if result.State == PhoneHomeResultFailure {
		// Two is the specific error code for Trident failure
		return 2
	} else {
		// One is the generic error code
		return 1
	}
}

func errorPhoneHomeResult(err error) PhoneHomeResult {
	return PhoneHomeResult{
		State:   PhoneHomeResultError,
		Message: err.Error(),
	}
}

type PhoneHomeResultState string

const (
	// Received a success state
	PhoneHomeResultSuccess PhoneHomeResultState = "succeeded"

	// Received a failure state
	PhoneHomeResultFailure PhoneHomeResultState = "failed"

	// Some error occurred
	PhoneHomeResultError PhoneHomeResultState = "error"
)

func SetupPhoneHomeServer(result chan<- PhoneHomeResult, remoteAddressFile string) {
	http.HandleFunc("/phonehome", func(w http.ResponseWriter, r *http.Request) {
		// log.WithField("remote-address", r.RemoteAddr).Info("Phone Home")
		w.WriteHeader(201)
		w.Write([]byte("OK"))

		var message OrchestratorMessage
		err := yaml.NewDecoder(r.Body).Decode(&message)
		if err != nil {
			result <- errorPhoneHomeResult(errors.Wrap(err, "failed to decode phone home message"))
			return
		}

		if message.State == "started" {
			log.Infof("Trident connected from %s", r.RemoteAddr)
			if remoteAddressFile != "" {
				// write the remote address to the address file
				err := os.WriteFile(remoteAddressFile, []byte(strings.Split(r.RemoteAddr, ":")[0]), 0644)
				if err != nil {
					result <- errorPhoneHomeResult(errors.Wrap(err, "failed to write remote address"))
					return
				}
			}
		}

		if message.Host_Status != "" {
			log.Infof("Reported host Status:\n%s", message.Host_Status)
		}

		if message.State == string(PhoneHomeResultFailure) {
			result <- PhoneHomeResult{
				State:      PhoneHomeResultFailure,
				Message:    message.Message,
				HostStatus: message.Host_Status,
			}
		} else if message.State == string(PhoneHomeResultSuccess) {
			result <- PhoneHomeResult{
				State:      PhoneHomeResultSuccess,
				Message:    message.Message,
				HostStatus: message.Host_Status,
			}
		} else {
			log.WithField("state", message.State).WithField("host_status", message.Host_Status).Info(message.Message)
		}
	})
}
