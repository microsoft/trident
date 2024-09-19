/*
Copyright © 2023 Microsoft Corporation

netlisten: A tool to listen for phonehome and logstream requests from Trident.

CLI usage: netlisten -p <port>

Instructions:
Pick a port to listen on. This port will be used for both phonehome and logstream requests.

Populate the phonehome and netlisten urls in the Trident config file with the address of
the machine running netlisten.

Example:

```yaml
phonehome: http://<netlisten_address>:<port>/phonehome
logstream: http://<netlisten_address>:<port>/logstream
```

Start the netlisten server by running `netlisten -p <port>`.
Then start the provisioning using the patched trident config file.
*/
package main

import (
	"argus_toolkit/pkg/phonehome"
	"fmt"
	"net"

	"context"
	"net/http"
	"os"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
)

var listen_port uint16
var serveFolder string
var logTrace bool
var forceColor bool

// Filepath to store metrics from Trident
var TRIDENT_METRICS_PATH = "trident-metrics.jsonl"

var rootCmd = &cobra.Command{
	Use:   "netlisten",
	Short: "Trident Phonehome Server",
	PreRun: func(cmd *cobra.Command, args []string) {
		if listen_port == 0 {
			log.Fatal("A port must be specified")
		}

		// Set log level
		if logTrace {
			log.SetLevel(log.TraceLevel)
			log.Traceln("Trace logging enabled!")
		} else {
			log.SetLevel(log.DebugLevel)
		}

		if forceColor {
			log.SetFormatter(&log.TextFormatter{
				ForceColors: true,
			})
		}
	},
	Run: func(cmd *cobra.Command, args []string) {
		address := fmt.Sprintf("0.0.0.0:%d", listen_port)
		listen, err := net.Listen("tcp4", address)
		if err != nil {
			log.WithError(err).Fatalf("failed to open port listening on %s", address)
		}

		// Set up listening
		result := make(chan phonehome.PhoneHomeResult)
		server := &http.Server{}

		// Set up listening for phonehome
		phonehome.SetupPhoneHomeServer(result, "")
		// Set up listening for logstream
		phonehome.SetupLogstream()
		// Set up listening for tracestream
		phonehome.SetupTraceStream(TRIDENT_METRICS_PATH)

		if len(serveFolder) != 0 {
			http.Handle("/files/", http.StripPrefix("/files/", http.FileServer(http.Dir(serveFolder))))
		}

		// Start the HTTP server
		go server.Serve(listen)
		log.WithField("address", listen.Addr().String()).Info("Listening...")

		log.Info("Waiting for phone home...")

		// Wait for done signal
		var res = <-result

		// HACK: Ignore the first failure from phonehome to support the 'rerun'
		// E2E test. It would be better to use a 'maxFailures' parameter like
		// netlaunch does, but that's a more invasive change.
		if res.State == phonehome.PhoneHomeResultFailure {
			res = <-result
		}

		// Log the result
		res.Log()

		server.Shutdown(context.Background())

		os.Exit(res.ExitCode())
	},
}

func init() {
	rootCmd.PersistentFlags().Uint16VarP(&listen_port, "port", "p", 0, "Port to listen on for logstream & phonehome. Random if not specified.")
	rootCmd.PersistentFlags().StringVarP(&serveFolder, "servefolder", "s", "", "Optional folder to serve files from at /files")
	rootCmd.PersistentFlags().BoolVarP(&logTrace, "log-trace", "", false, "Enable trace level logs.")
	rootCmd.PersistentFlags().BoolVarP(&forceColor, "force-color", "", false, "Force colored output.")
	rootCmd.MarkFlagRequired("port")
	log.SetLevel(log.DebugLevel)
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
