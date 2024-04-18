/*
Copyright © 2023 Microsoft Corporation
*/
package main

import (
	"argus_toolkit/pkg/phonehome"

	"bytes"
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"time"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
	"gopkg.in/yaml.v2"

	bmclib "github.com/bmc-toolbox/bmclib/v2"
)

// `MagicString` is used to locate placeholder files in the initrd. Each placeholder file will be
// `PlaceholderLengthBytes` bytes long and start with this string, followed by the name
// of the file wrapped in colons. Unlike other files which may be compressed, each placeholder
// will directly have its bytes present in the output ISO so that it can be located and patched.
// This enables us to later replace the placeholder with the actual file contents without having
// to parse the ISO file format.
var MagicString = `#8505c8ab802dd717290331acd0592804c4e413b030150c53f5018ac998b7831d`

type NetLaunchConfig struct {
	Netlaunch struct {
		PublicIp string
		Bmc      struct {
			Ip       string
			Username string
			Password string
		}
	}
}

var netlaunchConfigFile string
var tridentConfigFile string
var iso string
var logstream bool
var listen_port int16
var remoteAddressFile string
var serveFolder string
var ignoreFailure bool
var traceFile string

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
		"When a trident configuration is passed, the ISO will be patched with the trident configuration.\n" +
		"Netlaunch supports replacing the string `NETLAUNCH_HOST_ADDRESS` in the trident configuration with the address of the netlaunch server.\n" +
		"E.g. `http://NETLAUNCH_HOST_ADDRESS/url/path` will be replaced with `http://<IP>:<port>/url/path`.",
	PreRun: func(cmd *cobra.Command, args []string) {
		if len(iso) == 0 {
			log.Fatal("ISO file not specified")
		}

		if logstream && listen_port == 0 && len(tridentConfigFile) == 0 {
			log.Fatal("logstream requires a specified port or trident config file")
		}
	},
	Run: func(cmd *cobra.Command, args []string) {
		viper.SetConfigType("yaml")
		viper.SetConfigFile(netlaunchConfigFile)
		if err := viper.ReadInConfig(); err != nil {
			log.WithError(err).Fatal("failed to read configuration file")
		}

		config := NetLaunchConfig{}

		if err := viper.UnmarshalExact(&config); err != nil {
			log.WithError(err).Fatal("could not unmarshal configuration")
		}

		address := fmt.Sprintf("%s:%d", config.Netlaunch.PublicIp, listen_port)
		listen, err := net.Listen("tcp4", address)
		if err != nil {
			log.WithError(err).Fatalf("failed to open port listening on %s", address)
		}

		iso, err := os.ReadFile(iso)
		if err != nil {
			log.WithError(err).Fatalf("failed to find iso for testing")
		}

		// Do we expect trident to reach back? If so we need to listen to it.
		// If we have a specified port, we assume that the intent is that trident will reach back.
		enable_phonehome_listening := listen_port != 0

		done := make(chan bool)
		server := &http.Server{}

		// If we have a trident config file, we need to patch it into the ISO.
		if len(tridentConfigFile) != 0 {
			log.Info("Using Trident config file: ", tridentConfigFile)
			tridentConfigContents, err := os.ReadFile(tridentConfigFile)
			if err != nil {
				log.WithError(err).Fatalf("failed to read Trident config")
			}

			// Replace NETLAUNCH_HOST_ADDRESS with the address of the netlaunch server
			tridentConfigContentsStr := strings.ReplaceAll(string(tridentConfigContents), "NETLAUNCH_HOST_ADDRESS", listen.Addr().String())

			trident := make(map[string]interface{})
			err = yaml.UnmarshalStrict([]byte(tridentConfigContentsStr), &trident)
			if err != nil {
				log.WithError(err).Fatalf("failed to unmarshal Trident config")
			}

			trident["phonehome"] = fmt.Sprintf("http://%s/phonehome", listen.Addr().String())

			if logstream {
				trident["logstream"] = fmt.Sprintf("http://%s/logstream", listen.Addr().String())
			}

			tridentConfig, err := yaml.Marshal(trident)
			if err != nil {
				log.WithError(err).Fatalf("failed to marshal trident config")
			}

			err = patchFile(iso, "/etc/trident/config.yaml", tridentConfig)
			if err != nil {
				log.WithError(err).Fatalf("failed to patch trident config into ISO")
			}

			http.HandleFunc("/provision.iso", func(w http.ResponseWriter, r *http.Request) {
				http.ServeContent(w, r, "provision.iso", time.Now(), bytes.NewReader(iso))
			})

			// We injected the phonehome & logstream config, so we're expecting trident to reach back
			enable_phonehome_listening = true
		} else {
			// Otherwise, serve the iso as-is
			http.HandleFunc("/provision.iso", func(w http.ResponseWriter, r *http.Request) {
				http.ServeContent(w, r, "provision.iso", time.Now(), bytes.NewReader(iso))
				done <- true
			})
		}

		// If we're expecting trident to reach back, we need to listen for it.
		if enable_phonehome_listening {
			// Set up listening for phonehome
			phonehome.SetupPhoneHomeServer(done, remoteAddressFile, ignoreFailure)

			// Set up listening for logstream
			phonehome.SetupLogstream()

			// Set up listening for tracestream
			phonehome.SetupTraceStream(traceFile)

		}

		if len(serveFolder) != 0 {
			http.Handle("/files/", http.StripPrefix("/files/", http.FileServer(http.Dir(serveFolder))))
		}

		// Start the HTTP server
		go server.Serve(listen)
		log.WithField("address", listen.Addr().String()).Info("Listening...")

		// Deploy ISO to BMC
		client := bmclib.NewClient(
			config.Netlaunch.Bmc.Ip,
			config.Netlaunch.Bmc.Username,
			config.Netlaunch.Bmc.Password,
		)

		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
		defer cancel()

		client.Registry.Drivers = client.Registry.For("gofish")
		if err := client.Open(context.Background()); err != nil {
			log.WithError(err).Fatalf("failed to open connection to BMC")
		}

		if _, err = client.SetPowerState(ctx, "off"); err != nil {
			log.WithError(err).Fatalf("failed to turn off machine")
		}

		if _, err = client.SetVirtualMedia(ctx, "CD", "http://"+listen.Addr().String()+"/provision.iso"); err != nil {
			log.WithError(err).Fatalf("failed to set virtual media")
		}

		if _, err = client.SetBootDevice(ctx, "cdrom", false, true); err != nil {
			log.WithError(err).Fatalf("failed to set boot media")
		}

		if _, err = client.SetPowerState(ctx, "on"); err != nil {
			log.WithError(err).Fatalf("failed to turn on machine")
		}

		log.Info("ISO deployed!")

		if len(tridentConfigFile) != 0 {
			log.Info("Waiting for phone home...")
		}

		// Wait for done signal
		<-done
		server.Shutdown(context.Background())
	},
}

