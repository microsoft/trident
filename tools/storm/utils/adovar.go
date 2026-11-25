package utils

import "fmt"

// LogAzureDevopsWarning raises a warning against the running task so it appears
// in the pipeline summary rather than only in the step log.
func LogAzureDevopsWarning(message string) {
	fmt.Printf("##vso[task.logissue type=warning]%s\n", message)
}
