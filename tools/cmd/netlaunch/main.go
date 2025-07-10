/*
Copyright © 2023 Microsoft Corporation
*/
package main

import (
	"sync"
	"tridenttools/pkg/config"
	"tridenttools/pkg/netfinder"
	"tridenttools/pkg/phonehome"
	"tridenttools/storm/utils"

	"bytes"
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/fatih/color"
	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
	"github.com/stmcginnis/gofish/redfish"
	"gopkg.in/yaml.v2"

	bmclib "github.com/bmc-toolbox/bmclib/v2"
	"github.com/google/uuid"
)

// `MagicString` is used to locate placeholder files in the initrd. Each placeholder file will be
// `PlaceholderLengthBytes` bytes long and start with this string, followed by the name
// of the file wrapped in colons. Unlike other files which may be compressed, each placeholder
// will directly have its bytes present in the output ISO so that it can be located and patched.
// This enables us to later replace the placeholder with the actual file contents without having
// to parse the ISO file format.
var MagicString = `#8505c8ab802dd717290331acd0592804c4e413b030150c53f5018ac998b7831d`

var (
	netlaunchConfigFile string
	tridentConfigFile   string
	iso                 string
	logstream           bool
	listenPort          uint16
	remoteAddressFile   string
	serveFolder         string
	maxFailures         uint
	traceFile           string
	forceColor          bool
	waitForProvisioned  bool
)

var backgroundLogstreamFull string

