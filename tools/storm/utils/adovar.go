package utils

import "fmt"

func SetAzureDevopsVariables(name string, value string) {
	fmt.Printf("##vso[task.setvariable variable=%s]%s\n", name, value)
}

func SetAzureDevopsOutputVariable(name string, value string) {
	fmt.Printf("##vso[task.setvariable variable=%s;isOutput=true]%s\n", name, value)
}

// LogAzureDevopsWarning raises a warning against the running task so it appears
// in the pipeline summary rather than only in the step log.
func LogAzureDevopsWarning(message string) {
	fmt.Printf("##vso[task.logissue type=warning]%s\n", message)
}
