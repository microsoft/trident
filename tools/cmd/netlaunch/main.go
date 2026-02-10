/*
Copyright © 2023 Microsoft Corporation
*/
package main

import (
	"context"
	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/phonehome"

	"fmt"
	"os"

	"github.com/fatih/color"
	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

var (
	netlaunchConfigFile  string
	tridentConfigFile    string
	iso                  string
	logstream            bool
	listenPort           uint16
	remoteAddressFile    string
	serveFolder          string
	maxFailures          uint
	traceFile            string
	forceColor           bool
	waitForProvisioned   bool
	onlyPrintExitCode    bool
	secureBoot           bool
	signingCert          string
	rcpMode              string
	tridentBinaryPath    string
	osmodifierBinaryPath string
	streamImage          bool
)

const (
	rcpModeLegacy = "cli"
	rcpModeGrpc   = "grpc"
)

var backgroundLogstreamFull string

var rootCmd = &cobra.Command{
	Use: "netlaunch",
	Short: "Launch a BMC boot\n\n" +
		"When a Trident configuration is passed, the ISO will be patched with the Trident configuration.\n" +
		"Netlaunch supports replacing the string `NETLAUNCH_HOST_ADDRESS` in the Trident configuration with the address of the netlaunch server.\n" +
		"E.g. `http://NETLAUNCH_HOST_ADDRESS/url/path` will be replaced with `http://<IP>:<port>/url/path`.",
	PreRun: func(cmd *cobra.Command, args []string) {
		if len(iso) == 0 {
			log.Fatal("ISO file not specified")
		}

		// To enable logstream, we need either:
		// - A specified port
		// - A Trident config file (so that we can patch in the assigned port)
		if logstream && listenPort == 0 && len(tridentConfigFile) == 0 {
			log.Fatal("logstream requires a specified port or Trident config file")
		}

		if forceColor {
			log.SetFormatter(&log.TextFormatter{
				ForceColors: true,
			})

			// Force color to be enabled
			color.NoColor = false
		}

		if rcpMode != "" {
			log.Infof("Using RCP mode: %s", rcpMode)
			if rcpMode != rcpModeGrpc && rcpMode != rcpModeLegacy {
				log.Fatalf("Invalid RCP mode, must be: %s or %s, got: %s", rcpModeLegacy, rcpModeGrpc, rcpMode)
			}
		} else {
			if tridentBinaryPath != "" {
				log.Fatal("Trident binary path specified without RCP mode")
			}
			if osmodifierBinaryPath != "" {
				log.Fatal("Osmodifier binary path specified without RCP mode")
			}
			if streamImage {
				log.Fatal("Stream image specified without RCP mode")
			}
		}

		// Set log level
		log.SetLevel(log.DebugLevel)
	},
	Run: func(cmd *cobra.Command, args []string) {
		viper.SetConfigType("yaml")
		viper.SetConfigFile(netlaunchConfigFile)
		if err := viper.ReadInConfig(); err != nil {
			log.WithError(err).Fatal("failed to read configuration file")
		}

		config := netlaunch.NetLaunchConfig{}

		if err := viper.UnmarshalExact(&config); err != nil {
			log.WithError(err).Fatal("could not unmarshal configuration")
		}

		config.IsoPath = iso
		config.ListenPort = listenPort
		config.HostConfigFile = tridentConfigFile
		config.RemoteAddressFile = remoteAddressFile
		config.LogstreamFile = backgroundLogstreamFull
		config.TracestreamFile = traceFile
		config.ServeDirectory = serveFolder
		config.CertificateFile = signingCert
		config.EnableSecureBoot = secureBoot
		config.WaitForProvisioning = waitForProvisioned
		config.MaxPhonehomeFailures = maxFailures

		if rcpMode != "" {
			config.Rcp = &netlaunch.RcpConfiguration{
				GrpcMode: rcpMode == rcpModeGrpc,
			}

			if tridentBinaryPath != "" {
				config.Rcp.LocalTridentPath = &tridentBinaryPath
			}

			if osmodifierBinaryPath != "" {
				config.Rcp.LocalOsmodifierPath = &osmodifierBinaryPath
			}

			config.Rcp.UseStreamImage = streamImage
		}

		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()

		err := netlaunch.RunNetlaunch(ctx, &config)

		// Get an exit code based on the error and log it
		var exitCode int = phonehome.GetExitCodeFromErrorAndLog(err)

		fmt.Printf("Phone home exited: %d\n", exitCode)
		if onlyPrintExitCode {
			// Only print the exit code and exit
			return
		}

		os.Exit(exitCode)
	},
}

func init() {
	rootCmd.PersistentFlags().StringVarP(&netlaunchConfigFile, "config", "c", "netlaunch.yaml", "Netlaunch config file")
	rootCmd.PersistentFlags().StringVarP(&tridentConfigFile, "trident", "t", "", "Trident local config file")
	rootCmd.PersistentFlags().BoolVarP(&logstream, "logstream", "l", false, "Enable log streaming. (Requires --trident || --port)")
	rootCmd.PersistentFlags().Uint16VarP(&listenPort, "port", "p", 0, "Port to listen on for logstream & phonehome. Random if not specified")
	rootCmd.PersistentFlags().StringVarP(&remoteAddressFile, "remoteaddress", "r", "", "File for writing remote address of the Trident instance")
	rootCmd.PersistentFlags().StringVarP(&serveFolder, "servefolder", "s", "", "Optional folder to serve files from at /files")
	rootCmd.PersistentFlags().UintVarP(&maxFailures, "max-failures", "e", 0, "Maximum number of failures allowed before terminating. Default 0: no failures are tolerated")
	rootCmd.PersistentFlags().StringVarP(&traceFile, "trace-file", "m", "", "File for writing metrics collected from Trident.")
	rootCmd.PersistentFlags().StringVarP(&backgroundLogstreamFull, "full-logstream", "b", "logstream-full.log", "File to write full logstream output to. (Requires -l)")
	rootCmd.PersistentFlags().BoolVarP(&waitForProvisioned, "wait-for-provisioned-state", "", false, "Wait for Host Status servicingState to be 'provisioned'")
	rootCmd.PersistentFlags().BoolVarP(&onlyPrintExitCode, "only-print-exit-code", "", false, "Only print the exit code")
	rootCmd.PersistentFlags().BoolVarP(&forceColor, "force-color", "", false, "Force colored output")
	rootCmd.PersistentFlags().BoolVarP(&secureBoot, "secure-boot", "", false, "Enable SecureBoot")
	rootCmd.PersistentFlags().StringVarP(&signingCert, "signing-cert", "", "", "Path to signing certificate")
	rootCmd.PersistentFlags().StringVarP(&rcpMode, "rcp-agent-mode", "", "", "RCP agent mode to use (grpc|cli). If not specified, the rcp-agent is not used.")
	rootCmd.PersistentFlags().StringVarP(&tridentBinaryPath, "trident-binary", "", "", "Optional path to Trident binary to be copied into the VM, requires RCP mode.")
	rootCmd.PersistentFlags().StringVarP(&osmodifierBinaryPath, "osmodifier-binary", "", "", "Optional path to Osmodifier binary to be copied into the VM, requires RCP mode.")
	rootCmd.PersistentFlags().BoolVarP(&streamImage, "stream-image", "", false, "Use stream image for installation instead of the default method, requires RCP mode.")
	rootCmd.Flags().StringVarP(&iso, "iso", "i", "", "ISO for Netlaunch testing")
	rootCmd.MarkFlagRequired("iso-template")
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
