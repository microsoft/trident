package runner

import (
	"storm/pkg/storm/core"
)

type runnableContext struct {
	core.TestRegistrantMetadata
	core.LoggerProvider
}
