/*
miniproxy: a simple TCP proxy that listens on a port and forwards all traffic to a destination port.
*/
package main

import (
	"io"
	"net"
	"os"
	"strconv"
	"sync"

	"github.com/dustin/go-humanize"
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

	rootCmd.MarkFlagRequired("listen-port")
	rootCmd.MarkFlagRequired("forward-port")
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

func handleConnection(client net.Conn, id uint64) {
	log.WithField("id", id).WithField("address", client.RemoteAddr().String()).Info("Accepted connection")
	defer client.Close()
	server, err := net.Dial("tcp", "localhost:"+strconv.Itoa(int(dest_port)))
	if err != nil {
		log.WithField("id", id).WithError(err).Fatalf("Failed to connect to destination")
	}
	defer server.Close()

	log.WithField("id", id).WithField("address", server.RemoteAddr().String()).Info("Connected to destination")

	var wg sync.WaitGroup

	wg.Add(2)
	go func() {
		defer wg.Done()
		defer client.Close()
		defer server.Close()
		copied, _ := io.Copy(server, client)
		log.WithField("id", id).Infof("Done copying %s from client to server", humanize.IBytes(uint64(copied)))
	}()

	go func() {
		defer wg.Done()
		defer client.Close()
		defer server.Close()
		copied, _ := io.Copy(client, server)
		log.WithField("id", id).Infof("Done copying %s from server to client", humanize.IBytes(uint64(copied)))
	}()

	wg.Wait()
	log.WithField("id", id).Info("Connection closed")
}