func patchFile(iso []byte, filename string, contents []byte) error {
	// Search for magic string
	i := bytes.Index(iso, []byte(MagicString+":"+filename+":"))
	if i == -1 {
		return errors.New("could not find magic string")
	}

	// Determine placeholder size
	placeholderLength := bytes.IndexByte(iso[i:], byte('\n')) + 1
	if len(contents) > placeholderLength {
		return errors.New("file is too big to fit in placeholder")
	}

	// If the contents are smaller than the placeholder then add a trailing newline.
	if len(contents) < placeholderLength {
		contents = append(contents, byte('\n'))
	}

	// If the contents are still smaller, then pad with # symbols and one final newline.
	if len(contents) < placeholderLength {
		padding := placeholderLength - len(contents)
		contents = append(contents, []byte(strings.Repeat("#", padding-1)+"\n")...)
	}

	copy(iso[i:], contents)

	return nil
}

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

		// Set log level
		log.SetLevel(log.DebugLevel)
	},
	Run: func(cmd *cobra.Command, args []string) {
		// Read the ISO
		iso, err := os.ReadFile(iso)
		if err != nil {
			log.WithError(err).Fatalf("failed to find iso for testing")
		}

		viper.SetConfigType("yaml")
		viper.SetConfigFile(netlaunchConfigFile)
		if err := viper.ReadInConfig(); err != nil {
			log.WithError(err).Fatal("failed to read configuration file")
		}

		config := config.NetLaunchConfig{}

		if err := viper.UnmarshalExact(&config); err != nil {
			log.WithError(err).Fatal("could not unmarshal configuration")
		}

		localListenAddress := fmt.Sprintf(":%d", listenPort)
		listen, err := net.Listen("tcp4", localListenAddress)
		if err != nil {
			log.WithError(err).Fatalf("failed to open port listening on %s", localListenAddress)
		}

		// Find the port we're listening on
		var announcePort string
		if config.Netlaunch.AnnouncePort != nil {
			announcePort = fmt.Sprintf("%d", *config.Netlaunch.AnnouncePort)
		} else {
			announcePort = strings.Split(listen.Addr().String(), ":")[1]
		}

		// Do we expect Trident to reach back? If so we need to listen to it.
		// If we have a specified port, we assume that the intent is that Trident will reach back.
		enable_phonehome_listening := listenPort != 0

		terminateCtx, terminateFunc := context.WithCancel(context.Background())
		defer terminateFunc()

		result := make(chan phonehome.PhoneHomeResult)
		server := &http.Server{}

		// Create the final address that will be announced to the BMC and Trident.
		var announceIp string
		if config.Netlaunch.AnnounceIp != nil {
			// If an IP is specified, use it.
			announceIp = *config.Netlaunch.AnnounceIp
		} else {
			// Otherwise, try to be clever...
			// We need to find the IP of the local interface that can reach the BMC.
			log.Warn("No announce IP specified. Attempting to find local IP to announce based on BMC IP.")
			announceIp, err = netfinder.FindLocalIpForTargetIp(config.Netlaunch.Bmc.Ip)
			if err != nil {
				log.WithError(err).Fatalf("failed to find local IP for BMC")
			}
		}

		announceAddress := fmt.Sprintf("%s:%s", announceIp, announcePort)
		log.WithField("address", announceAddress).Info("Announcing address")

		// A flag to record if we've already logged the ISO being fetched by the
		// BMC. We only want to log this once.
		var isoFetcheLog sync.Once
		var isoLogFunc = func(address string) {
			isoFetcheLog.Do(func() {
				log.WithField("address", address).Info("BMC has requested the ISO!")
			})
		}

		// If we have a Trident config file, we need to patch it into the ISO.
		if len(tridentConfigFile) != 0 {
			log.Info("Using Trident config file: ", tridentConfigFile)
			tridentConfigContents, err := os.ReadFile(tridentConfigFile)
			if err != nil {
				log.WithError(err).Fatalf("failed to read Trident config")
			}

			// Replace NETLAUNCH_HOST_ADDRESS with the address of the netlaunch server
			tridentConfigContentsStr := strings.ReplaceAll(string(tridentConfigContents), "NETLAUNCH_HOST_ADDRESS", announceAddress)

			trident := make(map[string]interface{})
			err = yaml.UnmarshalStrict([]byte(tridentConfigContentsStr), &trident)
			if err != nil {
				log.WithError(err).Fatalf("failed to unmarshal Trident config")
			}

			if _, ok := trident["trident"]; !ok {
				trident["trident"] = make(map[interface{}]interface{})
			}
			trident["trident"].(map[interface{}]interface{})["phonehome"] = fmt.Sprintf("http://%s/phonehome", announceAddress)
			trident["trident"].(map[interface{}]interface{})["logstream"] = fmt.Sprintf("http://%s/logstream", announceAddress)

			tridentConfig, err := yaml.Marshal(trident)
			if err != nil {
				log.WithError(err).Fatalf("failed to marshal Trident config")
			}

			err = patchFile(iso, "/etc/trident/config.yaml", tridentConfig)
			if err != nil {
				log.WithError(err).Fatalf("failed to patch Trident config into ISO")
			}

			if config.Iso.PreTridentScript != nil {
				log.Info("Patching in pre-trident script!")
				err = patchFile(iso, "/trident_cdrom/pre-trident-script.sh", []byte(*config.Iso.PreTridentScript))
				if err != nil {
					log.WithError(err).Fatalf("failed to patch pre-trident script into ISO")
				}
			}

			http.HandleFunc("/provision.iso", func(w http.ResponseWriter, r *http.Request) {
				isoLogFunc(r.RemoteAddr)
				http.ServeContent(w, r, "provision.iso", time.Now(), bytes.NewReader(iso))
			})

			// We injected the phonehome & logstream config, so we're expecting Trident to reach back
			enable_phonehome_listening = true
		} else {
			// Otherwise, serve the iso as-is
			http.HandleFunc("/provision.iso", func(w http.ResponseWriter, r *http.Request) {
				isoLogFunc(r.RemoteAddr)
				http.ServeContent(w, r, "provision.iso", time.Now(), bytes.NewReader(iso))
				terminateFunc()
			})
		}

		// If we're expecting Trident to reach back, we need to listen for it.
		if enable_phonehome_listening {
			// Set up listening for phonehome
			phonehome.SetupPhoneHomeServer(result, remoteAddressFile)

			// Set up listening for logstream
			logstreamFull, err := phonehome.SetupLogstream(backgroundLogstreamFull)
			if err != nil {
				log.WithError(err).Fatalf("failed to setup logstream")
			}
			defer logstreamFull.Close()

			// Set up listening for tracestream
			traceFile, err := phonehome.SetupTraceStream(traceFile)
			if err != nil {
				log.WithError(err).Fatalf("failed to setup tracestream")
			}
			defer traceFile.Close()

		}

		if len(serveFolder) != 0 {
			http.Handle("/files/", http.StripPrefix("/files/", http.FileServer(http.Dir(serveFolder))))
		}

		// Start the HTTP server
		go server.Serve(listen)
		log.WithField("address", listen.Addr().String()).Info("Listening...")
		iso_location := fmt.Sprintf("http://%s/provision.iso", announceAddress)

		if config.Netlaunch.LocalVmUuid != nil {
			startLocalVm(*config.Netlaunch.LocalVmUuid, iso_location)
		} else {
			if config.Netlaunch.Bmc != nil && config.Netlaunch.Bmc.SerialOverSsh != nil {
				serial, err := config.Netlaunch.Bmc.ListenForSerialOutput()
				if err != nil {
					log.WithError(err).Fatalf("Failed to open serial over SSH session")
				}
				defer serial.Close()
			}
			// Deploy ISO to BMC

			// Default to port 443
			port := "443"
			if config.Netlaunch.Bmc.Port != nil {
				port = *config.Netlaunch.Bmc.Port
			}

			client := bmclib.NewClient(
				config.Netlaunch.Bmc.Ip,
				config.Netlaunch.Bmc.Username,
				config.Netlaunch.Bmc.Password,
				bmclib.WithRedfishPort(port),
			)

			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
			defer cancel()

			log.Info("Connecting to BMC")
			client.Registry.Drivers = client.Registry.For("gofish")
			if err := client.Open(context.Background()); err != nil {
				log.WithError(err).Fatalf("failed to open connection to BMC")
			}

			log.Info("Shutting down machine")
			if _, err = client.SetPowerState(ctx, "off"); err != nil {
				log.WithError(err).Fatalf("failed to turn off machine")
			}

			log.WithField("url", iso_location).Info("Setting virtual media to ISO")
			if _, err = client.SetVirtualMedia(ctx, string(redfish.CDMediaType), iso_location); err != nil {
				log.WithError(err).Fatalf("failed to set virtual media")
			}

			log.Info("Setting boot media")
			if _, err = client.SetBootDevice(ctx, "cdrom", false, true); err != nil {
				log.WithError(err).Fatalf("failed to set boot media")
			}

			log.Info("Turning on machine")
			if _, err = client.SetPowerState(ctx, "on"); err != nil {
				log.WithError(err).Fatalf("failed to turn on machine")
			}
		}

		log.Info("ISO deployed!")

		if len(tridentConfigFile) != 0 {
			log.Info("Waiting for phone home...")
		}

		// Wait for something to happen
		var exitCode = phonehome.ListenLoop(terminateCtx, result, waitForProvisioned, maxFailures)

		err = server.Shutdown(context.Background())
		if err != nil {
			log.WithError(err).Errorln("failed to shutdown server")
		}

		os.Exit(exitCode)
	},
}

