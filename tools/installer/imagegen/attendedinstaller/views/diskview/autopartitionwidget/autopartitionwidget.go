// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package autopartitionwidget

import (
	"fmt"

	"github.com/gdamore/tcell"
	"github.com/rivo/tview"

	"installer/imagegen/attendedinstaller/primitives/customshortcutlist"
	"installer/imagegen/attendedinstaller/primitives/navigationbar"
	"installer/imagegen/attendedinstaller/uitext"
	"installer/imagegen/attendedinstaller/uiutils"
	"installer/imagegen/configuration"
	"installer/imagegen/diskutils"
	"installer/internal/logger"
)

// UI constants.
const (
	nextButtonIndex = 1 // Changed from 1 to 2 when reenabling Custom partitions button
	defaultPadding  = 1

	textProportion = 0
	listProportion = 0

	navBarHeight     = 0
	navBarProportion = 1
)

// AutoPartitionWidget contains the disk selection UI
type AutoPartitionWidget struct {
	navBar       *navigationbar.NavigationBar
	flex         *tview.Flex
	centeredFlex *tview.Flex
	deviceList   *customshortcutlist.List
	helpText     *tview.TextView

	systemDevices  []diskutils.SystemBlockDevice
	hostConfigData *configuration.TridentConfigData
	bootType       string
}

// New creates and returns a new AutoPartitionWidget.
func New(systemDevices []diskutils.SystemBlockDevice, bootType string) *AutoPartitionWidget {
	return &AutoPartitionWidget{
		systemDevices: systemDevices,
		bootType:      bootType,
	}
}

// Initialize initializes the view.
func (ap *AutoPartitionWidget) Initialize(hostConfigData *configuration.TridentConfigData, backButtonText string, app *tview.Application, switchMode, nextPage, previousPage, quit, refreshTitle func()) (err error) {
	ap.hostConfigData = hostConfigData
	ap.navBar = navigationbar.NewNavigationBar().
		AddButton(backButtonText, previousPage).
		// AddButton(uitext.DiskButtonCustom, switchMode). // Temporarily disabled manual partitioning
		AddButton(uitext.ButtonNext, func() {
			ap.saveSelectedDevice()
			nextPage()
		}).
		SetAlign(tview.AlignCenter)

	ap.deviceList = customshortcutlist.NewList().
		ShowSecondaryText(false)
	ap.populateBlockDeviceOptions()

	ap.helpText = tview.NewTextView().
		SetText(uitext.DiskHelp)

	textWidth, textHeight := uiutils.MinTextViewWithNoWrapSize(ap.helpText)
	centeredText := uiutils.Center(textWidth, textHeight, ap.helpText)

	listWidth, listHeight := uiutils.MinListSize(ap.deviceList)
	centeredList := uiutils.Center(listWidth, listHeight, ap.deviceList)

	ap.flex = tview.NewFlex().
		SetDirection(tview.FlexRow).
		AddItem(centeredText, textHeight, textProportion, false).
		AddItem(centeredList, listHeight, listProportion, true).
		AddItem(ap.navBar, navBarHeight, navBarProportion, false)

	ap.centeredFlex = uiutils.CenterVerticallyDynamically(ap.flex)

	// Box styling
	ap.helpText.SetBorderPadding(defaultPadding, defaultPadding, defaultPadding, defaultPadding)
	ap.centeredFlex.SetBackgroundColor(tview.Styles.PrimitiveBackgroundColor)

	return
}

// HandleInput handles custom input.
func (ap *AutoPartitionWidget) HandleInput(event *tcell.EventKey) *tcell.EventKey {
	if ap.navBar.UnfocusedInputHandler(event) {
		return nil
	}

	return event
}

// Reset resets the page, undoing any user input.
func (ap *AutoPartitionWidget) Reset() (err error) {
	ap.deviceList.SetCurrentItem(0)
	ap.navBar.ClearUserFeedback()
	ap.navBar.SetSelectedButton(nextButtonIndex)

	return
}

// Name returns the friendly name of the view.
func (ap *AutoPartitionWidget) Name() string {
	return "AUTOPARTITIONWIDGET"
}

// Title returns the title of the view.
func (ap *AutoPartitionWidget) Title() string {
	return uitext.DiskTitle
}

// Primitive returns the primary primitive to be rendered for the view.
func (ap *AutoPartitionWidget) Primitive() tview.Primitive {
	return ap.centeredFlex
}

// SelectedSystemDevice returns the index of the currently selected system device.
func (ap *AutoPartitionWidget) SelectedSystemDevice() int {
	return ap.deviceList.GetCurrentItem()
}

func (ap *AutoPartitionWidget) saveSelectedDevice() {
	currentItem := ap.deviceList.GetCurrentItem()
	selectedDevicePath := ap.systemDevices[currentItem].DevicePath

	logger.Log.Infof("Selected device: %s", selectedDevicePath)
	ap.hostConfigData.DiskPath = selectedDevicePath
}

func (ap *AutoPartitionWidget) populateBlockDeviceOptions() {
	for _, disk := range ap.systemDevices {
		formattedSize := diskutils.BytesToSizeAndUnit(disk.RawDiskSize)
		diskRepresentation := fmt.Sprintf("%s - %s @ %s", disk.Model, formattedSize, disk.DevicePath)
		ap.deviceList.AddItem(diskRepresentation, "", 0, nil)
	}
}
