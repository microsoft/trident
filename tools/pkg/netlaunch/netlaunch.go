package netlaunch

import (
	"bytes"
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/bmc-toolbox/bmclib/v2"
	"github.com/google/uuid"
	log "github.com/sirupsen/logrus"
	"github.com/stmcginnis/gofish/redfish"
	"gopkg.in/yaml.v2"

	"tridenttools/pkg/isopatcher"
	"tridenttools/pkg/netfinder"
	"tridenttools/pkg/phonehome"
	rcpclient "tridenttools/pkg/rcp/client"
	"tridenttools/pkg/rcp/tlscerts"
	"tridenttools/pkg/tridentgrpc"
	stormutils "tridenttools/storm/utils"
)

func RunNetlaunch(ctx context.Context, config *NetLaunchConfig) error {
	// Read the ISO
	iso, err := os.ReadFile(config.IsoPath)
	if err != nil {
		return fmt.Errorf("failed to read ISO file '%s': %w", config.IsoPath, err)
	}

	localListenAddress := fmt.Sprintf(":%d", config.ListenPort)
	listen, err := net.Listen("tcp4", localListenAddress)
	if err != nil {
		return fmt.Errorf("failed to open port listening on '%s': %v", localListenAddress, err)
	}
	defer listen.Close()

	// Find the port we're listening on
	var announcePort string
	if config.Netlaunch.AnnouncePort != nil {
		announcePort = fmt.Sprintf("%d", *config.Netlaunch.AnnouncePort)
	} else {
		announcePort = strings.Split(listen.Addr().String(), ":")[1]
	}

	// When enabled, set up RCP listener.
	var rcpListener *rcpclient.RcpListener
	if config.IsGrpcModeEnabled() {
		if config.Rcp.GetGrpcMode() == GrpcModeLocalProxy {
			log.Info("RCP gRPC mode enabled: local proxy mode")
			if config.Rcp.LocalProxySocket == "" {
				return fmt.Errorf("local proxy socket path must be specified when using local proxy gRPC mode")
			}
		}

		port := uint16(0)
		if config.Rcp.ListenPort != nil {
			port = *config.Rcp.ListenPort
		}

		rcpListener, err = rcpclient.ListenAndAccept(ctx, tlscerts.ServerCertProvider, port)
		if err != nil {
			return fmt.Errorf("failed to start RCP listener: %w", err)
		}
		defer rcpListener.Close()
		log.WithField("port", rcpListener.Port).Info("RCP listener started")
	}

	// Do we expect Trident to reach back? If so we need to listen to it.
	// If we have a specified port, we assume that the intent is that Trident will reach back.
	enable_phonehome_listening := config.ListenPort != 0

	result := make(chan phonehome.PhoneHomeResult)
	mux := http.NewServeMux()
	server := &http.Server{Handler: mux}

	// Create the final address that will be announced to the BMC and Trident.
	var announceIp string
	if config.Netlaunch.AnnounceIp != nil {
		// If an IP is specified, use it.
		announceIp = *config.Netlaunch.AnnounceIp
	} else if config.Netlaunch.Bmc != nil && config.Netlaunch.Bmc.Ip != "" {
		// Otherwise, try to be clever...
		// We need to find the IP of the local interface that can reach the BMC.
		log.Warn("No announce IP specified. Attempting to find local IP to announce based on BMC IP.")
		announceIp, err = netfinder.FindLocalIpForTargetIp(config.Netlaunch.Bmc.Ip)
		if err != nil {
			return fmt.Errorf("failed to find local IP for BMC: %w", err)
		}
	} else {
		// If we have no BMC, find the default outbound IP.
		log.Warn("No announce IP specified. Attempting to find default outbound IP to announce.")
		announceIp, err = netfinder.FindDefaultOutboundIp()
		if err != nil {
			return fmt.Errorf("failed to find default outbound IP: %w", err)
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

	// Create a context that we can use to terminate the phonehome listening
	terminateCtx, terminateFunc := context.WithCancel(ctx)
	defer terminateFunc()

	var finalHostConfigurationYaml string
	hostConfigData := make(map[string]interface{})
	var extraRcpAgentFiles []rcpAgentFileDownload

	// If we have a Trident config file, we need to patch it into the ISO.
	if len(config.HostConfigFile) != 0 {
		log.Info("Using Trident config file: ", config.HostConfigFile)
		tridentConfigContents, err := os.ReadFile(config.HostConfigFile)
		if err != nil {
			return fmt.Errorf("failed to read Trident config: %w", err)
		}

		// Replace NETLAUNCH_HOST_ADDRESS with the address of the netlaunch server
		tridentConfigContentsStr := strings.ReplaceAll(string(tridentConfigContents), "NETLAUNCH_HOST_ADDRESS", announceAddress)

		err = yaml.UnmarshalStrict([]byte(tridentConfigContentsStr), &hostConfigData)
		if err != nil {
			return fmt.Errorf("failed to unmarshal Trident config: %w", err)
		}

		logstreamAddress := fmt.Sprintf("http://%s/logstream", announceAddress)

		// Add phonehome & logstream config ONLY when NOT in gRPC RCP mode
		if !config.IsGrpcModeEnabled() {
			if _, ok := hostConfigData["trident"]; !ok {
				hostConfigData["trident"] = make(map[interface{}]interface{})
			}
			hostConfigData["trident"].(map[interface{}]interface{})["phonehome"] = fmt.Sprintf("http://%s/phonehome", announceAddress)
			hostConfigData["trident"].(map[interface{}]interface{})["logstream"] = logstreamAddress
		}

		// Patch the ISO Host Configuration file unless this is a stream-image
		// test, where the Host Configuration file is not expected to be
		// present, or we are using gRPC mode, where the HC file is irrelevant.
		if config.Rcp == nil || !config.Rcp.UseStreamImage || !config.IsGrpcModeEnabled() {
			tridentConfig, err := yaml.Marshal(hostConfigData)
			if err != nil {
				return fmt.Errorf("failed to marshal Trident config: %w", err)
			}

			// Store the modified trident config for later use
			finalHostConfigurationYaml = string(tridentConfig)

			err = isopatcher.PatchFile(iso, "/etc/trident/config.yaml", tridentConfig)
			if err != nil {
				return fmt.Errorf("failed to patch Trident config into ISO: %w", err)
			}
		}

		if config.Rcp != nil && config.Rcp.UseStreamImage {
			overrideFile, err := makeStreamImageOverrideFileDownload(hostConfigData, logstreamAddress)
			if err != nil {
				return fmt.Errorf("failed to create stream image override file download: %w", err)
			}

			extraRcpAgentFiles = append(extraRcpAgentFiles, overrideFile)
		}

		if config.Iso.PreTridentScript != nil {
			log.Info("Patching in pre-trident script!")
			err = isopatcher.PatchFile(iso, "/trident_cdrom/pre-trident-script.sh", []byte(*config.Iso.PreTridentScript))
			if err != nil {
				return fmt.Errorf("failed to patch pre-trident script into ISO: %w", err)
			}
		}

		if config.Iso.ServiceOverride != nil {
			log.Info("Patching Trident service override!")
			err = isopatcher.PatchFile(iso, "/trident_cdrom/trident-override.conf", []byte(*config.Iso.ServiceOverride))
			if err != nil {
				return fmt.Errorf("failed to patch service override into ISO: %w", err)
			}
		}

		// We injected the phonehome & logstream config, so we're expecting Trident to reach back
		enable_phonehome_listening = true
	}

	// Inject RCP agent config when we are using it.
	if config.Rcp != nil {
		// Populate RCP agent config into the ISO. This handles both CLI and
		// GRPC modes, where rcpListener == nil and rcpListener != nil
		// respectively.
		err = injectRcpAgentConfig(mux, announceIp, announceAddress, iso, rcpListener, *config.Rcp, extraRcpAgentFiles...)
		if err != nil {
			return fmt.Errorf("failed to inject RCP agent config into ISO: %w", err)
		}
	}

	mux.HandleFunc("/provision.iso", func(w http.ResponseWriter, r *http.Request) {
		isoLogFunc(r.RemoteAddr)
		http.ServeContent(w, r, "provision.iso", time.Now(), bytes.NewReader(iso))
		if !enable_phonehome_listening {
			// If we're not expecting Trident to reach back, we can terminate
			// after serving the ISO
			terminateFunc()
		}
	})

	// If we're expecting Trident to reach back, we need to listen for it.
	if enable_phonehome_listening {
		// Set up listening for phonehome
		phonehome.SetupPhoneHomeServer(mux, result, config.RemoteAddressFile)

		// Set up listening for logstream
		logstreamFull, err := phonehome.SetupLogstream(mux, config.LogstreamFile)
		if err != nil {
			return fmt.Errorf("failed to setup logstream: %w", err)
		}
		defer logstreamFull.Close()

		// Set up listening for tracestream
		traceFile, err := phonehome.SetupTraceStream(mux, config.TracestreamFile)
		if err != nil {
			return fmt.Errorf("failed to setup tracestream: %w", err)
		}
		defer traceFile.Close()

	}

	if len(config.ServeDirectory) != 0 {
		mux.Handle("/files/", http.StripPrefix("/files/", http.FileServer(http.Dir(config.ServeDirectory))))
	}

	// Start the HTTP server
	go server.Serve(listen)
	log.WithField("address", listen.Addr().String()).Info("Listening...")
	iso_location := fmt.Sprintf("http://%s/provision.iso", announceAddress)

	// Validate that if file at signingCert exists, it can be read
	if config.CertificateFile != "" {
		file, err := os.Open(config.CertificateFile)
		if err != nil {
			return fmt.Errorf("failed to open signing certificate '%s' for reading: %v", config.CertificateFile, err)
		}
		file.Close()
	}

	if config.Netlaunch.LocalVmUuid != nil {
		err := startLocalVm(*config.Netlaunch.LocalVmUuid, iso_location, config.EnableSecureBoot, config.CertificateFile)
		if err != nil {
			return err
		}
	} else {
		if config.Netlaunch.Bmc != nil && config.Netlaunch.Bmc.SerialOverSsh != nil {
			serial, err := config.Netlaunch.Bmc.ListenForSerialOutput(ctx)
			if err != nil {
				return fmt.Errorf("failed to open serial over SSH session: %w", err)
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

		bmcCtx, cancel := context.WithTimeout(ctx, 5*time.Minute)
		defer cancel()

		log.Info("Connecting to BMC")
		client.Registry.Drivers = client.Registry.For("gofish")
		if err := client.Open(bmcCtx); err != nil {
			return fmt.Errorf("failed to open connection to BMC: %w", err)
		}

		log.Info("Shutting down machine")
		if _, err = client.SetPowerState(bmcCtx, "off"); err != nil {
			return fmt.Errorf("failed to turn off machine: %w", err)
		}

		log.WithField("url", iso_location).Info("Setting virtual media to ISO")
		if _, err = client.SetVirtualMedia(bmcCtx, string(redfish.CDMediaType), iso_location); err != nil {
			return fmt.Errorf("failed to set virtual media: %w", err)
		}

		log.Info("Setting boot media")
		if _, err = client.SetBootDevice(bmcCtx, "cdrom", false, true); err != nil {
			return fmt.Errorf("failed to set boot media: %w", err)
		}

		log.Info("Turning on machine")
		if _, err = client.SetPowerState(bmcCtx, "on"); err != nil {
			return fmt.Errorf("failed to turn on machine: %w", err)
		}
	}

	log.Info("ISO deployed!")

	if len(config.HostConfigFile) != 0 {
		log.Info("Waiting for phone home...")
	}

	if rcpListener != nil {
		if config.Rcp.GetGrpcMode() == GrpcModeLocalProxy {
			err := openLocalProxy(ctx, config.Rcp.LocalProxySocket, rcpListener)
			if err != nil {
				return fmt.Errorf("failed to run local proxy: %w", err)
			}
		} else {
			// Netlaunch will directly speak gRPC to trident. Wait for the
			// connection and handle it.
			log.Info("Waiting for RCP connection...")
			select {
			case <-ctx.Done():
				return ctx.Err()
			case conn, ok := <-rcpListener.ConnChan:
				if !ok {
					return fmt.Errorf("RCP listener channel closed")
				}

				defer conn.Close()

				log.Infof("Accepted RCP connection from %s", conn.RemoteAddr())

				if config.Rcp.GetGrpcMode() == GrpcModeInstall {
					installCtx, installCancel := context.WithTimeout(ctx, time.Minute*10)
					defer installCancel()

					err := doGrpcInstall(installCtx, conn, finalHostConfigurationYaml)
					if err != nil {
						return fmt.Errorf("gRPC installation failed: %w", err)
					}

					log.Info("gRPC installation completed successfully")
				} else if config.Rcp.GetGrpcMode() == GrpcModeStream {
					streamCtx, streamCancel := context.WithTimeout(ctx, time.Minute*10)
					defer streamCancel()

					imgUrl, imgHash, err := getUmageUrlAndHashFromTridentConfig(hostConfigData)
					if err != nil {
						return fmt.Errorf("failed to get image URL and hash from Trident config: %w", err)
					}

					err = doGrpcStream(streamCtx, conn, imgUrl, imgHash)
					if err != nil {
						return fmt.Errorf("gRPC streaming failed: %w", err)
					}
					log.Info("gRPC streaming completed successfully")
				} else {
					return fmt.Errorf("unsupported RCP gRPC mode: %s", config.Rcp.GetGrpcMode())
				}
			}
		}
	} else {
		// Wait for something to happen
		exitError := phonehome.ListenLoop(terminateCtx, result, config.WaitForProvisioning, config.MaxPhonehomeFailures)

		err = server.Shutdown(ctx)
		if err != nil {
			log.WithError(err).Errorln("failed to shutdown server")
		}

		if exitError != nil {
			log.WithError(exitError).Errorln("phonehome returned an error")
			return exitError
		}
	}

	return nil
}

func startLocalVm(localVmUuidStr string, isoLocation string, secureBoot bool, signingCert string) error {
	log.Info("Using local VM")

	// TODO: Parse the UUID directly when reading the config file
	vmUuid, err := uuid.Parse(localVmUuidStr)
	if err != nil {
		return fmt.Errorf("failed to parse LocalVmUuid as UUID: %w", err)
	}

	vm, err := stormutils.InitializeVm(vmUuid)
	if err != nil {
		return fmt.Errorf("failed to initialize VM: %w", err)
	}
	defer vm.Disconnect()

	if err = vm.SetFirmwareVars(isoLocation, secureBoot, signingCert); err != nil {
		return fmt.Errorf("failed to set UEFI variables: %w", err)
	}

	if err = vm.Start(); err != nil {
		return fmt.Errorf("failed to start VM: %w", err)
	}

	return nil
}

func injectRcpAgentConfig(
	mux *http.ServeMux,
	announceIp string,
	announceHttpAddress string,
	iso []byte,
	rcpListener *rcpclient.RcpListener,
	localRcpConf RcpConfiguration,
	extraFiles ...rcpAgentFileDownload,
) error {
	rcpServerAddress := tridentgrpc.DefaultTridentSocketPath
	rcpServerConnectionType := "unix"

	if localRcpConf.RcpAgentServerAddress != "" {
		log.Warnf("Custom rcp-agent server: '%s'", localRcpConf.RcpAgentServerAddress)
		rcpServerAddress = localRcpConf.RcpAgentServerAddress
	}

	if localRcpConf.RcpAgentServerConnectionType != "" {
		log.Warnf("Custom rcp-agent server connection type: '%s'", localRcpConf.RcpAgentServerConnectionType)
		rcpServerConnectionType = localRcpConf.RcpAgentServerConnectionType
	}

	// Create an empty RcpAgentConfiguration
	rcpAgentConfBuilder := newRcpAgentConfigBuilder(
		mux,
		announceIp,
		announceHttpAddress,
		rcpListener,
		rcpServerAddress,
		rcpServerConnectionType,
	)

	// If we have a local trident path, serve that file via HTTP and set the download URL.
	if localRcpConf.LocalTridentPath != nil {
		data, err := os.ReadFile(*localRcpConf.LocalTridentPath)
		if err != nil {
			return fmt.Errorf("failed to read local Trident binary from '%s': %w", *localRcpConf.LocalTridentPath, err)
		}

		rcpAgentConfBuilder.registerRcpFile(newRcpAgentFileDownload("trident", "/usr/bin/trident", 0755, data))
	}

	// If we have a local osmodifier path, serve that file via HTTP and set the download URL.
	if localRcpConf.LocalOsmodifierPath != nil {
		data, err := os.ReadFile(*localRcpConf.LocalOsmodifierPath)
		if err != nil {
			return fmt.Errorf("failed to read local Osmodifier binary from '%s': %w", *localRcpConf.LocalOsmodifierPath, err)
		}

		rcpAgentConfBuilder.registerRcpFile(newRcpAgentFileDownload("osmodifier", "/usr/bin/osmodifier", 0755, data))
	}

	for _, extraFile := range extraFiles {
		rcpAgentConfBuilder.registerRcpFile(extraFile)
	}

	for _, file := range localRcpConf.AdditionalFiles {
		rcpAgentConfBuilder.registerRcpFile(newRcpAgentFileDownload(file.Name, file.Destination, file.Mode, file.Data))
	}

	for _, service := range localRcpConf.StartServices {
		rcpAgentConfBuilder.startService(service)
	}

	encoded, err := yaml.Marshal(rcpAgentConfBuilder.build())
	if err != nil {
		return fmt.Errorf("failed to marshal RCP agent config to YAML: %w", err)
	}

	err = isopatcher.PatchFile(iso, "/trident_cdrom/rcp-agent.yaml", encoded)
	if err != nil {
		return fmt.Errorf("failed to patch RCP agent config into ISO: %w", err)
	}

	return nil
}
