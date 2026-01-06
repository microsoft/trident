package netlisten

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/phonehome"

	"github.com/sirupsen/logrus"
)

func RunNetlisten(ctx context.Context, config *netlaunch.NetListenConfig) error {
	address := fmt.Sprintf("0.0.0.0:%d", config.ListenPort)
	listen, err := net.Listen("tcp4", address)
	if err != nil {
		return fmt.Errorf("failed to open port listening on %s: %w", address, err)
	}

	// Set up listening
	result := make(chan phonehome.PhoneHomeResult)
	mux := http.NewServeMux()
	server := &http.Server{Handler: mux}

	// Set up listening for phonehome
	phonehome.SetupPhoneHomeServer(mux, result, "")
	// Set up listening for logstream
	logstreamFull, err := phonehome.SetupLogstream(mux, config.LogstreamFile)
	if err != nil {
		return fmt.Errorf("failed to set up logstream: %w", err)
	}
	defer logstreamFull.Close()

	// Set up listening for tracestream
	traceFile, err := phonehome.SetupTraceStream(mux, config.TracestreamFile)
	if err != nil {
		return fmt.Errorf("failed to set up trace stream: %w", err)
	}
	defer traceFile.Close()

	if len(config.ServeDirectory) != 0 {
		mux.Handle("/files/", http.StripPrefix("/files/", http.FileServer(http.Dir(config.ServeDirectory))))
	}

	// If serial over SSH is configured, listen for serial output.
	if config.Netlisten.Bmc != nil && config.Netlisten.Bmc.SerialOverSsh != nil {
		serial, err := config.Netlisten.Bmc.ListenForSerialOutput(ctx)
		if err != nil {
			return fmt.Errorf("failed to open serial over SSH session: %w", err)
		}
		defer serial.Close()
	}

	// Start the HTTP server
	go server.Serve(listen)
	logrus.WithField("address", listen.Addr().String()).Info("Listening...")

	logrus.Info("Waiting for phone home...")

	// Wait for done signal.
	phonehomeErr := phonehome.ListenLoop(ctx, result, false, config.MaxPhonehomeFailures)

	err = server.Shutdown(ctx)
	if err != nil {
		if ctx.Err() != nil {
			logrus.Infoln("server shutdown due to context cancellation")
		} else {
			logrus.WithError(err).Errorln("failed to shutdown server")
		}
	}

	if phonehomeErr != nil {
		logrus.WithError(phonehomeErr).Errorln("phonehome returned an error")
		return phonehomeErr
	}

	return nil
}
