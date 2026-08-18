package tests

import (
	stormaksconfig "tridenttools/storm/aksagent/utils/config"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func FetchLogs(testConfig stormaksconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.FetchLogs(vmConfig, testConfig.OutputPath)
}
