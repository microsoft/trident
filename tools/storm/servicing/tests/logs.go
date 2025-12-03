package tests

import (
	stormsvcconfig "tridenttools/storm/servicing/utils/config"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func FetchLogs(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.FetchLogs(vmConfig, testConfig.OutputPath)
}
