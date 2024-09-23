/*
miniproxy: a simple TCP proxy that listens on a port and forwards all traffic to a destination port.
*/
package main

import (
	"io"
	"net"
	"os"
	"strconv"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
)

var listen_port uint16
var dest_port uint16

var rootCmd = &cobra.Command{
	Use:   "miniproxy",
	Short: "Trivial TCP proxy",
	PreRun: func(cmd *cobra.Command, args []string) {

	},
	Run: run,
}

func init() {
	rootCmd.PersistentFlags().Uint16VarP(&listen_port, "listen-port", "l", 0, "Port to listen on.")
	rootCmd.PersistentFlags().Uint16VarP(&dest_port, "forward-port", "f", 0, "Port to forward to.")

	rootCmd.MarkFlagRequired("src-port")
	rootCmd.MarkFlagRequired("dst-port")
	log.SetLevel(log.DebugLevel)
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}

func run(cmd *cobra.Command, args []string) {
	listener, err := net.Listen("tcp", "0.0.0.0:"+strconv.Itoa(int(listen_port)))
	if err != nil {
		log.WithError(err).Fatalf("Failed to listen")
	}

	log.WithField("address", listener.Addr().String()).WithField("dest", dest_port).Info("Listening...")
	var id uint64 = 0
	for {
		conn, err := listener.Accept()
		if err != nil {
			log.WithError(err).Fatalf("Failed to accept connection")
		}

		go handleConnection(conn, id)
		id++
	}
}

func handleConnection(source net.Conn, id uint64) {
	log.WithField("id", id).WithField("address", source.RemoteAddr().String()).Info("Accepted connection")
	defer source.Close()
	dest, err := net.Dial("tcp", "localhost:"+strconv.Itoa(int(dest_port)))
	if err != nil {
		log.WithField("id", id).WithError(err).Fatalf("Failed to connect to destination")
	}
	defer dest.Close()

	log.WithField("id", id).WithField("address", dest.RemoteAddr().String()).Info("Connected to destination")

	done := make(chan bool)
	go func() {
		defer source.Close()
		defer dest.Close()
		copied, _ := io.Copy(dest, source)
		log.WithField("id", id).WithField("bytes", copied).Info("Done copying from src to dst")
		done <- true
	}()

	go func() {
		defer source.Close()
		defer dest.Close()
		copied, _ := io.Copy(source, dest)
		log.WithField("id", id).WithField("bytes", copied).Info("Done copying from dst to src")
		done <- true
	}()

	<-done
	<-done
	log.WithField("id", id).Info("Connection closed")
}
