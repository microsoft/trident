package tests

import (
	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func FetchLogs(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.FetchLogs(vmConfig, testConfig.OutputPath)
}
