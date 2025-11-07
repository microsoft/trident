package display_logs

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type DisplayLogsScriptSet struct {
	DisplayLogs DisplayLogsScript `cmd:"" help:"Displays and copies serial, trident, and trace logs to artifacts folder."`
}

type DisplayLogsScript struct {
	SkipSerialLog               bool   `help:"Skip displaying serial log." default:"false"`
	NetlistenConfig             string `help:"Path to netlisten config file."`
	SerialLogFallbackFolder     string `help:"Folder to search for serial log files." default:"/tmp"`
	SerialLogFallbackFileSuffix string `help:"File suffix to match when searching for serial log files in fallback folder." default:"serial0.log"`
	SerialLogArtifactFileName   string `help:"Filename to use when copying serial log to artifacts folder."`
	TridentLogFile              string `help:"File containing trident log output." default:""`
	TridentTraceLogFile         string `help:"File containing trace log output." default:""`
	ArtifactsFolder             string `help:"Folder to copy log files into."`
}

func (s *DisplayLogsScript) Run() error {
	err := s.displaySerial()
	if err != nil {
		return err
	}
	err = s.displayTrident()
	if err != nil {
		return err
	}
	err = s.displayTraceTrident()
	if err != nil {
		return err
	}

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

func (s *DisplayLogsScript) copyAndDisplayLogFile(logFilePath string, artifactFileName string) error {
	logrus.Infof("== Copy Log from %s to %s ==", logFilePath, s.ArtifactsFolder)
	err := copyFileToArtifactsFolder(logFilePath, s.ArtifactsFolder, artifactFileName)
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

func (s *DisplayLogsScript) displaySerial() error {
	if s.SkipSerialLog {
		logrus.Info("Skipping serial log.")
		return nil
	}
	serialLogFile := getSerialPathFromNetlistenConfig(s.NetlistenConfig)
	if serialLogFile == "" {
		// Look for a file in the fallback folder that ends with the specified suffix
		entries, err := os.ReadDir(s.SerialLogFallbackFolder)
		if err != nil {
			logrus.Info("No serial log file specified and cannot read fallback folder")
			return nil
		}

		var matchingFiles []string
		for _, entry := range entries {
			if !entry.IsDir() && strings.HasSuffix(entry.Name(), s.SerialLogFallbackFileSuffix) {
				matchingFiles = append(matchingFiles, entry.Name())
			}
		}

		if len(matchingFiles) == 0 {
			logrus.Info("No serial log file specified and no matching fallback file found")
			return nil
		} else if len(matchingFiles) > 1 {
			logrus.Warnf("Multiple files found ending with '%s': %v, using first one", s.SerialLogFallbackFileSuffix, matchingFiles)
		}

		serialLogFile = filepath.Join(s.SerialLogFallbackFolder, matchingFiles[0])
		logrus.Tracef("Using fallback serial log file: %s", serialLogFile)
	}

	return s.copyAndDisplayLogFile(serialLogFile, s.SerialLogArtifactFileName)
}

func (s *DisplayLogsScript) displayTridentLogFile(logFile string, skipMessage string) error {
	if logFile == "" {
		logrus.Info(skipMessage)
		return nil
	}

	return s.copyAndDisplayLogFile(logFile, filepath.Base(logFile))
}

func (s *DisplayLogsScript) displayTrident() error {
	return s.displayTridentLogFile(s.TridentLogFile, "No trident log file specified")
}

func (s *DisplayLogsScript) displayTraceTrident() error {
	return s.displayTridentLogFile(s.TridentTraceLogFile, "No trident trace log file specified")
}
