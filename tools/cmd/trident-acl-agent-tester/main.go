package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"syscall"
	"time"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"

	"tridenttools/pkg/acltester"
)

func main() {
	cobra.CheckErr(newRootCmd().Execute())
}

func newRootCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "trident-acl-agent-tester",
		Short: "Validation harness for the trident-acl-agent label protocol",
		PersistentPreRun: func(cmd *cobra.Command, args []string) {
			log.SetFormatter(&log.TextFormatter{FullTimestamp: true})
		},
	}

	cmd.AddCommand(newAPIServerCmd())
	cmd.AddCommand(newRPProxyCmd())
	cmd.AddCommand(newKubeletProxyCmd())
	cmd.AddCommand(newNebraskaProxyCmd())
	return cmd
}

func newAPIServerCmd() *cobra.Command {
	var listenAddr string
	var nodeName string
	var seedLabels string
	var seedFile string

	cmd := &cobra.Command{
		Use:   "apiserver",
		Short: "Run a fake single-node Kubernetes apiserver",
		RunE: func(cmd *cobra.Command, args []string) error {
			seedMap, err := acltester.ParseKeyValueList(seedLabels)
			if err != nil {
				return err
			}

			seedNode := acltester.NewSeedNode(nodeName, seedMap)
			if seedFile != "" {
				data, err := os.ReadFile(seedFile)
				if err != nil {
					return fmt.Errorf("failed to read seed file %s: %w", seedFile, err)
				}
				seedNode, err = acltester.LoadSeedNode(data)
				if err != nil {
					return err
				}
				if nodeName != acltester.DefaultNodeName {
					seedNode.Name = nodeName
				}
				for key, value := range seedMap {
					seedNode.Labels[key] = value
				}
			}

			ctx, cancel := signalContext()
			defer cancel()
			store := acltester.NewNodeStore(seedNode)
			server := acltester.NewAPIServer(seedNode.Name, store)
			listener, err := server.ListenAndServe(ctx, listenAddr)
			if err != nil {
				return err
			}
			log.WithField("listen", listener.Addr().String()).Info("fake apiserver started")
			<-ctx.Done()
			return nil
		},
	}
	cmd.Flags().StringVar(&listenAddr, "listen", "127.0.0.1:18080", "Address to listen on")
	cmd.Flags().StringVar(&nodeName, "node-name", acltester.DefaultNodeName, "Node name to serve")
	cmd.Flags().StringVar(&seedLabels, "seed-labels", "", "Comma-separated key=value labels to seed onto the Node")
	cmd.Flags().StringVar(&seedFile, "seed-file", "", "Path to a Node JSON seed file")
	return cmd
}

func newRPProxyCmd() *cobra.Command {
	var apiserverURL string
	var nodeName string
	var scenarioPath string
	var jsonOutput bool

	cmd := &cobra.Command{
		Use:   "rp-proxy",
		Short: "Run an aks-rp scenario against the fake apiserver",
		RunE: func(cmd *cobra.Command, args []string) error {
			if apiserverURL == "" {
				return fmt.Errorf("--apiserver-url is required")
			}
			if scenarioPath == "" {
				return fmt.Errorf("--scenario is required")
			}
			scenario, err := acltester.LoadScenario(scenarioPath)
			if err != nil {
				return err
			}
			runner := &acltester.RPClient{APIServerURL: apiserverURL, NodeName: nodeName}
			report, err := runner.RunScenario(context.Background(), scenario)
			if err != nil {
				return err
			}
			if jsonOutput {
				payload, err := json.MarshalIndent(report, "", "  ")
				if err != nil {
					return err
				}
				fmt.Println(string(payload))
			} else {
				printHumanReport(report)
			}
			if !report.Passed {
				return fmt.Errorf("scenario failed")
			}
			return nil
		},
	}
	cmd.Flags().StringVar(&apiserverURL, "apiserver-url", "", "Base URL of the fake apiserver")
	cmd.Flags().StringVar(&nodeName, "node-name", acltester.DefaultNodeName, "Node name to manipulate")
	cmd.Flags().StringVar(&scenarioPath, "scenario", "", "Path to scenario YAML")
	cmd.Flags().BoolVar(&jsonOutput, "json", false, "Emit the scenario report as JSON")
	return cmd
}

