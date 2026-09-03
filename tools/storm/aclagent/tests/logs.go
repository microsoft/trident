package tests

import (
	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func FetchLogs(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.FetchLogs(vmConfig, testConfig.OutputPath)
}
