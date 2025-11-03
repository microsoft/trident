package utils

import "fmt"

func SetAzureDevopsVariables(name string, value string) {
	fmt.Printf("##vso[task.setvariable variable=%s]%s\n", name, value)
}

func SetAzureDevopsOutputVariable(name string, value string) {
	fmt.Printf("##vso[task.setvariable variable=%s;isOutput=true]%s\n", name, value)
}
