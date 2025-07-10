/*
Copyright © 2023 Microsoft Corporation
*/
package main

import (
	"context"
	"os"
	"tridenttools/pkg/config"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

var (
	// Reuse netlaunch config file
	netlaunchConfigFile string
)

var rootCmd = &cobra.Command{
	Use:   "seriallisten",
	Short: "Listen to BMC serial output\n\n",
	PreRun: func(cmd *cobra.Command, args []string) {
		// Set log level
		log.SetLevel(log.DebugLevel)
	},
	Run: func(cmd *cobra.Command, args []string) {
		viper.SetConfigType("yaml")
		viper.SetConfigFile(netlaunchConfigFile)
		if err := viper.ReadInConfig(); err != nil {
			log.WithError(err).Fatal("failed to read configuration file")
		}

		config := config.NetLaunchConfig{}
		if err := viper.UnmarshalExact(&config); err != nil {
			log.WithError(err).Fatal("could not unmarshal configuration")
		}

		terminateCtx, terminateFunc := context.WithCancel(context.Background())
		defer terminateFunc()

		if config.Netlaunch.Bmc != nil && config.Netlaunch.Bmc.SerialOverSsh != nil {
			serial, err := config.Netlaunch.Bmc.ListenForSerialOutput()
			if err != nil {
				log.WithError(err).Fatalf("Failed to open serial over SSH session")
			}
			defer serial.Close()

			// Wait for context cancellation
			<-terminateCtx.Done()
		}
		// If we're told to terminate, then we're done.
		os.Exit(0)
	},
}

func init() {
	rootCmd.PersistentFlags().StringVarP(&netlaunchConfigFile, "config", "c", "netlaunch.yaml", "Netlaunch config file")
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
