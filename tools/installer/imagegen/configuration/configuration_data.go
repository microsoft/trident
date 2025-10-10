// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package configuration

import (
	"fmt"

	"golang.org/x/crypto/bcrypt"
)

// Information to update Trident's Host Configuration template.
type TridentConfigData struct {
	ImagePath      string
	DiskPath       string
	Hostname       string
	Username       string
	HashedPassword string
	PasswordScript string
}

func NewTridentConfigData() *TridentConfigData {
	return &TridentConfigData{}
}

// SetPassword securely hashes the provided password and stores it in the configuration.
// The original password is not stored in memory after hashing.
// Returns an error if hashing fails, without exposing the password in the error message.
func (tcd *TridentConfigData) SetPassword(password string) error {
	if password == "" {
		return fmt.Errorf("password cannot be empty")
	}

	hashedPassword, err := bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)
	if err != nil {
		return fmt.Errorf("failed to hash password")
	}

	tcd.HashedPassword = string(hashedPassword)
	return nil
}
