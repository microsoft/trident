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
Then start the provisioning using the patched Trident config file.
*/
package main

import (
	"fmt"
	"net"
	"os/signal"
	"syscall"
	"tridenttools/pkg/config"
	"tridenttools/pkg/phonehome"

	"context"
	"net/http"
	"os"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

var listen_port uint16
var serveFolder string
var forceColor bool
var backgroundLogstreamFull string
var traceFile string
var netlistenConfigFile string

var rootCmd = &cobra.Command{
	Use:   "netlisten",
	Short: "Trident Phonehome Server",
	PreRun: func(cmd *cobra.Command, args []string) {
		if listen_port == 0 {
			log.Fatal("A port must be specified")
		}

		// Set log level
		log.SetLevel(log.DebugLevel)

		if forceColor {
			log.SetFormatter(&log.TextFormatter{
				ForceColors: true,
			})
		}
	},
	Run: func(cmd *cobra.Command, args []string) {
		// Create a context that can be cancelled
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel() // Ensure resources are released

		// Handle signals
		sigChan := make(chan os.Signal, 1)
		signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

		go func() {
			sig := <-sigChan
			log.WithField("signal", sig).Warn("Received signal, shutting down...")
			cancel() // Cancel the context when a signal is received
		}()

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
		logstreamFull, err := phonehome.SetupLogstream(backgroundLogstreamFull)
		if err != nil {
			log.WithError(err).Fatalf("failed to set up logstream")
		}
		defer logstreamFull.Close()

		// Set up listening for tracestream
		traceFile, err := phonehome.SetupTraceStream(traceFile)
		if err != nil {
			log.WithError(err).Fatalf("failed to set up trace stream")
		}
		defer traceFile.Close()

		if len(serveFolder) != 0 {
			http.Handle("/files/", http.StripPrefix("/files/", http.FileServer(http.Dir(serveFolder))))
		}

		if netlistenConfigFile != "" {
			go func() {
				viper.SetConfigType("yaml")
				viper.SetConfigFile(netlistenConfigFile)
				if err := viper.ReadInConfig(); err != nil {
					log.WithError(err).Fatal("failed to read configuration file")
				}

				config := config.NetListenConfig{}
				if err := viper.UnmarshalExact(&config); err != nil {
					log.WithError(err).Fatal("could not unmarshal configuration")
				}
				if config.Netlisten.Bmc != nil && config.Netlisten.Bmc.SerialOverSsh != nil {
					serial, err := config.Netlisten.Bmc.ListenForSerialOutput()
					if err != nil {
						log.WithError(err).Fatalf("Failed to open serial over SSH session")
					}
					defer serial.Close()

					// Wait for context cancellation
					<-ctx.Done()
				}
			}()
		}

		// Start the HTTP server
		go server.Serve(listen)
		log.WithField("address", listen.Addr().String()).Info("Listening...")

		log.Info("Waiting for phone home...")

		// Wait for done signal.
		// HACK: Ignore the first failure from phonehome to support the 'rerun'
		// E2E test.
		exitCode := phonehome.ListenLoop(ctx, result, false, 1, false)

		log.Info("Shutting down server...")
		server.Shutdown(context.Background())
		log.Info("Server shut down")

		os.Exit(exitCode)
	},
}

func init() {
	rootCmd.PersistentFlags().Uint16VarP(&listen_port, "port", "p", 0, "Port to listen on for logstream & phonehome. Random if not specified.")
	rootCmd.PersistentFlags().StringVarP(&serveFolder, "servefolder", "s", "", "Optional folder to serve files from at /files")
	rootCmd.PersistentFlags().BoolVarP(&forceColor, "force-color", "", false, "Force colored output.")
	rootCmd.PersistentFlags().StringVarP(&backgroundLogstreamFull, "full-logstream", "b", "logstream-full.log", "File to write full logstream output to.")
	rootCmd.PersistentFlags().StringVarP(&traceFile, "trace-file", "m", "trident-metrics.jsonl", "File for writing metrics collected from Trident. Defaults to trident-metrics.jsonl")
	rootCmd.PersistentFlags().StringVarP(&netlistenConfigFile, "config", "c", "", "Optional netlisten config file")
	rootCmd.MarkFlagRequired("port")
	log.SetLevel(log.DebugLevel)
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
