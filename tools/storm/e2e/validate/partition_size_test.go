package validate

import (
	"testing"

	"tridenttools/storm/utils/sysinspect"
)

func TestValidatePartitionSizeComparesRealBytes(t *testing.T) {
	lsblk, err := sysinspect.ParseLsblk(`{"blockdevices":[{"name":"sda","size":1000,"children":[{"name":"sda1","size":1048576}]}]}`)
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	m := map[string]string{"esp": "sda1"}

	var ok SoftAsserter
	validatePartitionSize(&ok, lsblk, m, PartitionExpectation{ID: "esp", SizeBytes: 1048576, HasSize: true})
	if ok.HasFailures() {
		t.Errorf("matching size failed: %s", ok.Summary())
	}

	var bad SoftAsserter
	validatePartitionSize(&bad, lsblk, m, PartitionExpectation{ID: "esp", SizeBytes: 2097152, HasSize: true})
	if !bad.HasFailures() {
		t.Error("mismatched size passed; the assertion is vacuous")
	}

	var grow SoftAsserter
	validatePartitionSize(&grow, lsblk, m, PartitionExpectation{ID: "esp", HasSize: false})
	if grow.Failures() != 0 {
		t.Error("grow partition should be skipped")
	}
}
