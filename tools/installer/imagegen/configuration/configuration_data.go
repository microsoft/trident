// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package configuration

// Information to update Trident's Host Configuration template.
type TridentConfigData struct {
	ImagePath      string
	DiskPath       string
	Hostname       string
	Username       string
	Password       string
	PasswordScript string
}

func NewTridentConfigData() *TridentConfigData {
	return &TridentConfigData{}
}
