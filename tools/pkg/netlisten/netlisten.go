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
	// Close the bound port on every early-return path below. Without this a
	// setup failure after a successful bind leaves the port occupied, and the
	// next scenario or retry in the same runner fails with "address already in
	// use" rather than the real error. server.Serve takes ownership on the
	// success path, so this is cleared once the server is handed the listener.
	defer func() {
		if listen != nil {
			listen.Close()
		}
	}()

	// Set up listening
	result := make(chan phonehome.PhoneHomeResult, 1)
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
	if traceFile != nil {
		defer traceFile.Close()
	}

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
	// Serve owns the listener from here on; Shutdown/Close will close it.
	served := listen
	listen = nil
	go server.Serve(served)
	logrus.WithField("address", served.Addr().String()).Info("Listening...")

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
