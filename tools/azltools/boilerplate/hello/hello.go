// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package hello

import "tridenttools/azltools/internal/timestamp"

// World is a sample public (starts with a capital letter, must be commented) function.
func World() string {
	timestamp.StartEvent("hello world", nil)
	defer timestamp.StopEvent(nil)

	return "Hello, world!"
}
