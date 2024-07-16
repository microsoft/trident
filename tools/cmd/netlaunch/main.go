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

	"github.com/fatih/color"
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
var listenPort uint16
var remoteAddressFile string
var serveFolder string
var maxFailures uint
var traceFile string
var logTrace bool
var forceColor bool

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

		if logstream && listenPort == 0 && len(tridentConfigFile) == 0 {
			log.Fatal("logstream requires a specified port or trident config file")
		}

		if forceColor {
			log.SetFormatter(&log.TextFormatter{
				ForceColors: true,
			})

			// Force color to be enabled
			color.NoColor = false
		}

		// Set log level
		if logTrace {
			log.SetLevel(log.TraceLevel)
			log.Traceln("Trace logging enabled!")
		} else {
			log.SetLevel(log.DebugLevel)
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

		address := fmt.Sprintf("%s:%d", config.Netlaunch.PublicIp, listenPort)
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
		enable_phonehome_listening := listenPort != 0

		terminate := make(chan bool)
		result := make(chan phonehome.PhoneHomeResult)
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
				terminate <- true
			})
		}

		// If we're expecting trident to reach back, we need to listen for it.
		if enable_phonehome_listening {
			// Set up listening for phonehome
			phonehome.SetupPhoneHomeServer(result, remoteAddressFile)

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

		// Wait for something to happen
		var exitCode = listen_loop(terminate, result)

		err = server.Shutdown(context.Background())
		if err != nil {
			log.WithError(err).Errorln("failed to shutdown server")
		}

		os.Exit(exitCode)
	},
}

// listen_loop listens for phonehome results and logs them.
// If a result file is specified, it writes the result to that file.
// If the result indicates that we should terminate, it returns.
// If the terminate channel receives something, it returns.
func listen_loop(terminate <-chan bool, result <-chan phonehome.PhoneHomeResult) int {
	failureCount := uint(0)

	// Loop forever!
	for {
		// Wait for something to happen
		select {

		case <-terminate:
			// If we're told to terminate, then we're done.
			return 0

		case result := <-result:
			// If we get a result log it.
			result.Log()

			// Check the state of the result.
			switch result.State {
			case phonehome.PhoneHomeResultFailure:
				// If we failed, increment the failure count.
				failureCount++
			default:
				// For everything else, return the exit code.
				return result.ExitCode()
			}

			// Check if we've exceeded the maximum number of failures.
			if failureCount > maxFailures {
				log.Errorf("Maximum number of failures (%d) exceeded. Terminating.", maxFailures)
				return result.ExitCode()
			}
		}
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
	rootCmd.PersistentFlags().BoolVarP(&logTrace, "log-trace", "", false, "Enable trace level logs.")
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
