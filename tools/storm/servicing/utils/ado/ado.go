// Package storm provides helpers for Trident loop-update Storm tests.
// This file contains helpers converted from Bash scripts in scripts/loop-update.
package ado

import (
	"fmt"
)

func LogError(msg string, a ...any) {
	fmt.Printf("##vso[task.logissue type=error]%s\n", fmt.Sprintf(msg, a...))
}
