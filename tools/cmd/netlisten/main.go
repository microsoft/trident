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

var listen_port int16

var rootCmd = &cobra.Command{
	Use:   "netlisten",
	Short: "Trident Phonehome Server",
	PreRun: func(cmd *cobra.Command, args []string) {
		if listen_port == 0 {
			log.Fatal("A port must be specified")
		}
	},
	Run: func(cmd *cobra.Command, args []string) {
		address := fmt.Sprintf("0.0.0.0:%d", listen_port)
		listen, err := net.Listen("tcp4", address)
		if err != nil {
			log.WithError(err).Fatalf("failed to open port listening on %s", address)
		}

		// Set up listening
		done := make(chan bool)
		server := &http.Server{}

		// Set up listening for phonehome
		phonehome.SetupPhoneHomeServer(done, "", false)
		// Set up listening for logstream
		phonehome.SetupLogstream()

		// Start the HTTP server
		go server.Serve(listen)
		log.WithField("address", listen.Addr().String()).Info("Listening...")

		log.Info("Waiting for phone home...")

		// Wait for done signal
		<-done
		server.Shutdown(context.Background())
	},
}

func init() {
	rootCmd.PersistentFlags().Int16VarP(&listen_port, "port", "p", 0, "Port to listen on for logstream & phonehome. Random if not specified.")
	rootCmd.MarkFlagRequired("port")
	log.SetLevel(log.DebugLevel)
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
