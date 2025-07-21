// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package configuration

import (
	"encoding/json"
	"os"
)

const (
	fileName = "user_input.json"
)

type UserInput struct {
	DiskPath string `json:"disk_path"`
	Hostname string `json:"hostname"`
	Password string `json:"password"`
	Username string `json:"username"`
}

func NewUserInput() *UserInput {
	return &UserInput{}
}

func (u *UserInput) Save() error {
	file, err := os.Create(fileName)
	if err != nil {
		return err
	}
	defer file.Close()
	encoder := json.NewEncoder(file)
	encoder.SetIndent("", "  ")
	return encoder.Encode(u)
}
