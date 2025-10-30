package helpers

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type DisplayLogsHelper struct {
	args struct {
		SkipSerialLog               bool   `help:"Skip displaying serial log." default:"false"`
		NetlistenConfig             string `help:"Path to netlisten config file." type:"string" default:""`
		SerialLogFallbackFolder     string `help:"File containing serial log output." type:"string" default:"/tmp"`
		SerialLogFallbackFileSuffix string `help:"File containing serial log output." type:"string" default:"serial0.log"`
		SerialLogArtifactFileName   string `help:"Filename to use when copying serial log to artifacts folder." type:"string" default:""`
		TridentLogFile              string `help:"File containing trident log output." type:"string" default:""`
		TridentTraceLogFile         string `help:"File containing trace log output." type:"string" default:""`
		ArtifactsFolder             string `help:"Folder to copy log files into." type:"string" default:""`
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

func copyFileToArtifactsFolder(sourcePath, artifactsFolder, artifactFileName string) error {
	// Ensure artifacts folder exists
	err := os.MkdirAll(artifactsFolder, 0755)
	if err != nil {
		return err
	}

	destinationPath := filepath.Join(artifactsFolder, artifactFileName)
	input, err := os.ReadFile(sourcePath)
	if err != nil {
		return err
	}
	err = os.WriteFile(destinationPath, input, 0644)
	if err != nil {
		return err
	}
	logrus.Tracef("Copied file to artifacts folder: %s", destinationPath)
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
					if logFile, ok := serialOverSsh["logFile"].(string); ok {
						return logFile
					}
				}
			}
		}
	}
	return ""
}

func (h *DisplayLogsHelper) copyAndDisplayLogFile(tc storm.TestCase, logFilePath string, artifactFileName string) error {
	logrus.Infof("== Copy Log from %s to %s ==", logFilePath, h.args.ArtifactsFolder)
	err := copyFileToArtifactsFolder(logFilePath, h.args.ArtifactsFolder, artifactFileName)
	if err != nil {
		return err
	}

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
	serialLogFile := getSerialPathFromNetlistenConfig(h.args.NetlistenConfig)
	if serialLogFile == "" {
		// Look for a file in the fallback folder that ends with the specified suffix
		entries, err := os.ReadDir(h.args.SerialLogFallbackFolder)
		if err != nil {
			tc.Skip("No serial log file specified and cannot read fallback folder")
			return nil
		}

		var matchingFiles []string
		for _, entry := range entries {
			if !entry.IsDir() && strings.HasSuffix(entry.Name(), h.args.SerialLogFallbackFileSuffix) {
				matchingFiles = append(matchingFiles, entry.Name())
			}
		}

		if len(matchingFiles) == 0 {
			tc.Skip("No serial log file specified and no matching fallback file found")
			return nil
		} else if len(matchingFiles) > 1 {
			logrus.Warnf("Multiple files found ending with '%s': %v, using first one", h.args.SerialLogFallbackFileSuffix, matchingFiles)
		}

		serialLogFile = filepath.Join(h.args.SerialLogFallbackFolder, matchingFiles[0])
		logrus.Tracef("Using fallback serial log file: %s", serialLogFile)
	}

	return h.copyAndDisplayLogFile(tc, serialLogFile, h.args.SerialLogArtifactFileName)
}

func (h *DisplayLogsHelper) displayTridentLogFile(tc storm.TestCase, logFile string, skipMessage string) error {
	if logFile == "" {
		tc.Skip(skipMessage)
		return nil
	}

	return h.copyAndDisplayLogFile(tc, logFile, filepath.Base(logFile))
}

func (h *DisplayLogsHelper) displayTrident(tc storm.TestCase) error {
	return h.displayTridentLogFile(tc, h.args.TridentLogFile, "No trident log file specified")
}

func (h *DisplayLogsHelper) displayTraceTrident(tc storm.TestCase) error {
	return h.displayTridentLogFile(tc, h.args.TridentTraceLogFile, "No trident trace log file specified")
}