func init() {
	rootCmd.PersistentFlags().StringVarP(&netlaunchConfigFile, "config", "c", "netlaunch.yaml", "Netlaunch config file")
	rootCmd.PersistentFlags().StringVarP(&tridentConfigFile, "trident", "t", "", "Trident local config file")
	rootCmd.PersistentFlags().BoolVarP(&logstream, "logstream", "l", false, "Enable log streaming. (Requires --trident || --port)")
	rootCmd.PersistentFlags().Int16VarP(&listen_port, "port", "p", 0, "Port to listen on for logstream & phonehome. Random if not specified.")
	rootCmd.PersistentFlags().StringVarP(&remoteAddressFile, "remoteaddress", "r", "", "File for writing remote address of the Trident instance.")
	rootCmd.PersistentFlags().StringVarP(&serveFolder, "servefolder", "s", "", "Optional folder to serve files from at /files")
	rootCmd.PersistentFlags().BoolVarP(&ignoreFailure, "ignore-failure", "", false, "Keep running even if Trident sends back a failure message")
	rootCmd.PersistentFlags().StringVarP(&traceFile, "trace-file", "m", "", "File for writing metrics collected from Trident.")
	rootCmd.Flags().StringVarP(&iso, "iso", "i", "", "ISO for Netlaunch testing.")
	rootCmd.MarkFlagRequired("iso-template")
	log.SetLevel(log.DebugLevel)
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
