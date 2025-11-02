package utils

import "fmt"

func SetAzureDevopsVariables(name string, value string) {
	// Escape any special characters in name and value if necessary
	
	fmt.Printf("##vso[task.setvariable variable=%s]%s\n", name, value)
}

