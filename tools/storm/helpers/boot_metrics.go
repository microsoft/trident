package helpers

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"strconv"
	"strings"
	"time"
	"tridenttools/storm/utils/env"
	stormenv "tridenttools/storm/utils/env"
	"tridenttools/storm/utils/retry"
	sshclient "tridenttools/storm/utils/ssh/client"
	sshconfig "tridenttools/storm/utils/ssh/config"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
)

type BootMetricsHelper struct {
	args struct {
		sshconfig.SshCliSettings `embed:""`
		env.EnvCliSettings       `embed:""`
		MetricsFile              string `required:"" help:"Metrics file." type:"path"`
		MetricsOperation         string `required:"" help:"Metrics operation."`
	}
}

type BootMetric struct {
	Operation   string  `json:"operation"`
	FirmwareMs  float64 `json:"firmware,omitempty"`
	LoaderMs    float64 `json:"loader,omitempty"`
	KernelMs    float64 `json:"kernel,omitempty"`
	InitrdMs    float64 `json:"initrd,omitempty"`
	UserspaceMs float64 `json:"userspace,omitempty"`
}

type BootMetrics struct {
	Timestamp        string                 `json:"timestamp"`
	MetricName       string                 `json:"metric_name"`
	Value            BootMetric             `json:"value"`
	AdditionalFields map[string]interface{} `json:"additional_fields"`
	PlatformInfo     map[string]interface{} `json:"platform_info"`
}

func (h BootMetricsHelper) Name() string {
	return "boot-metrics"
}

func (h *BootMetricsHelper) Args() any {
	return &h.args
}

func (h *BootMetricsHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("collect-boot-metrics", h.collectBootMetrics)
	return nil
}

func (h *BootMetricsHelper) collectBootMetrics(tc storm.TestCase) error {
	if h.args.Env == stormenv.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}
	logrus.Infof("Waiting for the host to reboot and come back online...")

	result, err := h.initializeBootMetrics(tc, h.args.MetricsFile)
	if err != nil {
		tc.FailFromError(err)
	}

	value, err := retry.Retry(
		time.Second*time.Duration(h.args.Timeout),
		time.Second*5,
		func(attempt int) (*BootMetric, error) {
			var err error = nil
			result := BootMetric{}
			client, err := sshclient.OpenSshClient(h.args.SshCliSettings)
			if err != nil {
				return &result, err
			}

			// Expect output in the form of:
			//   Startup finished in [13.022s (firmware) + 2.552s (loader) + ]? 4.740s (kernel) + 1.267s (initrd) + 15.249s (userspace) = 35.565s
			//   graphical.target reached after 13.272s in userspace
			systemdAnalzeBootResult, err := sshclient.RunCommand(client, "systemd-analyze | head -n 1")
			if err != nil {
				return &result, err
			}
			systemdAnalzeBootOutput := systemdAnalzeBootResult.Stdout

			result.Operation = h.args.MetricsOperation

			if firmwareBoot, units, firmwareBootExists := h.findWordBeforeMatch(tc, systemdAnalzeBootOutput, "(firmware)"); firmwareBootExists {
				result.FirmwareMs, err = h.ensureMilliseconds(tc, firmwareBoot, units)
				if err != nil {
					return &result, err
				}
			}
			if loaderBoot, units, loaderBootExists := h.findWordBeforeMatch(tc, systemdAnalzeBootOutput, "(loader)"); loaderBootExists {
				result.LoaderMs, err = h.ensureMilliseconds(tc, loaderBoot, units)
				if err != nil {
					return &result, err
				}
			}
			if kernelBoot, units, kernelBootExists := h.findWordBeforeMatch(tc, systemdAnalzeBootOutput, "(kernel)"); kernelBootExists {
				result.KernelMs, err = h.ensureMilliseconds(tc, kernelBoot, units)
				if err != nil {
					return &result, err
				}
			}
			if initrdBoot, units, initrdBootExists := h.findWordBeforeMatch(tc, systemdAnalzeBootOutput, "(initrd)"); initrdBootExists {
				result.InitrdMs, err = h.ensureMilliseconds(tc, initrdBoot, units)
				if err != nil {
					return &result, err
				}
			}
			if userspaceBoot, units, userspaceBootExists := h.findWordBeforeMatch(tc, systemdAnalzeBootOutput, "(userspace)"); userspaceBootExists {
				result.UserspaceMs, err = h.ensureMilliseconds(tc, userspaceBoot, units)
				if err != nil {
					return &result, err
				}
			}
			return &result, err
		},
	)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	result.Value = *value

	jsonBytes, err := json.Marshal(result)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	file, err := os.OpenFile(h.args.MetricsFile, os.O_APPEND|os.O_WRONLY|os.O_CREATE, 0600)
	if err != nil {
		return err
	}
	defer file.Close()

	_, err = file.WriteString(string(jsonBytes) + "\n")
	if err != nil {
		return err
	}

	return nil
}

func (h *BootMetricsHelper) ensureMilliseconds(tc storm.TestCase, value string, unit string) (float64, error) {
	valueAsDouble, err := strconv.ParseFloat(value, 64)
	if err != nil {
		logrus.Infof("Failed to parse value: %s", value)
		return 0, fmt.Errorf("failed to parse value: %s", value)
	}

	switch unit {
	case "s":
		return valueAsDouble * 1000, nil
	case "ms":
		return valueAsDouble, nil
	case "m":
		return valueAsDouble * 60 * 1000, nil
	case "ns":
		return valueAsDouble / 1000000, nil
	}

	return 0, fmt.Errorf("unknown time unit: %s", unit)
}

func (h *BootMetricsHelper) initializeBootMetrics(tc storm.TestCase, metricsFile string) (BootMetrics, error) {
	result := BootMetrics{}

	// Open metrics file
	file, err := os.OpenFile(h.args.MetricsFile, os.O_RDONLY, os.ModeAppend)
	if err != nil {
		return result, err
	}
	defer file.Close()

	// Read only the first line of the file and parse it as JSON
	// This is to ensure that the file is not empty and contains valid JSON
	// Read the first line
	scanner := bufio.NewScanner(file)
	if !scanner.Scan() {
		return result, fmt.Errorf("failed to read the first line of the file")
	}

	// Decode the first line as JSON
	decoder := json.NewDecoder(strings.NewReader(scanner.Text()))
	var firstRecord map[string]interface{}
	err = decoder.Decode(&firstRecord)
	if err != nil {
		return result, err
	}

	result.Timestamp = time.Now().Format(time.RFC3339)
	result.MetricName = "boot_info"

	// Get additional fields from first record
	if firstRecord["additional_fields"] != nil {
		additionalFields, ok := firstRecord["additional_fields"].(map[string]interface{})
		if ok {
			result.AdditionalFields = additionalFields
		}
	}
	// Get platform info from first record
	if firstRecord["platform_info"] != nil {
		platformInfo, ok := firstRecord["platform_info"].(map[string]interface{})
		if ok {
			result.PlatformInfo = platformInfo
		}
	}
	return result, nil
}

func (h *BootMetricsHelper) findWordBeforeMatch(tc storm.TestCase, text, target string) (string, string, bool) {
	re := regexp.MustCompile(`([-+]?\d*\.?\d+)(.)\s+` + regexp.QuoteMeta(target))
	match := re.FindStringSubmatch(text)
	if len(match) >= 3 {
		return match[1], match[2], true
	}
	return "", "", false
}
