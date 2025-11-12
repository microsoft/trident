package helpers

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type DisplayLogsHelper struct {
	args struct {
		SkipSerialLog               bool   `help:"Skip serial log handling." default:"false"`
		SerialLogPath               string `help:"If specified, this is the serial log file path." default:""`
		NetlistenConfig             string `help:"If specified and --serial-log-path is not set, the netlisten config file specified here is queried for the serial log path." default:""`
		SerialLogFallbackFolder     string `help:"If --serial-log-path and --netlisten-config are not specified, this folder is searched for serial log files." default:"/tmp"`
		SerialLogFallbackFileSuffix string `help:"If --serial-log-path and --netlisten-config are not specified, this suffix is used to match files in --serial-log-fallback-folder." default:"serial0.log"`
		SerialLogArtifactFileName   string `help:"Filename to use when copying serial log to artifacts folder, required if serial log is not skipped." default:""`
		TridentLogFile              string `help:"File containing trident log output, skipped if not specified." default:""`
		TridentTraceLogFile         string `help:"File containing trident trace log output, skipped if not specified." default:""`
	}
}

func (h DisplayLogsHelper) Name() string {
	return "display-logs"
}

func (h *DisplayLogsHelper) Args() any {
	return &h.args
}

func (h *DisplayLogsHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("display-serial", h.displaySerial)
	r.RegisterTestCase("display-trident", h.displayTrident)
	r.RegisterTestCase("display-trace-trident", h.displayTraceTrident)
	return nil
}

func getSerialPathFromNetlistenConfig(netlistenConfigPath string) string {
	if netlistenConfigPath != "" {
		tridentConfigContents, err := os.ReadFile(netlistenConfigPath)
		if err != nil {
			logrus.Tracef("Failed to read netlisten config file %s: %v", netlistenConfigPath, err)
			return ""
		}

		netlistenConfig := make(map[string]interface{})
		err = yaml.UnmarshalStrict(tridentConfigContents, &netlistenConfig)
		if err != nil {
			logrus.Tracef("Failed to parse netlisten config file %s: %v", netlistenConfigPath, err)
			return ""
		}
		if netlisten, ok := netlistenConfig["netlisten"].(map[string]interface{}); ok {
			if bmc, ok := netlisten["bmc"].(map[string]interface{}); ok {
				if serialOverSsh, ok := bmc["serialOverSsh"].(map[string]interface{}); ok {
					if output, ok := serialOverSsh["output"].(string); ok {
						return output
					}
				}
			}
		}
	}
	return ""
}

func copyAndDisplayLogFile(tc storm.TestCase, logFilePath string, artifactFileName string) error {
	logrus.Infof("== Copy Log from %s to artifacts ==", logFilePath)
	tc.ArtifactBroker().PublishLogFile(artifactFileName, logFilePath)

	logrus.Infof("== Log Output from %s ==", logFilePath)
	logs, err := os.ReadFile(logFilePath)
	if err != nil {
		return err
	}
	logrus.Info(strings.TrimSpace(string(logs)))
	return nil
}

func (h *DisplayLogsHelper) displaySerial(tc storm.TestCase) error {
	if h.args.SkipSerialLog {
		tc.Skip("Skipping serial log.")
		return nil
	}
	if h.args.SerialLogArtifactFileName == "" {
		return fmt.Errorf("serial log artifact file name must be specified when not skipping serial log")
	}

	// First look at specified serial log file
	serialLogFile := h.args.SerialLogPath
	// If serial log file is not explicitly specified, try getting from netlisten config
	if serialLogFile == "" {
		serialLogFile = getSerialPathFromNetlistenConfig(h.args.NetlistenConfig)
	}
	// If serial log file is still not set, try looking in fallback folder
	if serialLogFile == "" {
		// Look for a file in the fallback folder that ends with the specified suffix
		entries, err := os.ReadDir(h.args.SerialLogFallbackFolder)
		if err != nil {
			logrus.Info("No serial log file specified and cannot read fallback folder")
			return nil
		}

		var matchingFiles []string
		for _, entry := range entries {
			if !entry.IsDir() && strings.HasSuffix(entry.Name(), h.args.SerialLogFallbackFileSuffix) {
				matchingFiles = append(matchingFiles, entry.Name())
			}
		}

		if len(matchingFiles) == 0 {
			logrus.Info("No serial log file specified and no matching fallback file found")
			return nil
		} else if len(matchingFiles) > 1 {
			logrus.Warnf("Multiple files found ending with '%s': %v, using first one", h.args.SerialLogFallbackFileSuffix, matchingFiles)
		}

		serialLogFile = filepath.Join(h.args.SerialLogFallbackFolder, matchingFiles[0])
		logrus.Tracef("Using fallback serial log file: %s", serialLogFile)
	}

	return copyAndDisplayLogFile(tc, serialLogFile, h.args.SerialLogArtifactFileName)
}

func (h *DisplayLogsHelper) displayTridentLogFile(tc storm.TestCase, logFile string, skipMessage string) error {
	if logFile == "" {
		tc.Skip(skipMessage)
		return nil
	}

	return copyAndDisplayLogFile(tc, logFile, filepath.Base(logFile))
}

func (h *DisplayLogsHelper) displayTrident(tc storm.TestCase) error {
	return h.displayTridentLogFile(tc, h.args.TridentLogFile, "No trident log file specified")
}

func (h *DisplayLogsHelper) displayTraceTrident(tc storm.TestCase) error {
	return h.displayTridentLogFile(tc, h.args.TridentTraceLogFile, "No trident trace log file specified")
}
