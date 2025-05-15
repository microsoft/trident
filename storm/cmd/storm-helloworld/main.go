package main

import (
	"storm"

	"storm/helloworld"
)

func main() {
	storm := storm.CreateSuite("hello-world")

	// Add hello world scenario
	storm.AddScenario(&helloworld.HelloWorldScenario{})

	// Add hello world helper
	storm.AddHelper(&helloworld.HelloWorldHelper{})

	storm.Run()
}
