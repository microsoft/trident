package metrics

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"tridenttools/pkg/hostconfig"
)

func testPipeline() PipelineContext {
	return PipelineContext{
		PipelineName:      "trident-ci",
		PipelineBuildId:   "1175236",
		PipelineAgentSku:  UnknownValue,
		Environment:       UnknownValue,
		Location:          UnknownValue,
		ServerName:        UnknownValue,
		Branch:            "storm-port",
		MachineType:       "vm",
		RuntimeEnv:        "host",
		TridentConfigName: "base",
		TridentCommitHash: "ec782a5d",
		TestRunner:        "storm",
	}
}

func writeMetrics(t *testing.T, content string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "metrics.jsonl")
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("write metrics file: %v", err)
	}
	return path
}

func readRecords(t *testing.T, path string) []map[string]any {
	t.Helper()
	f, err := os.Open(path)
	if err != nil {
		t.Fatalf("open metrics file: %v", err)
	}
	defer f.Close()

	var out []map[string]any
	dec := json.NewDecoder(f)
	for dec.More() {
		var record map[string]any
		if err := dec.Decode(&record); err != nil {
			t.Fatalf("decode record: %v", err)
		}
		out = append(out, record)
	}
	return out
}

func configFromYaml(t *testing.T, yaml string) hostconfig.HostConfig {
	t.Helper()
	config, err := hostconfig.NewHostConfigFromYaml([]byte(yaml))
	if err != nil {
		t.Fatalf("parse host config: %v", err)
	}
	return config
}

func TestNewEnrichmentOnlySetsEnabledFeatureFlags(t *testing.T) {
	config := configFromYaml(t, `
storage:
  abUpdate:
    volumePairs: []
  encryption:
    volumes: []
`)

	e := NewEnrichment(testPipeline(), &config)

	for _, key := range []string{"abUpdate_enabled", "encryption_enabled"} {
		if e.AdditionalFields[key] != true {
			t.Errorf("expected %s to be true, got %v", key, e.AdditionalFields[key])
		}
	}
	// Legacy omits the key entirely rather than writing false.
	for _, key := range []string{"raid_enabled", "verity_enabled"} {
		if _, present := e.AdditionalFields[key]; present {
			t.Errorf("expected %s to be absent for a config without it", key)
		}
	}
	if e.AdditionalFields["test_runner"] != "storm" {
		t.Errorf("expected test_runner to be recorded, got %v", e.AdditionalFields["test_runner"])
	}
}

func TestEnrichFileAddsKeysInsideNestedObjectsOnly(t *testing.T) {
	path := writeMetrics(t,
		`{"timestamp":"t1","metric_name":"a","value":1,"additional_fields":{"existing":"kept"},"platform_info":{"os":"azurelinux"}}`+"\n"+
			`{"timestamp":"t2","metric_name":"b","value":2,"additional_fields":{},"platform_info":{}}`+"\n")

	config := configFromYaml(t, "storage:\n  raid:\n    software: []\n")
	count, err := EnrichFile(path, NewEnrichment(testPipeline(), &config))
	if err != nil {
		t.Fatalf("EnrichFile: %v", err)
	}
	if count != 2 {
		t.Fatalf("expected 2 records, got %d", count)
	}

	records := readRecords(t, path)
	if len(records) != 2 {
		t.Fatalf("expected 2 records on disk, got %d", len(records))
	}

	// A new top-level field would require a Kusto mapping change, so the
	// top-level key set must be exactly what came in.
	for i, record := range records {
		if len(record) != 5 {
			t.Errorf("record %d gained top-level fields: %v", i, record)
		}
	}

	first := records[0]
	platform := first["platform_info"].(map[string]any)
	if platform["os"] != "azurelinux" {
		t.Error("enrichment dropped a pre-existing platform_info key")
	}
	if platform["pipeline_build_id"] != "1175236" || platform["branch"] != "storm-port" {
		t.Errorf("platform_info not enriched: %v", platform)
	}

	additional := first["additional_fields"].(map[string]any)
	if additional["existing"] != "kept" {
		t.Error("enrichment dropped a pre-existing additional_fields key")
	}
	if additional["trident_config_name"] != "base" || additional["raid_enabled"] != true {
		t.Errorf("additional_fields not enriched: %v", additional)
	}
}

func TestEnrichFileIsIdempotent(t *testing.T) {
	const content = `{"timestamp":"t1","metric_name":"a","value":1,"additional_fields":{},"platform_info":{}}` + "\n"
	path := writeMetrics(t, content)

	config := configFromYaml(t, "storage: {}\n")
	e := NewEnrichment(testPipeline(), &config)

	if _, err := EnrichFile(path, e); err != nil {
		t.Fatalf("first EnrichFile: %v", err)
	}
	first, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read after first pass: %v", err)
	}

	if _, err := EnrichFile(path, e); err != nil {
		t.Fatalf("second EnrichFile: %v", err)
	}
	second, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read after second pass: %v", err)
	}

	if string(first) != string(second) {
		t.Errorf("enrichment is not idempotent:\nfirst:  %s\nsecond: %s", first, second)
	}
}

func TestEnrichFileCreatesMissingNestedObjects(t *testing.T) {
	path := writeMetrics(t, `{"timestamp":"t1","metric_name":"a","value":1}`+"\n")

	if _, err := EnrichFile(path, NewEnrichment(testPipeline(), nil)); err != nil {
		t.Fatalf("EnrichFile: %v", err)
	}

	record := readRecords(t, path)[0]
	platform, ok := record["platform_info"].(map[string]any)
	if !ok || platform["machine_type"] != "vm" {
		t.Errorf("expected platform_info to be created and populated, got %v", record["platform_info"])
	}
	additional, ok := record["additional_fields"].(map[string]any)
	if !ok || additional["trident_commit_hash"] != "ec782a5d" {
		t.Errorf("expected additional_fields to be created and populated, got %v", record["additional_fields"])
	}
}

func TestEnrichFileLeavesFileIntactOnMalformedContent(t *testing.T) {
	const content = `{"metric_name":"a"}` + "\n" + `{"metric_name":` + "\n"
	path := writeMetrics(t, content)

	if _, err := EnrichFile(path, NewEnrichment(testPipeline(), nil)); err == nil {
		t.Fatal("expected an error for malformed trace-stream content")
	}

	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read after failed enrichment: %v", err)
	}
	if string(after) != content {
		t.Errorf("a failed enrichment must not modify the file, got: %s", after)
	}
}
