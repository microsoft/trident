package phonehome

import (
	"context"

	log "github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

// ListenLoop listens for phonehome results and logs them.
// If a result file is specified, it writes the result to that file.
// If the result indicates that we should terminate, it returns.
// If the terminate channel receives something, it returns.
func ListenLoop(ctx context.Context, result <-chan PhoneHomeResult, waitForProvisioned bool, maxFailures uint, onlyPrintExitCode bool) int {
	failureCount := uint(0)

	// Loop forever!
	for {
		// Wait for something to happen
		select {

		case <-ctx.Done():
			// If we're told to terminate, then we're done.
			return 0

		case result := <-result:
			// If we get a result log it.
			result.Log()

			// Check the state of the result.
			switch result.State {
			case PhoneHomeResultFailure:
				// If we failed, increment the failure count.
				failureCount++
			default:
				if !waitForProvisioned {
					// First successful phonehome message should return the exit code.
					return result.ExitCode()
				}

				var hostStatus map[string]interface{}
				err := yaml.Unmarshal([]byte(result.HostStatus), &hostStatus)
				if err != nil {
					log.Infof("Failed to parse phonehome Host Status: %v", err)
					// Increment the failure count.
					failureCount++
				} else if hostStatus["servicingState"] == "provisioned" {
					// Only phonehome message with provisioned servicingState should
					// return the exit code.
					return result.ExitCode()
				}
			}

			// Check if we've exceeded the maximum number of failures.
			if failureCount > maxFailures {
				log.Errorf("Maximum number of failures (%d) exceeded. Terminating.", maxFailures)
				return result.ExitCode()
			}
		}
	}
}