func newKubeletProxyCmd() *cobra.Command {
	var apiserverURL string
	var nodeName string
	var bootstrapLabels string
	var markerFile string
	var rebootDuration time.Duration

	cmd := &cobra.Command{
		Use:   "kubelet-proxy",
		Short: "Simulate kubelet bootstrap labels and reboot readiness flips",
		RunE: func(cmd *cobra.Command, args []string) error {
			if apiserverURL == "" {
				return fmt.Errorf("--apiserver-url is required")
			}
			labels, err := acltester.ParseKeyValueList(bootstrapLabels)
			if err != nil {
				return err
			}
			ctx, cancel := signalContext()
			defer cancel()
			proxy := &acltester.KubeletProxy{
				APIServerURL:    apiserverURL,
				NodeName:        nodeName,
				BootstrapLabels: labels,
				MarkerFile:      markerFile,
				RebootDuration:  rebootDuration,
			}
			return proxy.Run(ctx)
		},
	}
	cmd.Flags().StringVar(&apiserverURL, "apiserver-url", "", "Base URL of the fake apiserver")
	cmd.Flags().StringVar(&nodeName, "node-name", acltester.DefaultNodeName, "Node name to manipulate")
	cmd.Flags().StringVar(&bootstrapLabels, "bootstrap-labels", "", "Comma-separated key=value bootstrap labels")
	cmd.Flags().StringVar(&markerFile, "marker-file", acltester.DefaultMarkerFile, "Marker file watched for reboot interception")
	cmd.Flags().DurationVar(&rebootDuration, "reboot-duration", 30*time.Second, "How long the node stays NotReady during a simulated reboot")
	return cmd
}

func newNebraskaProxyCmd() *cobra.Command {
	var listenAddr string
	var scenarioPath string

	cmd := &cobra.Command{
		Use:   "nebraska-proxy",
		Short: "Run a fake Omaha/Nebraska endpoint",
		RunE: func(cmd *cobra.Command, args []string) error {
			if scenarioPath == "" {
				return fmt.Errorf("--scenario is required")
			}
			scenario, err := acltester.LoadNebraskaScenario(scenarioPath)
			if err != nil {
				return err
			}
			ctx, cancel := signalContext()
			defer cancel()
			proxy := &acltester.NebraskaProxy{Scenario: scenario}
			listener, err := proxy.ListenAndServe(ctx, listenAddr)
			if err != nil {
				return err
			}
			log.WithField("listen", listener.Addr().String()).Info("fake Nebraska proxy started")
			<-ctx.Done()
			return nil
		},
	}
	cmd.Flags().StringVar(&listenAddr, "listen", "127.0.0.1:18081", "Address to listen on")
	cmd.Flags().StringVar(&scenarioPath, "scenario", "", "Path to Nebraska scenario YAML")
	return cmd
}

func signalContext() (context.Context, context.CancelFunc) {
	return signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM, syscall.SIGINT)
}

func printHumanReport(report *acltester.ScenarioReport) {
	fmt.Printf("scenario passed: %t\n", report.Passed)
	for _, step := range report.Steps {
		fmt.Printf("step %d [%s] passed=%t elapsed=%dms %s\n", step.Index, step.Kind, step.Passed, step.ElapsedMS, step.Message)
		if step.Expected != nil {
			fmt.Printf("  expected: %v\n", step.Expected)
		}
		if step.Actual != nil {
			fmt.Printf("  actual:   %v\n", step.Actual)
		}
	}
}
