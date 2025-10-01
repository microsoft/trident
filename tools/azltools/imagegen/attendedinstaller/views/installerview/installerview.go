// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package installerview

import (
	"fmt"

	"github.com/gdamore/tcell"
	"github.com/rivo/tview"

	"tridenttools/azltools/imagegen/attendedinstaller/primitives/customshortcutlist"
	"tridenttools/azltools/imagegen/attendedinstaller/primitives/navigationbar"
	"tridenttools/azltools/imagegen/attendedinstaller/speakuputils"
	"tridenttools/azltools/imagegen/attendedinstaller/uitext"
	"tridenttools/azltools/imagegen/attendedinstaller/uiutils"
	"tridenttools/azltools/imagegen/configuration"
	"tridenttools/azltools/internal/logger"
)

// UI constants.
const (
	// default to <Next>
	defaultNavButton = 1
	defaultPadding   = 1

	listProportion = 0

	navBarHeight     = 0
	navBarProportion = 1
)

const (
	terminalUISpeechOption = iota
	terminalUINoSpeechOption
)

// InstallerView contains the installer selection UI.
type InstallerView struct {
	optionList       *customshortcutlist.List
	navBar           *navigationbar.NavigationBar
	flex             *tview.Flex
	centeredFlex     *tview.Flex
	installerOptions []string
	needsToPrompt    bool

	hostConfigData *configuration.TridentConfigData
}

// New creates and returns a new InstallerView.
func New() *InstallerView {
	iv := &InstallerView{}

	// Only offer terminal-based installer options
	iv.installerOptions = []string{uitext.InstallerTerminalOption, uitext.InstallerTerminalNoSpeechOption}

	iv.needsToPrompt = (len(iv.installerOptions) > 1)

	return iv
}

// Initialize initializes the view.
func (iv *InstallerView) Initialize(hostConfigData *configuration.TridentConfigData, backButtonText string, app *tview.Application, nextPage, previousPage, quit, refreshTitle func()) (err error) {
	iv.hostConfigData = hostConfigData
	iv.navBar = navigationbar.NewNavigationBar().
		AddButton(backButtonText, previousPage).
		AddButton(uitext.ButtonNext, func() {
			iv.onNextButton(nextPage)
		}).
		SetAlign(tview.AlignCenter)

	iv.optionList = customshortcutlist.NewList().
		ShowSecondaryText(false)

	err = iv.populateInstallerOptions()
	if err != nil {
		return
	}

	listWidth, listHeight := uiutils.MinListSize(iv.optionList)
	centeredList := uiutils.Center(listWidth, listHeight, iv.optionList)

	iv.flex = tview.NewFlex().
		SetDirection(tview.FlexRow).
		AddItem(centeredList, listHeight, listProportion, true).
		AddItem(iv.navBar, navBarHeight, navBarProportion, false)

	iv.centeredFlex = uiutils.CenterVerticallyDynamically(iv.flex)

	// Box styling
	iv.optionList.SetBorderPadding(defaultPadding, defaultPadding, defaultPadding, defaultPadding)

	iv.centeredFlex.SetBackgroundColor(tview.Styles.PrimitiveBackgroundColor)

	return
}

// HandleInput handles custom input.
func (iv *InstallerView) HandleInput(event *tcell.EventKey) *tcell.EventKey {
	if iv.navBar.UnfocusedInputHandler(event) {
		return nil
	}

	return event
}

// NeedsToPrompt returns true if this view should be shown to the user so an installer can be selected.
func (iv *InstallerView) NeedsToPrompt() bool {
	return iv.needsToPrompt
}

// Reset resets the page, undoing any user input.
func (iv *InstallerView) Reset() (err error) {
	iv.navBar.ClearUserFeedback()
	iv.navBar.SetSelectedButton(defaultNavButton)

	iv.optionList.SetCurrentItem(0)

	return
}

// Name returns the friendly name of the view.
func (iv *InstallerView) Name() string {
	return "INSTALLER"
}

// Title returns the title of the view.
func (iv *InstallerView) Title() string {
	return uitext.InstallerExperienceTitle
}

// Primitive returns the primary primitive to be rendered for the view.
func (iv *InstallerView) Primitive() tview.Primitive {
	return iv.centeredFlex
}

// OnShow gets called when the view is shown to the user
func (iv *InstallerView) OnShow() {
	err := speakuputils.StartSpeakup()
	if err != nil {
		logger.Log.Warnf("Failed to start speakup, continuing")
		err = nil
	}
}

func (iv *InstallerView) onNextButton(nextPage func()) {
	switch iv.optionList.GetCurrentItem() {
	case terminalUINoSpeechOption:
		err := speakuputils.StopSpeakup()
		if err != nil {
			logger.Log.Warnf("Failed to stop speakup, continuing")
			err = nil
		}
		fallthrough
	case terminalUISpeechOption:
		nextPage()
	default:
		logger.Log.Panicf("Unknown installer option: %d", iv.optionList.GetCurrentItem())
	}
}

func (iv *InstallerView) populateInstallerOptions() (err error) {
	if len(iv.installerOptions) == 0 {
		return fmt.Errorf("no installer options found")
	}

	for _, option := range iv.installerOptions {
		iv.optionList.AddItem(option, "", 0, nil)
	}

	return
}
