// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package configuration

import (
	"encoding/json"
	"os"
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

func (u *UserInput) Save(fileName string) error {
	file, err := os.Create(fileName)
	if err != nil {
		return err
	}
	defer file.Close()
	encoder := json.NewEncoder(file)
	encoder.SetIndent("", "  ")
	return encoder.Encode(u)
}
