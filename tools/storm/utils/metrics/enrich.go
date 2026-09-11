// Package metrics ports the enrichment half of the legacy
// tests/e2e_tests/helpers/process_trident_metrics.py: it decorates the
// trace-stream records netlisten captures so they carry the pipeline and
// Host Configuration context the Kusto `trident` table expects.
package metrics

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"tridenttools/pkg/hostconfig"
)

// A trace-stream record is
//
//	{timestamp, metric_name, value, additional_fields, platform_info}
//
// where the last two are *dynamic* columns in Kusto. Enrichment therefore only
// ever adds keys INSIDE those two objects: adding a new top-level field would
// require a Kusto mapping change and break ingestion.
const (
	platformInfoKey     = "platform_info"
	additionalFieldsKey = "additional_fields"
)

// UnknownValue is what legacy wrote for any pipeline value absent from the
// environment. Preserved verbatim so storm-produced rows sort alongside the
// legacy ones instead of carrying empty strings.
const UnknownValue = "Unknown"

// Enrichment is the set of keys added to every record of a metrics file.
type Enrichment struct {
	// PlatformInfo keys are merged into the record's platform_info object.
	PlatformInfo map[string]any

	// AdditionalFields keys are merged into the record's additional_fields
	// object.
	AdditionalFields map[string]any
}

// PipelineContext carries the pipeline-provided values legacy read from the
// environment. Callers populate it from CLI args, which may in turn be bound to
// the same environment variables the legacy script read.
type PipelineContext struct {
	PipelineName      string
	PipelineBuildId   string
	PipelineAgentSku  string
	Environment       string
	Location          string
	ServerName        string
	Branch            string
	MachineType       string
	RuntimeEnv        string
	TridentConfigName string
	TridentCommitHash string

	// TestRunner distinguishes storm-produced rows from the legacy suite's so
	// dashboards can filter while both suites run in parallel.
	TestRunner string
}

// NewEnrichment builds the enrichment for a run, combining the pipeline context
// with the feature flags implied by the Host Configuration under test.
//
// Feature flags are only ever set to true, never to false — matching legacy,
// which omits the key entirely when the feature is absent.
func NewEnrichment(pipeline PipelineContext, config *hostconfig.HostConfig) Enrichment {
	e := Enrichment{
		PlatformInfo: map[string]any{
			"pipeline_name":      pipeline.PipelineName,
			"pipeline_build_id":  pipeline.PipelineBuildId,
			"pipeline_agent_sku": pipeline.PipelineAgentSku,
			"environment":        pipeline.Environment,
			"location":           pipeline.Location,
			"server_name":        pipeline.ServerName,
			"branch":             pipeline.Branch,
			"machine_type":       pipeline.MachineType,
			"runtime_env":        pipeline.RuntimeEnv,
		},
		AdditionalFields: map[string]any{
			"trident_config_name": pipeline.TridentConfigName,
			"trident_commit_hash": pipeline.TridentCommitHash,
		},
	}

	if pipeline.TestRunner != "" {
		e.AdditionalFields["test_runner"] = pipeline.TestRunner
	}

	if config != nil {
		for key, enabled := range map[string]bool{
			"raid_enabled":       config.HasRaid(),
			"encryption_enabled": config.HasEncryption(),
			"abUpdate_enabled":   config.HasABUpdate(),
			"verity_enabled":     config.HasVerity(),
		} {
			if enabled {
				e.AdditionalFields[key] = true
			}
		}
	}

	return e
}

// LoadHostConfig reads a Host Configuration YAML file so its feature flags can
// be derived through the typed getters rather than by poking at raw YAML keys.
func LoadHostConfig(path string) (hostconfig.HostConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return hostconfig.HostConfig{}, fmt.Errorf("failed to read Host Configuration '%s': %w", path, err)
	}

	config, err := hostconfig.NewHostConfigFromYaml(data)
	if err != nil {
		return hostconfig.HostConfig{}, fmt.Errorf("failed to parse Host Configuration '%s': %w", path, err)
	}
	return config, nil
}

// EnrichFile rewrites every record of a JSONL metrics file with the given
// enrichment applied, and reports how many records were rewritten.
//
// The rewrite is idempotent: re-running it over an already-enriched file
// produces the same content, so it is safe to invoke more than once for the
// same file.
func EnrichFile(path string, e Enrichment) (int, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return 0, fmt.Errorf("failed to read metrics file '%s': %w", path, err)
	}

	records, err := enrichRecords(data, e)
	if err != nil {
		return 0, fmt.Errorf("failed to enrich metrics file '%s': %w", path, err)
	}

	// Write through a temporary file in the same directory so a failure
	// part-way through cannot leave a truncated metrics file behind to be
	// ingested.
	tmp, err := os.CreateTemp(filepath.Dir(path), filepath.Base(path)+".*.tmp")
	if err != nil {
		return 0, fmt.Errorf("failed to create temporary file for '%s': %w", path, err)
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)

	for _, record := range records {
		if _, err := tmp.Write(append(record, '\n')); err != nil {
			tmp.Close()
			return 0, fmt.Errorf("failed to write enriched metrics for '%s': %w", path, err)
		}
	}
	if err := tmp.Close(); err != nil {
		return 0, fmt.Errorf("failed to close temporary file for '%s': %w", path, err)
	}
	if err := os.Rename(tmpName, path); err != nil {
		return 0, fmt.Errorf("failed to replace metrics file '%s': %w", path, err)
	}

	return len(records), nil
}

// enrichRecords decodes the trace-stream content and returns one serialized
// record per entry. netlisten writes records back-to-back rather than as a JSON
// array, so a streaming decoder is used instead of splitting on newlines.
func enrichRecords(data []byte, e Enrichment) ([][]byte, error) {
	dec := json.NewDecoder(bytes.NewReader(data))

	var out [][]byte
	for dec.More() {
		var record map[string]any
		if err := dec.Decode(&record); err != nil {
			return nil, fmt.Errorf("failed to decode record %d: %w", len(out)+1, err)
		}

		mergeInto(record, platformInfoKey, e.PlatformInfo)
		mergeInto(record, additionalFieldsKey, e.AdditionalFields)

		encoded, err := json.Marshal(record)
		if err != nil {
			return nil, fmt.Errorf("failed to encode record %d: %w", len(out)+1, err)
		}
		out = append(out, encoded)
	}

	return out, nil
}

// mergeInto merges values into the nested object stored at key, creating the
// object when the record does not already carry one. Legacy raised a KeyError
// in that case; creating it keeps enrichment total, since a record missing the
// object would otherwise reach Kusto with none of the dynamic-column context.
func mergeInto(record map[string]any, key string, values map[string]any) {
	nested, ok := record[key].(map[string]any)
	if !ok {
		nested = map[string]any{}
		record[key] = nested
	}
	for k, v := range values {
		nested[k] = v
	}
}