func startLocalVm(localVmUuidStr string, isoLocation string) {
	log.Info("Using local VM")

	// TODO: Parse the UUID directly when reading the config file
	vmUuid, err := uuid.Parse(localVmUuidStr)
	if err != nil {
		log.WithError(err).Fatalf("failed to parse LocalVmUuid as UUID")
	}

	vm, err := utils.InitializeVm(vmUuid)
	if err != nil {
		log.WithError(err).Fatalf("failed to initialize VM")
	}
	defer vm.Disconnect()

	if err = vm.SetVmHttpBootUri(isoLocation); err != nil {
		log.WithError(err).Fatalf("failed to set VM HTTP boot URI")
	}

	if err = vm.Start(); err != nil {
		log.WithError(err).Fatalf("failed to start VM")
	}
}

func init() {
	rootCmd.PersistentFlags().StringVarP(&netlaunchConfigFile, "config", "c", "netlaunch.yaml", "Netlaunch config file")
	rootCmd.PersistentFlags().StringVarP(&tridentConfigFile, "trident", "t", "", "Trident local config file")
	rootCmd.PersistentFlags().BoolVarP(&logstream, "logstream", "l", false, "Enable log streaming. (Requires --trident || --port)")
	rootCmd.PersistentFlags().Uint16VarP(&listenPort, "port", "p", 0, "Port to listen on for logstream & phonehome. Random if not specified.")
	rootCmd.PersistentFlags().StringVarP(&remoteAddressFile, "remoteaddress", "r", "", "File for writing remote address of the Trident instance.")
	rootCmd.PersistentFlags().StringVarP(&serveFolder, "servefolder", "s", "", "Optional folder to serve files from at /files")
	rootCmd.PersistentFlags().UintVarP(&maxFailures, "max-failures", "e", 0, "Maximum number of failures allowed before terminating. Default 0: no failures are tolerated.")
	rootCmd.PersistentFlags().StringVarP(&traceFile, "trace-file", "m", "", "File for writing metrics collected from Trident.")
	rootCmd.PersistentFlags().StringVarP(&backgroundLogstreamFull, "full-logstream", "b", "logstream-full.log", "File to write full logstream output to. (Requires -l)")
	rootCmd.PersistentFlags().BoolVarP(&waitForProvisioned, "wait-for-provisioned-state", "", false, "Wait for Host Status servicingState to be 'provisioned'")
	rootCmd.PersistentFlags().BoolVarP(&forceColor, "force-color", "", false, "Force colored output.")
	rootCmd.Flags().StringVarP(&iso, "iso", "i", "", "ISO for Netlaunch testing.")
	rootCmd.MarkFlagRequired("iso-template")
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
