package trident

import (
	"fmt"

	"github.com/Jeffail/gabs/v2"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"

	"tridenttools/pkg/hostconfig"
)

// ServicingState mirrors trident_api::status::ServicingState (kebab-case). It
// is the value reported under `servicingState` in `trident get` output.
type ServicingState string

const (
	ServicingStateNotProvisioned              ServicingState = "not-provisioned"
	ServicingStateCleanInstallStaged          ServicingState = "clean-install-staged"
	ServicingStateAbUpdateStaged              ServicingState = "ab-update-staged"
	ServicingStateManualRollbackAbStaged      ServicingState = "manual-rollback-ab-staged"
	ServicingStateManualRollbackRuntimeStaged ServicingState = "manual-rollback-runtime-staged"
	ServicingStateRuntimeUpdateStaged         ServicingState = "runtime-update-staged"
	ServicingStateCleanInstallFinalized       ServicingState = "clean-install-finalized"
	ServicingStateAbUpdateFinalized           ServicingState = "ab-update-finalized"
	ServicingStateManualRollbackAbFinalized   ServicingState = "manual-rollback-ab-finalized"
	ServicingStateProvisioned                 ServicingState = "provisioned"
	ServicingStateAbUpdateHealthCheckFailed   ServicingState = "ab-update-health-check-failed"
)

// AbVolumeSelection mirrors trident_api::status::AbVolumeSelection (kebab-case).
// It is the value reported under `abActiveVolume` in `trident get` output.
type AbVolumeSelection string

const (
	AbVolumeA AbVolumeSelection = "volume-a"
	AbVolumeB AbVolumeSelection = "volume-b"
)

// Other returns the opposite A/B volume selection.
func (v AbVolumeSelection) Other() AbVolumeSelection {
	if v == AbVolumeA {
		return AbVolumeB
	}
	return AbVolumeA
}

// HostStatus is a hybrid view over the YAML emitted by `trident get`. It wraps a
// gabs.Container (escape hatch for rarely-touched corners) and provides typed
// accessors for the stable core fields that E2E validations depend on.
type HostStatus struct {
	*gabs.Container
}

// NewHostStatusFromYaml parses `trident get` YAML output into a HostStatus.
//
// The Host Status embeds a Host Configuration under `spec` whose `contents`
// nodes carry custom YAML tags (e.g. `!image`). yaml.v3 decodes these into
// plain maps when the target is `map[string]any`, so no custom tag constructor
// is required (unlike the Python suite's yaml.add_multi_constructor).
func NewHostStatusFromYaml(yamlData []byte) (HostStatus, error) {
	var data map[string]any
	if err := yaml.Unmarshal(yamlData, &data); err != nil {
		return HostStatus{}, fmt.Errorf("failed to unmarshal Host Status YAML: %w", err)
	}

	return HostStatus{Container: gabs.Wrap(data)}, nil
}

// GetHostStatus runs `trident get` on the host over the provided SSH client and
// parses the result into a HostStatus.
func GetHostStatus(runtime RuntimeType, client *ssh.Client) (HostStatus, error) {
	out, err := InvokeTrident(runtime, client, nil, "get")
	if err != nil {
		return HostStatus{}, fmt.Errorf("failed to invoke 'trident get': %w", err)
	}
	if err := out.Check(); err != nil {
		return HostStatus{}, fmt.Errorf("'trident get' failed: %s", out.Report())
	}

	return NewHostStatusFromYaml([]byte(out.Stdout))
}

// ServicingState returns the current servicing state of the host.
func (hs *HostStatus) ServicingState() ServicingState {
	s, _ := hs.S("servicingState").Data().(string)
	return ServicingState(s)
}

// AbActiveVolume returns the active A/B volume and whether it is present. It is
// absent on hosts that were never A/B updated (or failed before provisioning).
func (hs *HostStatus) AbActiveVolume() (AbVolumeSelection, bool) {
	s, ok := hs.S("abActiveVolume").Data().(string)
	if !ok {
		return "", false
	}
	return AbVolumeSelection(s), true
}

// PartitionPaths returns the device path of each block device, keyed by device
// ID (the `partitionPaths` map).
func (hs *HostStatus) PartitionPaths() map[string]string {
	result := make(map[string]string)
	for id, child := range hs.S("partitionPaths").ChildrenMap() {
		if path, ok := child.Data().(string); ok {
			result[id] = path
		}
	}
	return result
}

// Spec returns the embedded Host Configuration (`spec`) as a HostConfig, reusing
// the existing gabs-backed configuration handling for storage introspection.
func (hs *HostStatus) Spec() hostconfig.HostConfig {
	return hostconfig.NewHostConfigFromContainer(hs.S("spec"))
}

// LastError returns the serialized YAML of the `lastError` field and whether it
// is present. Validations match substrings against it (e.g. rollback checks).
func (hs *HostStatus) LastError() (string, bool) {
	container := hs.S("lastError")
	if container == nil || container.Data() == nil {
		return "", false
	}

	raw, err := yaml.Marshal(container.Data())
	if err != nil {
		return "", false
	}
	return string(raw), true
}
